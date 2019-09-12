extern crate config;
extern crate mediawiki;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
extern crate serde_json;

use crate::sourcemd_command::SourceMDcommand;
use crate::wikidata_string_cache::WikidataStringCache;
use mediawiki::api::Api;
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
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

const INI_FILE: &str = "bot.ini";

fn command_authors(ini_file: &str) {
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file)));
    let mw_api = smd.read().unwrap().mw_api();
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
        author_from_id(&line, cache.clone(), smd.clone());
    }
}

fn author_from_id(id: &String, cache: Arc<WikidataStringCache>, smd: Arc<RwLock<SourceMD>>) {
    let mut command = SourceMDcommand::new_dummy("DUMMY", id);
    let bot = SourceMDbot::new(smd.clone(), cache.clone(), 0).unwrap();
    bot.process_author_metadata(&mut command).unwrap();
}

fn command_papers(ini_file: &str) {
    let mw_api = Arc::new(RwLock::new(SourceMD::create_mw_api(ini_file).unwrap()));
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
        paper_from_id(&line, mw_api.clone());
    }
}

fn paper_from_id(id: &String, mw_api: Arc<RwLock<Api>>) {
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
    wdp.testing = true;
    wdp.add_adapter(Box::new(PMC2Wikidata::new()));
    wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));

    match RE_WD.captures(&id) {
        Some(caps) => match caps.get(1) {
            Some(q) => {
                match wdp.create_or_update_item_from_q(mw_api, &q.as_str().to_string()) {
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
            None => {}
        },
        None => {}
    }

    let mut ids = vec![];
    match RE_DOI.captures(&id) {
        Some(caps) => match caps.get(1) {
            Some(id) => ids.push(GenericWorkIdentifier::new_prop(PROP_DOI, id.as_str())),
            None => {}
        },
        None => {}
    };
    match RE_PMID.captures(&id) {
        Some(caps) => match caps.get(1) {
            Some(id) => ids.push(GenericWorkIdentifier::new_prop(PROP_PMID, id.as_str())),
            None => {}
        },
        None => {}
    };
    match RE_PMCID.captures(&id) {
        Some(caps) => match caps.get(1) {
            Some(id) => ids.push(GenericWorkIdentifier::new_prop(PROP_PMCID, id.as_str())),
            None => {}
        },
        None => {}
    };

    // Paranoia
    ids.retain(|id| !id.id.is_empty());

    if ids.len() == 0 {
        println!("Can't find a valid ID in '{}'", &id);
        return;
    }
    //println!("IDs: {:?}", &ids);
    ids = wdp.update_from_paper_ids(&ids);

    match wdp.create_or_update_item_from_ids(mw_api, &ids) {
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
fn run_bot(config: Arc<RwLock<SourceMD>>, cache: Arc<WikidataStringCache>) -> bool {
    //println!("BOT!");
    let batch_id = match config.read().unwrap().get_next_batch() {
        Some(n) => n,
        None => return false, // Nothing to do
    };

    println!("SPAWN: Starting batch #{}", batch_id);
    let bot = match SourceMDbot::new(config.clone(), cache.clone(), batch_id) {
        Ok(bot) => bot,
        Err(error) => {
            println!(
                "Error when starting bot for batch #{}: '{}'",
                &batch_id, &error
            );
            config.read().unwrap().set_batch_failed(batch_id);
            return false;
        }
    };

    println!("Batch #{} spawned", batch_id);
    thread::spawn(move || while bot.run().unwrap_or(false) {});
    true
}
fn command_bot(ini_file: &str) {
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file)));
    let mw_api = Arc::new(RwLock::new(SourceMD::create_mw_api(ini_file).unwrap()));
    let cache = Arc::new(WikidataStringCache::new(mw_api));
    loop {
        //println!("BOT!");
        if run_bot(smd.clone(), cache.clone()) {
            thread::sleep(Duration::from_millis(1000));
        } else {
            thread::sleep(Duration::from_millis(5000));
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        return;
    }
    match args[1].as_str() {
        "papers" => command_papers(INI_FILE),
        "authors" => command_authors(INI_FILE),
        "bot" => command_bot(INI_FILE),
        _ => usage(&args[0]),
    }
}
