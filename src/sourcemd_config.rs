use crate::sourcemd_command::SourceMDcommand;
use chrono::prelude::*;
use config::{Config, File};
use mysql as my;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct SourceMD {
    params: Value,
    running_batch_ids: HashSet<i64>,
    pool: Option<my::Pool>,
}

impl SourceMD {
    pub fn new() -> Self {
        let mut ret = Self {
            params: json!({}),
            running_batch_ids: HashSet::new(),
            pool: None,
        };
        ret.init();
        ret
    }

    pub fn restart_batch(&self, batch_id: i64) -> Option<()> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };
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

    pub fn set_batch_running(&mut self, batch_id: i64) {
        println!("set_batch_running: Starting batch #{}", batch_id);
        self.running_batch_ids.insert(batch_id);
        println!("Currently {} bots running", self.number_of_bots_running());
    }

    pub fn number_of_bots_running(&self) -> usize {
        self.running_batch_ids.len()
    }

    pub fn timestamp(&self) -> String {
        let now = Utc::now();
        now.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    pub fn get_next_batch(&self) -> Option<i64> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };

        let sql: String = r#"SELECT * FROM batch WHERE `status` ='TODO' AND NOT EXISTS (SELECT * FROM command WHERE batch_id=batch.id AND `status` IN ("RUNNING","TODO") AND `mode` NOT IN ("CREATE_PAPER_BY_ID","ADD_AUTHOR_TO_PUBLICATION")) ORDER BY `last_action`"#.into();
        //let sql: String = "SELECT * FROM batch WHERE id=551".into(); // TESTING
        for row in pool.prep_exec(sql, ()).ok()? {
            let row = row.ok()?;
            let id = match &row["id"] {
                my::Value::Int(x) => *x as i64,
                _ => continue,
            };
            if self.running_batch_ids.contains(&id) {
                continue;
            }
            return Some(id);
        }
        None
    }

    pub fn deactivate_batch_run(self: &mut Self, batch_id: i64) -> Option<()> {
        println!("Deactivating batch #{}", batch_id);
        self.running_batch_ids.remove(&batch_id);
        println!("Currently {} bots running", self.number_of_bots_running());
        Some(())
    }

    pub fn set_batch_finished(&mut self, batch_id: i64) -> Option<()> {
        println!("set_batch_finished: Batch #{}", batch_id);
        self.set_batch_status("DONE", batch_id)
    }

    pub fn check_batch_not_stopped(self: &mut Self, batch_id: i64) -> Result<(), String> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => {
                return Err(format!(
                    "QuickStatementsConfig::check_batch_not_stopped: Can't get DB handle"
                ))
            }
        };
        let sql: String = format!(
            "SELECT * FROM batch WHERE id={} AND `status` NOT IN ('RUNNING','TODO')",
            batch_id
        );
        let result = match pool.prep_exec(sql, ()) {
            Ok(r) => r,
            Err(e) => return Err(format!("Error: {}", e)),
        };
        for _row in result {
            return Err(format!(
                "QuickStatementsConfig::check_batch_not_stopped: batch #{} is not RUNNING or TODO",
                batch_id
            ));
        }
        Ok(())
    }

    fn set_batch_status(&mut self, status: &str, batch_id: i64) -> Option<()> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };
        // TODO stats
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`=?,`last_action`=? WHERE id=?"#,
            (
                my::Value::from(status),
                my::Value::from(self.timestamp()),
                my::Value::Int(batch_id),
            ),
        )
        .ok()?;
        self.update_batch_stats(batch_id, pool)?;
        self.deactivate_batch_run(batch_id)
    }

    pub fn get_next_command(&mut self, batch_id: i64) -> Option<SourceMDcommand> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };
        let sql =
            r#"SELECT * FROM command FORCE INDEX (batch_id_4) WHERE `batch_id`=? AND `status`='TODO' ORDER BY `serial_number` LIMIT 1"#;
        for row in pool.prep_exec(sql, (my::Value::Int(batch_id),)).ok()? {
            let row = row.ok()?;
            return Some(SourceMDcommand::new_from_row(row));
        }
        None
    }

    pub fn set_command_status(
        self: &mut Self,
        command: &mut SourceMDcommand,
        new_status: &str,
        new_message: Option<String>,
    ) -> Option<()> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };
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
        let mut j = json!({});
        let sql =
            r#"SELECT `status`,count(*) AS cnt FROM command WHERE batch_id=? GROUP BY `status`"#;
        for row in pool.prep_exec(sql, (my::Value::from(batch_id),)).ok()? {
            let row = row.ok()?;
            let status = match &row["status"] {
                my::Value::Bytes(x) => String::from_utf8_lossy(x),
                _ => continue,
            };
            let cnt = match &row["cnt"] {
                my::Value::Int(x) => *x as i64,
                _ => continue,
            };
            j.as_object_mut()?.insert(status.to_string(), json!(cnt));
        }
        pool.prep_exec(
            r#"UPDATE `batch` SET `overview`=? WHERE `id`=?"#,
            (
                my::Value::from(format!("{}", &j)),
                my::Value::from(batch_id),
            ),
        )
        .ok()?;
        Some(())
    }

    fn init(&mut self) {
        let mut settings = Config::default();
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        let ini_file = "replica.my.ini";
        settings
            .merge(File::with_name(ini_file))
            .expect(format!("Replica file '{}' can't be opened", ini_file).as_str());
        self.params["mysql"]["user"] =
            json!(settings.get_str("client.user").expect("No client.name"));
        self.params["mysql"]["pass"] = json!(settings
            .get_str("client.password")
            .expect("No client.password"));
        self.params["mysql"]["schema"] = json!("s52680__sourcemd_batches_p");

        // On Labs
        self.params["mysql"]["host"] = json!("tools-db");
        self.params["mysql"]["port"] = json!(3306);
        self.create_mysql_pool();

        // Local fallback
        if self.pool.is_none() {
            self.params["mysql"]["host"] = json!("localhost");
            self.params["mysql"]["port"] = json!(3307);
            self.create_mysql_pool();
        }

        if self.pool.is_none() {
            panic!("Can't establish DB connection!");
        }

        let pool = match &self.pool {
            Some(pool) => pool,
            None => panic!("Oh no!"),
        };
        pool.prep_exec(
            r#"UPDATE `batch` SET `status`='TODO' WHERE status='RUNNING'"#,
            (),
        )
        .unwrap();
    }

    fn create_mysql_pool(&mut self) {
        let mut builder = my::OptsBuilder::new();
        //println!("{}", &self.params);
        builder
            .ip_or_hostname(self.params["mysql"]["host"].as_str())
            .db_name(self.params["mysql"]["schema"].as_str())
            .user(self.params["mysql"]["user"].as_str())
            .pass(self.params["mysql"]["pass"].as_str());
        match self.params["mysql"]["port"].as_u64() {
            Some(port) => {
                builder.tcp_port(port as u16);
            }
            None => {}
        }

        // Min 2, max 7 connections
        self.pool = match my::Pool::new_manual(2, 7, builder) {
            Ok(pool) => Some(pool),
            _ => None,
        }
    }
}
