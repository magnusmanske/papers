use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::prelude::*;
use config::{Config, File};
use dashmap::DashSet;
use mysql_async as my;
use mysql_async::prelude::Queryable;
use mysql_async::TxOpts;
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
        let mut conn = self.conn().await?;
        let mut txn = conn
            .start_transaction(TxOpts::default())
            .await
            .with_context(|| format!("restart_batch: opening txn for batch {batch_id}"))?;
        txn.exec_drop(
            r#"UPDATE `batch` SET `status`="RUNNING",`last_action`=? WHERE id=?"#,
            (self.timestamp(), batch_id),
        )
        .await
        .with_context(|| format!("restart_batch: setting batch {batch_id} RUNNING"))?;
        txn.exec_drop(
            r#"UPDATE `command` SET `status`="TODO",`note`="" WHERE `status`="RUNNING" AND `batch_id`=?"#,
            (batch_id,),
        )
        .await
        .with_context(|| {
            format!("restart_batch: resetting RUNNING commands of batch {batch_id} to TODO")
        })?;
        txn.commit()
            .await
            .with_context(|| format!("restart_batch: commit for batch {batch_id}"))?;
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
        let mut txn = conn
            .start_transaction(TxOpts::default())
            .await
            .with_context(|| format!("set_batch_status: opening txn for batch {batch_id}"))?;
        txn.exec_drop(
            r#"UPDATE `batch` SET `status`=?,`last_action`=? WHERE id=?"#,
            (status, self.timestamp(), batch_id),
        )
        .await
        .with_context(|| format!("set_batch_status: batch {batch_id} -> {status}"))?;
        self.update_batch_stats(batch_id, &mut txn).await?;
        txn.commit()
            .await
            .with_context(|| format!("set_batch_status: commit for batch {batch_id}"))?;
        Ok(())
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
        let mut txn = conn.start_transaction(TxOpts::default()).await.with_context(|| {
            format!("set_command_status: opening txn for command {}", command.id)
        })?;
        txn.exec_drop(
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
        self.update_batch_stats(command.batch_id, &mut txn).await?;
        txn.commit().await.with_context(|| {
            format!("set_command_status: commit for command {}", command.id)
        })?;
        Ok(())
    }

    async fn update_batch_stats<Q>(&self, batch_id: i64, conn: &mut Q) -> Result<()>
    where
        Q: Queryable + Send,
    {
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
    use wiremock::MockServer;

    use super::*;
    use crate::sourcemd_command::{SourceMDcommand, SourceMDcommandMode};
    use crate::test_helpers::start_mediawiki_mock_server as start_mock_server;

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

/// Live-DB integration tests.
///
/// These tests exercise the actual MySQL code path against a real database.
/// They are `#[ignore]` by default so they don't run in CI (or under a plain
/// `cargo test`). To run them locally:
///
/// 1. Open an SSH tunnel to Toolforge:
///    `ssh -L 3307:tools-db:3306 -N <user>@login.toolforge.org`
///
/// 2. Designate a TEST schema you control (NEVER point this at
///    `s52680__sourcemd_batches_p` — the test setup drops and recreates the
///    `batch` and `command` tables in whatever schema you specify):
///
///    ```
///    PAPERS_TEST_DB_SCHEMA=s12345__papers_test_p \
///    PAPERS_TEST_DB_USER=s12345 \
///    PAPERS_TEST_DB_PASS=... \
///    cargo test --lib live_db_tests -- --ignored --nocapture
///    ```
///
/// 3. If the required env vars aren't set, every test in this module returns
///    early with a one-line `skipping` notice instead of failing.
///
/// **Schema caveat.** This module CREATEs minimal `batch` and `command`
/// tables matching what the production SQL reads/writes. The columns and the
/// `batch_id_4` index are derived from the source code, not from production
/// DDL. If production has additional columns/indexes the tests won't notice;
/// if production lacks a column the tests rely on, that mismatch is not
/// detectable from here.
#[cfg(test)]
mod live_db_tests {
    use std::sync::Arc;

    use dashmap::DashSet;
    use mysql_async::prelude::Queryable;
    use tokio::sync::RwLock;
    use wikibase::mediawiki::api::Api;
    use wiremock::{
        matchers::{method, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;
    use crate::sourcemd_command::{SourceMDcommand, SourceMDcommandMode};

    const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");

    struct DbCreds {
        host: String,
        port: u16,
        user: String,
        pass: String,
        schema: String,
    }

    /// Read DB credentials from environment; return `None` if any required
    /// variable is missing so the caller can skip the test cleanly.
    fn db_creds_from_env() -> Option<DbCreds> {
        let user = std::env::var("PAPERS_TEST_DB_USER").ok()?;
        let pass = std::env::var("PAPERS_TEST_DB_PASS").ok()?;
        let schema = std::env::var("PAPERS_TEST_DB_SCHEMA").ok()?;
        let host =
            std::env::var("PAPERS_TEST_DB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = std::env::var("PAPERS_TEST_DB_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(3307);
        Some(DbCreds { host, port, user, pass, schema })
    }

    fn make_pool(creds: &DbCreds) -> my::Pool {
        let opts: my::Opts = my::OptsBuilder::default()
            .ip_or_hostname(creds.host.clone())
            .tcp_port(creds.port)
            .user(Some(creds.user.clone()))
            .pass(Some(creds.pass.clone()))
            .db_name(Some(creds.schema.clone()))
            .into();
        my::Pool::new(opts)
    }

    /// DROP-then-CREATE the two tables the production code touches. Each
    /// test calls this so it starts from a known empty state.
    async fn reset_tables(pool: &my::Pool) {
        let mut conn = pool.get_conn().await.expect("connect to test DB");
        conn.query_drop("DROP TABLE IF EXISTS `command`").await.unwrap();
        conn.query_drop("DROP TABLE IF EXISTS `batch`").await.unwrap();
        conn.query_drop(
            r#"CREATE TABLE `batch` (
                `id` BIGINT NOT NULL PRIMARY KEY AUTO_INCREMENT,
                `status` VARCHAR(32) NOT NULL DEFAULT 'TODO',
                `last_action` DATETIME NULL,
                `overview` TEXT NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"#,
        )
        .await
        .unwrap();
        conn.query_drop(
            r#"CREATE TABLE `command` (
                `id` BIGINT NOT NULL PRIMARY KEY AUTO_INCREMENT,
                `batch_id` BIGINT NOT NULL,
                `serial_number` BIGINT NOT NULL,
                `mode` VARCHAR(64) NOT NULL,
                `identifier` VARCHAR(512) NOT NULL,
                `status` VARCHAR(32) NOT NULL DEFAULT 'TODO',
                `note` TEXT NULL,
                `q` VARCHAR(32) NOT NULL DEFAULT '',
                `auto_escalate` TINYINT(1) NOT NULL DEFAULT 0,
                INDEX `batch_id_4` (`batch_id`, `status`, `serial_number`)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"#,
        )
        .await
        .unwrap();
    }

    /// Insert a single batch row and return its id.
    async fn insert_batch(pool: &my::Pool, status: &str) -> i64 {
        let mut conn = pool.get_conn().await.unwrap();
        conn.exec_drop(
            r#"INSERT INTO `batch` (`status`, `last_action`) VALUES (?, NOW())"#,
            (status,),
        )
        .await
        .unwrap();
        conn.exec_first::<i64, _, _>("SELECT LAST_INSERT_ID()", ()).await.unwrap().unwrap()
    }

    async fn insert_command(
        pool: &my::Pool,
        batch_id: i64,
        serial: i64,
        status: &str,
        mode: &str,
        identifier: &str,
    ) -> i64 {
        let mut conn = pool.get_conn().await.unwrap();
        conn.exec_drop(
            r#"INSERT INTO `command` (`batch_id`,`serial_number`,`mode`,`identifier`,`status`,`note`,`q`,`auto_escalate`)
               VALUES (?, ?, ?, ?, ?, '', '', 0)"#,
            (batch_id, serial, mode, identifier, status),
        )
        .await
        .unwrap();
        conn.exec_first::<i64, _, _>("SELECT LAST_INSERT_ID()", ()).await.unwrap().unwrap()
    }

    async fn fetch_batch_status(pool: &my::Pool, batch_id: i64) -> String {
        let mut conn = pool.get_conn().await.unwrap();
        conn.exec_first::<String, _, _>(
            "SELECT `status` FROM `batch` WHERE id=?",
            (batch_id,),
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn fetch_batch_overview(pool: &my::Pool, batch_id: i64) -> Option<String> {
        let mut conn = pool.get_conn().await.unwrap();
        conn.exec_first::<Option<String>, _, _>(
            "SELECT `overview` FROM `batch` WHERE id=?",
            (batch_id,),
        )
        .await
        .unwrap()
        .flatten()
    }

    async fn fetch_command_status(pool: &my::Pool, command_id: i64) -> String {
        let mut conn = pool.get_conn().await.unwrap();
        conn.exec_first::<String, _, _>(
            "SELECT `status` FROM `command` WHERE id=?",
            (command_id,),
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn start_mock_mw_api() -> (MockServer, Arc<RwLock<Api>>) {
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
        let api = Api::new(&mock_server.uri()).await.unwrap();
        (mock_server, Arc::new(RwLock::new(api)))
    }

    /// Build a `SourceMD` wired to a real DB pool and a wiremock-backed
    /// `mw_api`. The MockServer is returned so its lifetime extends through
    /// the test.
    async fn make_smd(pool: my::Pool) -> (SourceMD, MockServer) {
        let (mock_server, mw_api) = start_mock_mw_api().await;
        let smd = SourceMD {
            running_batch_ids: DashSet::new(),
            failed_batch_ids: DashSet::new(),
            pool: Some(pool),
            mw_api,
        };
        (smd, mock_server)
    }

    /// Run `body` with a fresh test DB. If env is missing, print a one-line
    /// notice and return without running anything (so `--ignored` doesn't
    /// fail on a developer machine that hasn't set up the tunnel).
    async fn with_clean_db<F, Fut>(test_name: &str, body: F)
    where
        F: FnOnce(SourceMD, my::Pool, MockServer) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let Some(creds) = db_creds_from_env() else {
            eprintln!(
                "[skip] {test_name}: PAPERS_TEST_DB_USER / _PASS / _SCHEMA not set"
            );
            return;
        };
        let pool = make_pool(&creds);
        reset_tables(&pool).await;
        let (smd, mock_server) = make_smd(pool.clone()).await;
        body(smd, pool, mock_server).await;
    }

    // === init ===========================================================

    #[ignore]
    #[tokio::test]
    async fn live_init_resets_running_batches_to_todo() {
        with_clean_db("live_init_resets_running_batches_to_todo", |mut smd, pool, _mock| async move {
            // Arrange: drop the pre-wired pool so init() has to find one,
            // and seed two RUNNING batches that init() should flip to TODO.
            insert_batch(&pool, "RUNNING").await;
            insert_batch(&pool, "RUNNING").await;
            smd.pool = None;

            // Use a throwaway ini file with both [user] and [client] sections.
            let tmp = tempfile_with_credentials();

            // Act
            smd.init(tmp.path().to_str().unwrap()).await.expect("init should connect");

            // Assert: both batches now TODO; pool is populated.
            assert!(smd.pool.is_some(), "init must leave a pool behind");
            let mut conn = pool.get_conn().await.unwrap();
            let running_count: i64 = conn
                .query_first("SELECT COUNT(*) FROM `batch` WHERE `status`='RUNNING'")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(running_count, 0, "init should have flipped all RUNNING -> TODO");
        })
        .await;
    }

    /// Build a temp ini file that points init() at the test DB.
    fn tempfile_with_credentials() -> tempfile::NamedTempFile {
        use std::io::Write;
        let creds = db_creds_from_env().expect("env should be set by caller");
        let mut f = tempfile::Builder::new()
            .suffix(".ini")
            .tempfile()
            .expect("create temp ini");
        writeln!(f, "[user]").unwrap();
        writeln!(f, "user = test").unwrap();
        writeln!(f, "pass = test").unwrap();
        writeln!(f, "[client]").unwrap();
        writeln!(f, "user = {}", creds.user).unwrap();
        writeln!(f, "password = {}", creds.pass).unwrap();
        f
    }

    // === restart_batch =================================================

    #[ignore]
    #[tokio::test]
    async fn live_restart_batch_sets_status_and_resets_commands() {
        with_clean_db(
            "live_restart_batch_sets_status_and_resets_commands",
            |smd, pool, _mock| async move {
                let batch_id = insert_batch(&pool, "TODO").await;
                let cmd_run = insert_command(&pool, batch_id, 1, "RUNNING", "CREATE_PAPER_BY_ID", "x").await;
                let cmd_done = insert_command(&pool, batch_id, 2, "DONE", "CREATE_PAPER_BY_ID", "y").await;

                smd.restart_batch(batch_id).await.expect("restart_batch should succeed");

                assert_eq!(fetch_batch_status(&pool, batch_id).await, "RUNNING");
                assert_eq!(
                    fetch_command_status(&pool, cmd_run).await,
                    "TODO",
                    "RUNNING commands should be reset to TODO"
                );
                assert_eq!(
                    fetch_command_status(&pool, cmd_done).await,
                    "DONE",
                    "DONE commands should NOT be touched"
                );
            },
        )
        .await;
    }

    // === get_next_batch =================================================

    #[ignore]
    #[tokio::test]
    async fn live_get_next_batch_returns_oldest_eligible_todo() {
        with_clean_db(
            "live_get_next_batch_returns_oldest_eligible_todo",
            |smd, pool, _mock| async move {
                // Two TODO batches, one with a RUNNING command (ineligible).
                let blocked = insert_batch(&pool, "TODO").await;
                insert_command(&pool, blocked, 1, "RUNNING", "CREATE_PAPER_BY_ID", "x").await;
                let eligible = insert_batch(&pool, "TODO").await;

                let got = smd.get_next_batch().await.expect("query should succeed");
                assert_eq!(
                    got,
                    Some(eligible),
                    "the batch with no RUNNING/TODO non-allowed commands should win"
                );
            },
        )
        .await;
    }

    #[ignore]
    #[tokio::test]
    async fn live_get_next_batch_returns_none_when_empty() {
        with_clean_db(
            "live_get_next_batch_returns_none_when_empty",
            |smd, _pool, _mock| async move {
                assert_eq!(smd.get_next_batch().await.unwrap(), None);
            },
        )
        .await;
    }

    #[ignore]
    #[tokio::test]
    async fn live_get_next_batch_skips_in_memory_running_set() {
        with_clean_db(
            "live_get_next_batch_skips_in_memory_running_set",
            |smd, pool, _mock| async move {
                let only = insert_batch(&pool, "TODO").await;
                smd.running_batch_ids.insert(only);
                assert_eq!(smd.get_next_batch().await.unwrap(), None);
                smd.running_batch_ids.remove(&only);
                assert_eq!(smd.get_next_batch().await.unwrap(), Some(only));
            },
        )
        .await;
    }

    // === get_next_command ===============================================

    #[ignore]
    #[tokio::test]
    async fn live_get_next_command_returns_lowest_serial_todo() {
        with_clean_db(
            "live_get_next_command_returns_lowest_serial_todo",
            |smd, pool, _mock| async move {
                let batch_id = insert_batch(&pool, "TODO").await;
                // Insert in deliberately non-sorted order to verify ORDER BY.
                let _high = insert_command(&pool, batch_id, 9, "TODO", "CREATE_PAPER_BY_ID", "z").await;
                let low = insert_command(&pool, batch_id, 1, "TODO", "CREATE_PAPER_BY_ID", "a").await;
                let _done = insert_command(&pool, batch_id, 0, "DONE", "CREATE_PAPER_BY_ID", "b").await;

                let cmd = smd
                    .get_next_command(batch_id)
                    .await
                    .expect("query")
                    .expect("expected one TODO command");
                assert_eq!(cmd.id, low, "should return the lowest-serial TODO command");
                assert_eq!(cmd.serial_number, 1);
                assert_eq!(cmd.identifier, "a");
                assert_eq!(cmd.mode, SourceMDcommandMode::CreatePaperById);
            },
        )
        .await;
    }

    #[ignore]
    #[tokio::test]
    async fn live_get_next_command_returns_none_when_no_todo() {
        with_clean_db(
            "live_get_next_command_returns_none_when_no_todo",
            |smd, pool, _mock| async move {
                let batch_id = insert_batch(&pool, "TODO").await;
                insert_command(&pool, batch_id, 1, "DONE", "CREATE_PAPER_BY_ID", "a").await;
                assert!(smd.get_next_command(batch_id).await.unwrap().is_none());
            },
        )
        .await;
    }

    // === set_command_status =============================================

    #[ignore]
    #[tokio::test]
    async fn live_set_command_status_persists_and_aggregates_overview() {
        with_clean_db(
            "live_set_command_status_persists_and_aggregates_overview",
            |smd, pool, _mock| async move {
                let batch_id = insert_batch(&pool, "RUNNING").await;
                let cmd_id =
                    insert_command(&pool, batch_id, 1, "TODO", "CREATE_PAPER_BY_ID", "x").await;
                let _other = insert_command(
                    &pool,
                    batch_id,
                    2,
                    "TODO",
                    "CREATE_PAPER_BY_ID",
                    "y",
                )
                .await;

                let mut cmd = SourceMDcommand {
                    id: cmd_id,
                    batch_id,
                    serial_number: 1,
                    mode: SourceMDcommandMode::CreatePaperById,
                    identifier: "x".to_string(),
                    status: "TODO".to_string(),
                    note: String::new(),
                    q: "Q42".to_string(),
                    auto_escalate: false,
                };
                smd.set_command_status(&mut cmd, "DONE", Some("ok".to_string()))
                    .await
                    .expect("set_command_status should succeed");

                // Command row reflects new status, q, and note.
                assert_eq!(fetch_command_status(&pool, cmd_id).await, "DONE");
                let mut conn = pool.get_conn().await.unwrap();
                let (note, q): (String, String) = conn
                    .exec_first("SELECT note, q FROM command WHERE id=?", (cmd_id,))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(note, "ok");
                assert_eq!(q, "Q42");

                // batch.overview was aggregated. Should contain "DONE":1, "TODO":1, "TOTAL":2.
                let overview =
                    fetch_batch_overview(&pool, batch_id).await.expect("overview not null");
                let parsed: serde_json::Value = serde_json::from_str(&overview).unwrap();
                assert_eq!(parsed["DONE"], 1);
                assert_eq!(parsed["TODO"], 1);
                assert_eq!(parsed["TOTAL"], 2);
            },
        )
        .await;
    }

    // === set_batch_finished / check_batch_not_stopped ===================

    #[ignore]
    #[tokio::test]
    async fn live_set_batch_finished_marks_done() {
        with_clean_db("live_set_batch_finished_marks_done", |smd, pool, _mock| async move {
            let batch_id = insert_batch(&pool, "RUNNING").await;
            insert_command(&pool, batch_id, 1, "DONE", "CREATE_PAPER_BY_ID", "x").await;

            smd.set_batch_finished(batch_id).await.expect("set_batch_finished");

            assert_eq!(fetch_batch_status(&pool, batch_id).await, "DONE");
            // update_batch_stats ran as part of set_batch_status.
            let overview = fetch_batch_overview(&pool, batch_id).await.unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&overview).unwrap();
            assert_eq!(parsed["DONE"], 1);
            assert_eq!(parsed["TOTAL"], 1);
        })
        .await;
    }

    #[ignore]
    #[tokio::test]
    async fn live_check_batch_not_stopped_passes_for_running_and_fails_for_done() {
        with_clean_db(
            "live_check_batch_not_stopped_passes_for_running_and_fails_for_done",
            |smd, pool, _mock| async move {
                let running = insert_batch(&pool, "RUNNING").await;
                let todo = insert_batch(&pool, "TODO").await;
                let done = insert_batch(&pool, "DONE").await;

                assert!(smd.check_batch_not_stopped(running).await.is_ok());
                assert!(smd.check_batch_not_stopped(todo).await.is_ok());
                let err = smd.check_batch_not_stopped(done).await.unwrap_err().to_string();
                assert!(
                    err.contains(&format!("batch #{done}")),
                    "error should name the batch: {err}"
                );
            },
        )
        .await;
    }
}
