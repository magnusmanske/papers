use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::prelude::*;
use config::{Config, File};
use dashmap::DashSet;
use mysql as my;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::info;
use wikibase::mediawiki::api::Api;

use crate::sourcemd_command::SourceMDcommand;

#[derive(Debug, Clone)]
pub struct SourceMD {
    params: Value,
    running_batch_ids: DashSet<i64>,
    failed_batch_ids: DashSet<i64>,
    pool: Option<my::Pool>,
    mw_api: Arc<RwLock<Api>>,
}

impl SourceMD {
    #[cfg(test)]
    pub(crate) fn new_for_testing(mw_api: Arc<RwLock<Api>>) -> Self {
        Self {
            params: json!({}),
            running_batch_ids: DashSet::new(),
            failed_batch_ids: DashSet::new(),
            pool: None,
            mw_api,
        }
    }

    pub async fn new(ini_file: &str) -> Result<Self> {
        Ok(Self {
            params: json!({}),
            running_batch_ids: DashSet::new(),
            failed_batch_ids: DashSet::new(),
            pool: None,
            mw_api: Arc::new(RwLock::new(Self::create_mw_api(ini_file).await?)),
        })
    }

    pub async fn set_batch_failed(&self, batch_id: i64) {
        self.failed_batch_ids.insert(batch_id);
    }

    pub fn mw_api(&self) -> Arc<RwLock<Api>> {
        self.mw_api.clone()
    }

    /// Borrow the configured MySQL pool, or return a contextual error if
    /// `init()` has not been run / the connection could not be established.
    fn pool(&self) -> Result<&my::Pool> {
        self.pool.as_ref().ok_or_else(|| anyhow!("no MySQL pool configured"))
    }

    pub fn restart_batch(&self, batch_id: i64) -> Result<()> {
        // NOTE: these two UPDATEs are not wrapped in a transaction. If the
        // first succeeds and the second fails the DB is left inconsistent
        // (batch RUNNING, commands still RUNNING). Tracked as a P2 follow-up
        // in audits/STATUS.md.
        let pool = self.pool()?;
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`="RUNNING",`last_action`=? WHERE id=?"#,
            (my::Value::from(self.timestamp()), my::Value::Int(batch_id)),
        )
        .with_context(|| format!("restart_batch: setting batch {batch_id} RUNNING"))?;
        pool.prep_exec(
            r#"UPDATE `command` SET `status`="TODO",`note`="" WHERE `status`="RUNNING" AND `batch_id`=?"#,
            (my::Value::Int(batch_id),),
        )
        .with_context(|| {
            format!("restart_batch: resetting RUNNING commands of batch {batch_id} to TODO")
        })?;
        Ok(())
    }

    pub async fn set_batch_running(&self, batch_id: i64) {
        info!(batch_id, "starting batch");
        self.running_batch_ids.insert(batch_id);
        info!(running = self.number_of_bots_running().await, "bots currently running");
    }

    pub async fn number_of_bots_running(&self) -> usize {
        self.running_batch_ids.len()
    }

    pub fn timestamp(&self) -> String {
        let now = Utc::now();
        now.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// Find the next eligible batch to run.
    ///
    /// Returns:
    /// - `Ok(Some(id))` if a batch is available,
    /// - `Ok(None)` if no batch matches the criteria,
    /// - `Err(_)` if the DB query itself fails (caller should back off, not
    ///   confuse this with "no work").
    pub async fn get_next_batch(&self) -> Result<Option<i64>> {
        let pool = self.pool()?;

        let sql = r#"SELECT * FROM batch WHERE `status` ='TODO' AND NOT EXISTS (SELECT * FROM command WHERE batch_id=batch.id AND `status` IN ("RUNNING","TODO") AND `mode` NOT IN ("CREATE_PAPER_BY_ID","ADD_AUTHOR_TO_PUBLICATION")) ORDER BY `last_action`"#;
        let result = pool.prep_exec(sql, ()).context("get_next_batch query")?;
        for row in result {
            // A single malformed row should not abort the whole search.
            let Ok(row) = row else { continue };
            let id = match &row["id"] {
                my::Value::Int(x) => *x,
                _ => continue,
            };
            if self.running_batch_ids.contains(&id) || self.failed_batch_ids.contains(&id) {
                continue;
            }
            return Ok(Some(id));
        }
        Ok(None)
    }

    pub async fn deactivate_batch_run(&self, batch_id: i64) -> Result<()> {
        info!(batch_id, "deactivating batch");
        self.set_batch_finished(batch_id)?;
        self.running_batch_ids.remove(&batch_id);
        info!(running = self.number_of_bots_running().await, "bots currently running");
        Ok(())
    }

    pub fn set_batch_finished(&self, batch_id: i64) -> Result<()> {
        info!(batch_id, "marking batch finished");
        self.set_batch_status("DONE", batch_id)
    }

    pub fn check_batch_not_stopped(&self, batch_id: i64) -> Result<()> {
        let pool = self.pool.as_ref().ok_or_else(|| {
            anyhow!("QuickStatementsConfig::check_batch_not_stopped: Can't get DB handle")
        })?;
        let sql: String = format!(
            "SELECT * FROM batch WHERE id={} AND `status` NOT IN ('RUNNING','TODO')",
            batch_id
        );
        let mut result = pool.prep_exec(sql, ())?;
        // trunk-ignore(clippy/never_loop)
        if result.next().is_some() {
            return Err(anyhow!(
                "QuickStatementsConfig::check_batch_not_stopped: batch #{} is not RUNNING or TODO",
                batch_id
            ));
        }
        Ok(())
    }

    fn set_batch_status(&self, status: &str, batch_id: i64) -> Result<()> {
        let pool = self.pool()?;
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`=?,`last_action`=? WHERE id=?"#,
            (my::Value::from(status), my::Value::from(self.timestamp()), my::Value::Int(batch_id)),
        )
        .with_context(|| format!("set_batch_status: batch {batch_id} -> {status}"))?;
        self.update_batch_stats(batch_id, pool)
    }

    /// Fetch the next TODO command for a batch.
    ///
    /// Returns `Ok(None)` when there is no work for this batch, `Err` when the
    /// DB query itself fails — callers must distinguish these.
    pub fn get_next_command(&self, batch_id: i64) -> Result<Option<SourceMDcommand>> {
        let pool = self.pool()?;
        let sql = r#"SELECT * FROM command FORCE INDEX (batch_id_4) WHERE `batch_id`=? AND `status`='TODO' ORDER BY `serial_number` LIMIT 1"#;
        let mut result = pool
            .prep_exec(sql, (my::Value::Int(batch_id),))
            .with_context(|| format!("get_next_command: batch {batch_id}"))?;
        let Some(row) = result.next() else { return Ok(None) };
        let row = row.with_context(|| format!("get_next_command: decoding row for batch {batch_id}"))?;
        Ok(SourceMDcommand::new_from_row(row))
    }

    pub fn set_command_status(
        &self,
        command: &mut SourceMDcommand,
        new_status: &str,
        new_message: Option<String>,
    ) -> Result<()> {
        let pool = self.pool()?;
        pool.prep_exec(
            r#"UPDATE `command` SET `status`=?,`note`=?,`q`=? WHERE `id`=?"#,
            (
                my::Value::from(new_status),
                my::Value::from(new_message.unwrap_or_default()),
                my::Value::from(&command.q),
                my::Value::from(&command.id),
            ),
        )
        .with_context(|| {
            format!("set_command_status: command {} -> {}", command.id, new_status)
        })?;
        self.update_batch_stats(command.batch_id, pool)
    }

    fn update_batch_stats(&self, batch_id: i64, pool: &my::Pool) -> Result<()> {
        let mut j = json!({"TOTAL": 0});
        let sql =
            r#"SELECT `status`,count(*) AS cnt FROM command WHERE batch_id=? GROUP BY `status`"#;
        let result = pool
            .prep_exec(sql, (my::Value::from(batch_id),))
            .with_context(|| format!("update_batch_stats: aggregating batch {batch_id}"))?;
        for row in result {
            // Skip individual malformed rows rather than abort the aggregation.
            let Ok(row) = row else { continue };
            let status = match &row["status"] {
                my::Value::Bytes(x) => String::from_utf8_lossy(x),
                _ => continue,
            };
            let cnt = match &row["cnt"] {
                my::Value::Int(x) => *x,
                _ => continue,
            };
            if let Some(obj) = j.as_object_mut() {
                obj.insert(status.to_string(), json!(cnt));
            }
            match j["TOTAL"].as_i64() {
                Some(i) => j["TOTAL"] = json!(cnt + i),
                None => j["TOTAL"] = json!(cnt),
            }
        }
        pool.prep_exec(
            r#"UPDATE `batch` SET `overview`=? WHERE `id`=?"#,
            (my::Value::from(format!("{}", j)), my::Value::from(batch_id)),
        )
        .with_context(|| format!("update_batch_stats: writing overview for batch {batch_id}"))?;
        Ok(())
    }

    /// Initialise the MySQL pool used by the bot loop.
    ///
    /// `ini_file` is the same path the user passes via `--config` — it must
    /// contain a `[client]` section with `user` and `password` fields for the
    /// SourceMD database (in addition to the `[user]` section that
    /// `create_mw_api` reads for Wikidata login).
    pub fn init(&mut self, ini_file: &str) -> Result<()> {
        let settings = Config::builder()
            .add_source(File::with_name(ini_file))
            .build()
            .with_context(|| format!("opening config file '{ini_file}'"))?;
        self.params["mysql"]["user"] =
            json!(settings.get_string("client.user").context("missing client.user")?);
        self.params["mysql"]["pass"] =
            json!(settings.get_string("client.password").context("missing client.password")?);
        self.params["mysql"]["schema"] = json!("s52680__sourcemd_batches_p");

        // Try each known DB host in turn.
        for (host, port) in
            [("tools-db", 3306u64), ("tools.labsdb", 3306), ("localhost", 3307)]
        {
            self.params["mysql"]["host"] = json!(host);
            self.params["mysql"]["port"] = json!(port);
            self.create_mysql_pool();
            if self.pool.is_some() {
                break;
            }
        }

        let pool = self.pool.as_ref().ok_or_else(|| {
            // Do NOT log connection-string contents — they include the password.
            anyhow!("could not establish a MySQL connection to any known host")
        })?;
        pool.prep_exec(r#"UPDATE `batch` SET `status`='TODO' WHERE status='RUNNING'"#, ())
            .context("resetting RUNNING batches to TODO on startup")?;
        Ok(())
    }

    fn create_mysql_pool(&mut self) {
        let mut builder = my::OptsBuilder::new();
        // println!("{}", &self.params);
        builder
            .ip_or_hostname(self.params["mysql"]["host"].as_str())
            .db_name(self.params["mysql"]["schema"].as_str())
            .user(self.params["mysql"]["user"].as_str())
            .pass(self.params["mysql"]["pass"].as_str());
        if let Some(port) = self.params["mysql"]["port"].as_u64() {
            builder.tcp_port(port as u16);
        }

        // Min 2, max 7 connections
        self.pool = my::Pool::new_manual(2, 7, builder).ok()
    }

    pub async fn create_mw_api(ini_file: &str) -> Result<Api> {
        let mut mw_api = Api::new("https://www.wikidata.org/w/api.php").await?;
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        let settings = Config::builder().add_source(File::with_name(ini_file)).build()?;
        match settings.get_string("user.token") {
            Ok(token) => {
                // Use OAuth2 token
                mw_api.set_oauth2(&token);
            },
            Err(_) => {
                // Use username/password login
                let lgname = settings.get_string("user.user")?;
                let lgpass = settings.get_string("user.pass")?;
                info!(user = %lgname, "Wikidata login (username/password)");
                mw_api.login(lgname, lgpass).await?;
            },
        }
        Ok(mw_api)
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use wikibase::mediawiki::api::Api;
    use wiremock::{
        matchers::{method, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;
    use crate::sourcemd_command::{SourceMDcommand, SourceMDcommandMode};

    const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");

    async fn start_mock_server() -> MockServer {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("meta", "siteinfo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(SITEINFO),
            )
            .mount(&mock_server)
            .await;
        mock_server
    }

    /// Build a SourceMD that points at a wiremock-backed Api with no DB pool.
    async fn make_sourcemd(mock_server: &MockServer) -> SourceMD {
        let api = Api::new(&mock_server.uri()).await.unwrap();
        SourceMD::new_for_testing(Arc::new(RwLock::new(api)))
    }

    #[tokio::test]
    async fn timestamp_format() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        let ts = smd.timestamp();
        // Format is "YYYY-MM-DD HH:MM:SS"
        let re = Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$").unwrap();
        assert!(re.is_match(&ts), "timestamp {} does not match expected format", ts);
    }

    #[tokio::test]
    async fn number_of_bots_running_starts_at_zero() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_eq!(smd.number_of_bots_running().await, 0);
    }

    #[tokio::test]
    async fn set_batch_running_increments_count() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        smd.set_batch_running(42).await;
        assert_eq!(smd.number_of_bots_running().await, 1);
        smd.set_batch_running(43).await;
        assert_eq!(smd.number_of_bots_running().await, 2);
    }

    #[tokio::test]
    async fn set_batch_running_is_idempotent_per_id() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        smd.set_batch_running(42).await;
        smd.set_batch_running(42).await;
        // Inserting the same id twice should not double-count
        assert_eq!(smd.number_of_bots_running().await, 1);
    }

    #[tokio::test]
    async fn set_batch_failed_records_the_id() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        smd.set_batch_failed(99).await;
        assert!(smd.failed_batch_ids.contains(&99));
    }

    // After the P0-4/P0-5 fix the DB layer returns `Result`; the no-pool case
    // is now an explicit error rather than a silent `None`, so callers can
    // distinguish "no work" from "DB unreachable".

    fn assert_no_pool_err<T: std::fmt::Debug>(r: Result<T>) {
        let err = r.unwrap_err().to_string();
        assert!(err.contains("no MySQL pool"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn restart_batch_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.restart_batch(1));
    }

    #[tokio::test]
    async fn get_next_batch_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.get_next_batch().await);
    }

    #[tokio::test]
    async fn deactivate_batch_run_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.deactivate_batch_run(1).await);
    }

    #[tokio::test]
    async fn set_batch_finished_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.set_batch_finished(1));
    }

    #[tokio::test]
    async fn check_batch_not_stopped_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.check_batch_not_stopped(1).is_err());
    }

    #[tokio::test]
    async fn get_next_command_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.get_next_command(1));
    }

    #[tokio::test]
    async fn set_command_status_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        let mut cmd = SourceMDcommand {
            id: 1,
            batch_id: 1,
            serial_number: 1,
            mode: SourceMDcommandMode::Dummy,
            identifier: "x".to_string(),
            status: "TODO".to_string(),
            note: String::new(),
            q: String::new(),
            auto_escalate: false,
        };
        assert_no_pool_err(smd.set_command_status(&mut cmd, "RUNNING", None));
    }

    #[tokio::test]
    async fn mw_api_returns_a_handle() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        // mw_api() returns a clone of the Arc; both should point to the same RwLock
        let h1 = smd.mw_api();
        let h2 = smd.mw_api();
        assert!(Arc::ptr_eq(&h1, &h2));
    }
}
