use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::prelude::*;
use config::{Config, File};
use dashmap::DashSet;
use mysql as my;
use serde_json::Value;
use tokio::sync::RwLock;
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

    pub fn restart_batch(&self, batch_id: i64) -> Option<()> {
        let pool = self.pool.as_ref()?;
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`="RUNNING",`last_action`=? WHERE id=?"#,
            (my::Value::from(self.timestamp()), my::Value::Int(batch_id)),
        )
        .ok()?;
        pool.prep_exec(
            r#"UPDATE `command` SET `status`="TODO",`note`="" WHERE `status`="RUNNING" AND `batch_id`=?"#,
            (my::Value::Int(batch_id),),
        )
        .ok()?;
        Some(())
    }

    pub async fn set_batch_running(&self, batch_id: i64) {
        println!("set_batch_running: Starting batch #{}", batch_id);
        self.running_batch_ids.insert(batch_id);
        println!("Currently {} bots running", self.number_of_bots_running().await);
    }

    pub async fn number_of_bots_running(&self) -> usize {
        self.running_batch_ids.len()
    }

    pub fn timestamp(&self) -> String {
        let now = Utc::now();
        now.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    pub async fn get_next_batch(&self) -> Option<i64> {
        let pool = self.pool.as_ref()?;

        let sql: String = r#"SELECT * FROM batch WHERE `status` ='TODO' AND NOT EXISTS (SELECT * FROM command WHERE batch_id=batch.id AND `status` IN ("RUNNING","TODO") AND `mode` NOT IN ("CREATE_PAPER_BY_ID","ADD_AUTHOR_TO_PUBLICATION")) ORDER BY `last_action`"#.into();
        // let sql: String = "SELECT * FROM batch WHERE id=8117".into(); // TESTING
        // (also 551)
        for row in pool.prep_exec(sql, ()).ok()? {
            let row = row.ok()?;
            let id = match &row["id"] {
                my::Value::Int(x) => *x,
                _ => continue,
            };
            if self.running_batch_ids.contains(&id) || self.failed_batch_ids.contains(&id) {
                continue;
            }
            return Some(id);
        }
        None
    }

    pub async fn deactivate_batch_run(&self, batch_id: i64) -> Option<()> {
        println!("Deactivating batch #{}", batch_id);
        self.set_batch_finished(batch_id)?;
        {
            self.running_batch_ids.remove(&batch_id);
        }
        println!("Currently {} bots running", self.number_of_bots_running().await);
        Some(())
    }

    pub fn set_batch_finished(&self, batch_id: i64) -> Option<()> {
        println!("set_batch_finished: Batch #{}", batch_id);
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

    fn set_batch_status(&self, status: &str, batch_id: i64) -> Option<()> {
        let pool = self.pool.as_ref()?;
        // TODO stats
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`=?,`last_action`=? WHERE id=?"#,
            (my::Value::from(status), my::Value::from(self.timestamp()), my::Value::Int(batch_id)),
        )
        .ok()?;
        self.update_batch_stats(batch_id, pool)
        // self.deactivate_batch_run(batch_id)
    }

    pub fn get_next_command(&self, batch_id: i64) -> Option<SourceMDcommand> {
        let pool = self.pool.as_ref()?;
        let sql = r#"SELECT * FROM command FORCE INDEX (batch_id_4) WHERE `batch_id`=? AND `status`='TODO' ORDER BY `serial_number` LIMIT 1"#;
        // trunk-ignore(clippy/never_loop)
        if let Some(row) = (pool.prep_exec(sql, (my::Value::Int(batch_id),)).ok()?).next() {
            let row = row.ok()?;
            return SourceMDcommand::new_from_row(row);
        }
        None
    }

    pub fn set_command_status(
        &self,
        command: &mut SourceMDcommand,
        new_status: &str,
        new_message: Option<String>,
    ) -> Option<()> {
        let pool = self.pool.as_ref()?;
        pool.prep_exec(
            r#"UPDATE `command` SET `status`=?,`note`=?,`q`=? WHERE `id`=?"#,
            (
                my::Value::from(new_status),
                my::Value::from(new_message.unwrap_or("".to_string())),
                my::Value::from(&command.q),
                my::Value::from(&command.id),
            ),
        )
        .ok()?;
        self.update_batch_stats(command.batch_id, pool)
    }

    fn update_batch_stats(&self, batch_id: i64, pool: &my::Pool) -> Option<()> {
        let mut j = json!({"TOTAL":0});
        let sql =
            r#"SELECT `status`,count(*) AS cnt FROM command WHERE batch_id=? GROUP BY `status`"#;
        for row in pool.prep_exec(sql, (my::Value::from(batch_id),)).ok()? {
            let row = row.ok()?;
            let status = match &row["status"] {
                my::Value::Bytes(x) => String::from_utf8_lossy(x),
                _ => continue,
            };
            let cnt = match &row["cnt"] {
                my::Value::Int(x) => *x,
                _ => continue,
            };
            j.as_object_mut()?.insert(status.to_string(), json!(cnt));
            match j["TOTAL"].as_i64() {
                Some(i) => j["TOTAL"] = json!(cnt + i),
                None => j["TOTAL"] = json!(cnt),
            }
        }
        pool.prep_exec(
            r#"UPDATE `batch` SET `overview`=? WHERE `id`=?"#,
            (my::Value::from(format!("{}", j)), my::Value::from(batch_id)),
        )
        .ok()?;
        Some(())
    }

    pub fn init(&mut self) {
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        let ini_file = "/data/project/sourcemd/rust/papers/replica.my.ini";
        let settings = Config::builder()
            .add_source(File::with_name(ini_file))
            .build()
            .unwrap_or_else(|_| panic!("Replica file '{}' can't be opened", ini_file));
        self.params["mysql"]["user"] =
            json!(settings.get_string("client.user").expect("No client.name"));
        self.params["mysql"]["pass"] =
            json!(settings.get_string("client.password").expect("No client.password"));
        self.params["mysql"]["schema"] = json!("s52680__sourcemd_batches_p");

        // On Labs
        self.params["mysql"]["host"] = json!("tools-db");
        self.params["mysql"]["port"] = json!(3306);
        self.create_mysql_pool();

        // On PetScan
        if self.pool.is_none() {
            self.params["mysql"]["host"] = json!("tools.labsdb");
            self.params["mysql"]["port"] = json!(3306);
            self.create_mysql_pool();
        }

        // Local fallback
        if self.pool.is_none() {
            self.params["mysql"]["host"] = json!("localhost");
            self.params["mysql"]["port"] = json!(3307);
            self.create_mysql_pool();
        }

        let pool = match &self.pool {
            Some(pool) => pool,
            None => {
                println!("{settings:?}");
                panic!("Can't establish DB connection!");
            },
        };
        pool.prep_exec(r#"UPDATE `batch` SET `status`='TODO' WHERE status='RUNNING'"#, ())
            .expect("SourceMD::init: Resetting old running batches to TODO has failed");
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
                println!("LOGIN {lgname}/{lgpass}");
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

    #[tokio::test]
    async fn restart_batch_without_pool_returns_none() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.restart_batch(1).is_none());
    }

    #[tokio::test]
    async fn get_next_batch_without_pool_returns_none() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.get_next_batch().await.is_none());
    }

    #[tokio::test]
    async fn deactivate_batch_run_without_pool_returns_none() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.deactivate_batch_run(1).await.is_none());
    }

    #[tokio::test]
    async fn set_batch_finished_without_pool_returns_none() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.set_batch_finished(1).is_none());
    }

    #[tokio::test]
    async fn check_batch_not_stopped_without_pool_errors() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.check_batch_not_stopped(1).is_err());
    }

    #[tokio::test]
    async fn get_next_command_without_pool_returns_none() {
        let mock_server = start_mock_server().await;
        let smd = make_sourcemd(&mock_server).await;
        assert!(smd.get_next_command(1).is_none());
    }

    #[tokio::test]
    async fn set_command_status_without_pool_returns_none() {
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
        assert!(smd.set_command_status(&mut cmd, "RUNNING", None).is_none());
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
