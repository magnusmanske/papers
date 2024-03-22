extern crate config;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
extern crate serde_json;

use crate::sourcemd_command::SourceMDcommand;
use crate::wikidata_string_cache::WikidataStringCache;
use papers::crossref2wikidata::Crossref2Wikidata;
use papers::orcid2wikidata::Orcid2Wikidata;
use papers::pmc2wikidata::PMC2Wikidata;
use papers::pubmed2wikidata::Pubmed2Wikidata;
use papers::semanticscholar2wikidata::Semanticscholar2Wikidata;
use papers::sourcemd_bot::SourceMDbot;
use papers::sourcemd_config::SourceMD;
use papers::wikidata_papers::WikidataPapers;
use papers::*;
use regex::Regex;
use std::env;
use std::io;
use std::io::prelude::*;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

const INI_FILE: &str = "bot.ini";

async fn command_authors(ini_file: &str) {
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file).await.unwrap()));
    let mw_api = smd.read().await.mw_api();
    let cache = Arc::new(WikidataStringCache::new(mw_api.clone()));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        println!("Processing {}", &line);
        author_from_id(&line, cache.clone(), smd.clone()).await;
    }
}

async fn author_from_id(id: &str, cache: Arc<WikidataStringCache>, smd: Arc<RwLock<SourceMD>>) {
    let mut command = SourceMDcommand::new_dummy("DUMMY", id);
    let bot = SourceMDbot::new(smd.clone(), cache.clone(), 0)
        .await
        .unwrap();
    bot.process_author_metadata(&mut command).await.unwrap();
}

async fn command_papers(ini_file: &str) {
    let mw_api = Arc::new(RwLock::new(
        SourceMD::create_mw_api(ini_file).await.unwrap(),
    ));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        //println!("Processing {}", &line);
        paper_from_id(&line, mw_api.clone()).await;
    }
}

async fn paper_from_id(id: &String, mw_api: Arc<RwLock<Api>>) {
    lazy_static! {
        static ref RE_WD: Regex =
            Regex::new(r#"^(Q\d+)$"#).expect("main.rs::paper_from_id: RE_WD does not compile");
        static ref RE_DOI: Regex =
            Regex::new(r#"^(.+/.+)$"#).expect("main.rs::paper_from_id: RE_DOI does not compile");
        static ref RE_PMID: Regex =
            Regex::new(r#"^(\d+)$"#).expect("main.rs::paper_from_id: RE_PMID does not compile");
        static ref RE_PMCID: Regex =
            Regex::new(r#"^(PMC\d+)$"#).expect("main.rs::paper_from_id: RE_PMCID does not compile");
    }

    let cache = Arc::new(WikidataStringCache::new(mw_api.clone()));

    let mut wdp = WikidataPapers::new(cache.clone());
    //wdp.testing = true;
    wdp.add_adapter(Box::new(PMC2Wikidata::new()));
    wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));

    if let Some(caps) = RE_WD.captures(id) {
        if let Some(q) = caps.get(1) {
            match wdp
                .create_or_update_item_from_q(mw_api, &q.as_str().to_string())
                .await
            {
                Some(er) => {
                    if er.edited {
                        println!("Created or updated https://www.wikidata.org/wiki/{}", &er.q)
                    } else {
                        println!("https://www.wikidata.org/wiki/{}, no changes ", &er.q)
                    }
                }
                None => println!("No item ID!"),
            }
            return;
        }
    }

    let mut ids = vec![];
    if let Some(caps) = RE_DOI.captures(id) {
        if let Some(id) = caps.get(1) {
            ids.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id.as_str()))
        }
    };
    if let Some(caps) = RE_PMID.captures(id) {
        if let Some(id) = caps.get(1) {
            ids.push(GenericWorkIdentifier::new_prop(IdProp::PMID, id.as_str()))
        }
    };
    if let Some(caps) = RE_PMCID.captures(id) {
        if let Some(id) = caps.get(1) {
            ids.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, id.as_str()))
        }
    };

    // Paranoia
    ids.retain(|id| !id.id().is_empty());

    if ids.is_empty() {
        println!("Can't find a valid ID in '{}'", &id);
        return;
    }
    //println!("IDs: {:?}", &ids);
    ids = wdp.update_from_paper_ids(&ids);

    match wdp.create_or_update_item_from_ids(mw_api, &ids).await {
        Some(er) => {
            if er.edited {
                println!("Created or updated https://www.wikidata.org/wiki/{}", &er.q)
            } else {
                println!(
                    "Exists as https://www.wikidata.org/wiki/{}, no changes ",
                    &er.q
                )
            }
        }
        None => println!("No item ID for '{}'!", &id),
    }
}

fn usage(command_name: &String) {
    println!("USAGE: {} [papers]", command_name);
}

/// Returns true if a new batch was started, false otherwise
async fn run_bot(config: Arc<RwLock<SourceMD>>, cache: Arc<WikidataStringCache>) -> bool {
    //println!("BOT!");
    let batch_id = match config.read().await.get_next_batch().await {
        Some(n) => n,
        None => return false, // Nothing to do
    };

    println!("SPAWN: Starting batch #{}", batch_id);
    let bot = match SourceMDbot::new(config.clone(), cache.clone(), batch_id).await {
        Ok(bot) => bot,
        Err(error) => {
            println!(
                "Error when starting bot for batch #{}: '{}'",
                &batch_id, &error
            );
            config.read().await.set_batch_failed(batch_id).await;
            return false;
        }
    };

    println!("Batch #{} spawned", batch_id);
    // tokio::spawn(async move { while bot.run().await.unwrap_or(false) {} });
    while bot.run().await.unwrap_or(false) {}
    true
}

async fn command_bot(ini_file: &str) {
    println!("== STARTING BOT MODE");
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file).await.unwrap()));
    let mw_api = Arc::new(RwLock::new(
        SourceMD::create_mw_api(ini_file).await.unwrap(),
    ));
    let cache = Arc::new(WikidataStringCache::new(mw_api));
    loop {
        //println!("BOT!");
        if run_bot(smd.clone(), cache.clone()).await {
            thread::sleep(Duration::from_millis(1000));
        } else {
            thread::sleep(Duration::from_millis(5000));
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        return;
    }
    match args[1].as_str() {
        "papers" => command_papers(INI_FILE).await,
        "authors" => command_authors(INI_FILE).await,
        "bot" => command_bot(INI_FILE).await,
        _ => usage(&args[0]),
    }
}
