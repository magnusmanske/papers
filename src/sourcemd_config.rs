use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::prelude::*;
use config::{Config, File};
use dashmap::DashSet;
use mysql_async as my;
use mysql_async::prelude::Queryable;
use tokio::sync::RwLock;
use tracing::info;
use wikibase::mediawiki::api::Api;

use crate::sourcemd_command::SourceMDcommand;

#[derive(Debug, Clone)]
pub struct SourceMD {
    running_batch_ids: DashSet<i64>,
    failed_batch_ids: DashSet<i64>,
    pool: Option<my::Pool>,
    mw_api: Arc<RwLock<Api>>,
}

impl SourceMD {
    #[cfg(test)]
    pub(crate) fn new_for_testing(mw_api: Arc<RwLock<Api>>) -> Self {
        Self {
            running_batch_ids: DashSet::new(),
            failed_batch_ids: DashSet::new(),
            pool: None,
            mw_api,
        }
    }

    pub async fn new(ini_file: &str) -> Result<Self> {
        Ok(Self {
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

    /// Check out a connection from the pool. All DB-touching public methods
    /// flow through here so error context is uniform.
    async fn conn(&self) -> Result<my::Conn> {
        self.pool()?.get_conn().await.context("checking out MySQL connection from pool")
    }

    pub async fn restart_batch(&self, batch_id: i64) -> Result<()> {
        // NOTE: these two UPDATEs share one Conn but are not wrapped in a
        // transaction. If the first succeeds and the second fails the DB is
        // left inconsistent (batch RUNNING, commands still RUNNING). Tracked
        // as P2-DB-1 in audits/STATUS.md.
        let mut conn = self.conn().await?;
        conn.exec_drop(
            r#"UPDATE `batch` SET `status`="RUNNING",`last_action`=? WHERE id=?"#,
            (self.timestamp(), batch_id),
        )
        .await
        .with_context(|| format!("restart_batch: setting batch {batch_id} RUNNING"))?;
        conn.exec_drop(
            r#"UPDATE `command` SET `status`="TODO",`note`="" WHERE `status`="RUNNING" AND `batch_id`=?"#,
            (batch_id,),
        )
        .await
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
        let mut conn = self.conn().await?;
        let sql = r#"SELECT * FROM batch WHERE `status` ='TODO' AND NOT EXISTS (SELECT * FROM command WHERE batch_id=batch.id AND `status` IN ("RUNNING","TODO") AND `mode` NOT IN ("CREATE_PAPER_BY_ID","ADD_AUTHOR_TO_PUBLICATION")) ORDER BY `last_action`"#;
        // SELECT returns a small candidate set (TODO batches only); collecting
        // ids into a Vec avoids juggling streaming-iterator lifetimes for the
        // running/failed-set filter that follows.
        let candidates: Vec<i64> = conn
            .query_map(sql, |row: my::Row| match &row["id"] {
                my::Value::Int(x) => *x,
                _ => -1,
            })
            .await
            .context("get_next_batch query")?;
        for id in candidates {
            if id < 0 {
                continue;
            }
            if self.running_batch_ids.contains(&id) || self.failed_batch_ids.contains(&id) {
                continue;
            }
            return Ok(Some(id));
        }
        Ok(None)
    }

    pub async fn deactivate_batch_run(&self, batch_id: i64) -> Result<()> {
        info!(batch_id, "deactivating batch");
        self.set_batch_finished(batch_id).await?;
        self.running_batch_ids.remove(&batch_id);
        info!(running = self.number_of_bots_running().await, "bots currently running");
        Ok(())
    }

    pub async fn set_batch_finished(&self, batch_id: i64) -> Result<()> {
        info!(batch_id, "marking batch finished");
        self.set_batch_status("DONE", batch_id).await
    }

    pub async fn check_batch_not_stopped(&self, batch_id: i64) -> Result<()> {
        let mut conn = self.conn().await.with_context(|| {
            format!("check_batch_not_stopped: getting DB handle for batch {batch_id}")
        })?;
        let sql = r#"SELECT 1 FROM batch WHERE id=? AND `status` NOT IN ('RUNNING','TODO')"#;
        let stopped: Option<i64> = conn
            .exec_first(sql, (batch_id,))
            .await
            .with_context(|| format!("check_batch_not_stopped: batch {batch_id}"))?;
        if stopped.is_some() {
            return Err(anyhow!("batch #{batch_id} is not RUNNING or TODO"));
        }
        Ok(())
    }

    async fn set_batch_status(&self, status: &str, batch_id: i64) -> Result<()> {
        let mut conn = self.conn().await?;
        conn.exec_drop(
            r#"UPDATE `batch` SET `status`=?,`last_action`=? WHERE id=?"#,
            (status, self.timestamp(), batch_id),
        )
        .await
        .with_context(|| format!("set_batch_status: batch {batch_id} -> {status}"))?;
        self.update_batch_stats(batch_id, &mut conn).await
    }

    /// Fetch the next TODO command for a batch.
    ///
    /// Returns `Ok(None)` when there is no work for this batch, `Err` when the
    /// DB query itself fails — callers must distinguish these.
    pub async fn get_next_command(&self, batch_id: i64) -> Result<Option<SourceMDcommand>> {
        let mut conn = self.conn().await?;
        let sql = r#"SELECT * FROM command FORCE INDEX (batch_id_4) WHERE `batch_id`=? AND `status`='TODO' ORDER BY `serial_number` LIMIT 1"#;
        let row: Option<my::Row> = conn
            .exec_first(sql, (batch_id,))
            .await
            .with_context(|| format!("get_next_command: batch {batch_id}"))?;
        Ok(row.and_then(SourceMDcommand::new_from_row))
    }

    pub async fn set_command_status(
        &self,
        command: &mut SourceMDcommand,
        new_status: &str,
        new_message: Option<String>,
    ) -> Result<()> {
        let mut conn = self.conn().await?;
        // NOTE: UPDATE + update_batch_stats are not atomic. Tracked as P2-DB-1.
        conn.exec_drop(
            r#"UPDATE `command` SET `status`=?,`note`=?,`q`=? WHERE `id`=?"#,
            (
                new_status,
                new_message.unwrap_or_default(),
                command.q.clone(),
                command.id,
            ),
        )
        .await
        .with_context(|| {
            format!("set_command_status: command {} -> {}", command.id, new_status)
        })?;
        self.update_batch_stats(command.batch_id, &mut conn).await
    }

    async fn update_batch_stats(&self, batch_id: i64, conn: &mut my::Conn) -> Result<()> {
        let sql =
            r#"SELECT `status`,count(*) AS cnt FROM command WHERE batch_id=? GROUP BY `status`"#;
        let counts: Vec<(String, i64)> = conn
            .exec_map(sql, (batch_id,), |row: my::Row| {
                let status = match &row["status"] {
                    my::Value::Bytes(x) => String::from_utf8_lossy(x).to_string(),
                    _ => String::new(),
                };
                let cnt = match &row["cnt"] {
                    my::Value::Int(x) => *x,
                    _ => 0,
                };
                (status, cnt)
            })
            .await
            .with_context(|| format!("update_batch_stats: aggregating batch {batch_id}"))?;

        let mut j = json!({"TOTAL": 0});
        let mut total: i64 = 0;
        for (status, cnt) in counts {
            if status.is_empty() {
                continue;
            }
            if let Some(obj) = j.as_object_mut() {
                obj.insert(status, json!(cnt));
            }
            total += cnt;
        }
        j["TOTAL"] = json!(total);

        conn.exec_drop(
            r#"UPDATE `batch` SET `overview`=? WHERE `id`=?"#,
            (j.to_string(), batch_id),
        )
        .await
        .with_context(|| format!("update_batch_stats: writing overview for batch {batch_id}"))?;
        Ok(())
    }

    /// Initialise the MySQL pool used by the bot loop.
    ///
    /// `ini_file` is the same path the user passes via `--config` — it must
    /// contain a `[client]` section with `user` and `password` fields for the
    /// SourceMD database (in addition to the `[user]` section that
    /// `create_mw_api` reads for Wikidata login).
    pub async fn init(&mut self, ini_file: &str) -> Result<()> {
        let settings = Config::builder()
            .add_source(File::with_name(ini_file))
            .build()
            .with_context(|| format!("opening config file '{ini_file}'"))?;
        let user = settings.get_string("client.user").context("missing client.user")?;
        let pass = settings.get_string("client.password").context("missing client.password")?;
        let schema = "s52680__sourcemd_batches_p".to_string();

        // Try each known DB host in turn. `mysql_async::Pool::new` is lazy and
        // never fails up-front, so verify reachability with a trial conn.
        let candidates: [(&str, u16); 3] =
            [("tools-db", 3306), ("tools.labsdb", 3306), ("localhost", 3307)];
        for (host, port) in candidates {
            let opts: my::Opts = my::OptsBuilder::default()
                .ip_or_hostname(host)
                .tcp_port(port)
                .user(Some(user.clone()))
                .pass(Some(pass.clone()))
                .db_name(Some(schema.clone()))
                .into();
            let pool = my::Pool::new(opts);
            match pool.get_conn().await {
                Ok(_) => {
                    info!(host, port, "MySQL pool connected");
                    self.pool = Some(pool);
                    break;
                },
                Err(e) => {
                    info!(host, port, error = %e, "MySQL host unreachable, trying next");
                    // pool drops here; lazy, so nothing to disconnect.
                },
            }
        }

        let mut conn = self.conn().await.context(
            // Do NOT log connection-string contents — they include the password.
            "could not establish a MySQL connection to any known host",
        )?;
        conn.exec_drop(r#"UPDATE `batch` SET `status`='TODO' WHERE status='RUNNING'"#, ())
            .await
            .context("resetting RUNNING batches to TODO on startup")?;
        Ok(())
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
        assert_no_pool_err(smd.restart_batch(1).await);
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
        assert_no_pool_err(smd.set_batch_finished(1).await);
    }

    #[tokio::test]
    async fn check_batch_not_stopped_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.check_batch_not_stopped(1).await.is_err());
    }

    #[tokio::test]
    async fn get_next_command_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert_no_pool_err(smd.get_next_command(1).await);
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
        assert_no_pool_err(smd.set_command_status(&mut cmd, "RUNNING", None).await);
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
