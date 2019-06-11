extern crate config;
extern crate lazy_static;
extern crate mediawiki;
extern crate regex;
//#[macro_use]
extern crate serde_json;

use config::{Config, File};
use mysql as my;
use serde_json::Value;
use std::collections::HashSet;
/*
use crate::*;
use regex::Regex;
use std::env;
use std::io;
use std::io::prelude::*;
use crate::crossref2wikidata::Crossref2Wikidata;
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::wikidata_papers::WikidataPapers;
*/

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

    pub fn get_next_batch(&self) -> Option<i64> {
        let pool = match &self.pool {
            Some(pool) => pool,
            None => return None,
        };

        let sql: String =
            "SELECT * FROM batch WHERE `status` ='TODO' ORDER BY `last_action`".into();
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
