extern crate config;
extern crate mediawiki;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
//#[macro_use]
extern crate serde_json;

use crate::wikidata_string_cache::WikidataStringCache;
use mediawiki::api::Api;
use papers::crossref2wikidata::Crossref2Wikidata;
use papers::orcid2wikidata::Orcid2Wikidata;
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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn command_papers(mw_api: &mut Api) {
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
        paper_from_id(&line, mw_api);
    }
}

fn paper_from_id(id: &String, mut mw_api: &mut Api) {
    lazy_static! {
        static ref RE_WD: Regex =
            Regex::new(r#"^(Q\d+)$"#).expect("main.rs::paper_from_id: RE_WD does not compile");
        static ref RE_DOI: Regex =
            Regex::new(r#"^(.+/.+)$"#).expect("main.rs::paper_from_id: RE_DOI does not compile");
        static ref RE_PMID: Regex =
            Regex::new(r#"^(\d+)$"#).expect("main.rs::paper_from_id: RE_PMID does not compile");
        static ref RE_PMCID: Regex = Regex::new(r#"^PMCID(\d+)$"#)
            .expect("main.rs::paper_from_id: RE_PMCID does not compile");
    }

    let api = Api::new("https://www.wikidata.org/w/api.php")
        .expect("main.rs::paper_from_id: cannot get Wikidata API");
    let cache = Arc::new(Mutex::new(WikidataStringCache::new(&api)));

    let mut wdp = WikidataPapers::new(cache.clone());
    wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));

    match RE_WD.captures(&id) {
        Some(caps) => match caps.get(1) {
            Some(q) => {
                match wdp.create_or_update_item_from_q(&mut mw_api, &q.as_str().to_string()) {
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

    match wdp.create_or_update_item_from_ids(&mut mw_api, &ids) {
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

fn run_bot(config_arc: Arc<Mutex<SourceMD>>, cache: Arc<Mutex<WikidataStringCache>>) {
    //println!("BOT!");
    let batch_id: i64;
    {
        let config = config_arc.lock().unwrap();
        batch_id = match config.get_next_batch() {
            Some(n) => n,
            None => return, // Nothing to do
        };
    }
    thread::spawn(move || {
        println!("SPAWN: Starting batch {}", &batch_id);
        let mut bot = match SourceMDbot::new(config_arc.clone(), cache.clone(), batch_id) {
            Ok(bot) => bot,
            Err(error) => {
                println!(
                    "Error when starting bot for batch #{}: '{}'",
                    &batch_id, &error
                );
                // TODO mark this as problematic so it doesn't get run again next time?
                return;
            }
        };
        while bot.run().unwrap_or(false) {}
    });
}
fn command_bot() {
    let smd = Arc::new(Mutex::new(SourceMD::new()));
    let api = Api::new("https://www.wikidata.org/w/api.php")
        .expect("main.rs::command_bot: cannot get Wikidata API");
    let cache = Arc::new(Mutex::new(WikidataStringCache::new(&api)));
    loop {
        //println!("BOT!");
        run_bot(smd.clone(), cache.clone());
        thread::sleep(Duration::from_millis(5000));
    }
}

fn main() {
    let mut mw_api = SourceMDbot::get_mw_api("bot.ini").unwrap();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        return;
    }
    match args[1].as_str() {
        "papers" => command_papers(&mut mw_api),
        "bot" => command_bot(),
        _ => usage(&args[0]),
    }
}
