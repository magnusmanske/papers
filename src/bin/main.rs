extern crate config;
extern crate mediawiki;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
//#[macro_use]
extern crate serde_json;

use config::{Config, File};
use papers::*;
use regex::Regex;
use std::env;
use std::io;
use std::io::prelude::*;
//use multimap::MultiMap;
use papers::crossref2wikidata::Crossref2Wikidata;
use papers::orcid2wikidata::Orcid2Wikidata;
use papers::pubmed2wikidata::Pubmed2Wikidata;
use papers::semanticscholar2wikidata::Semanticscholar2Wikidata;
use papers::wikidata_papers::WikidataPapers;

fn command_papers(mw_api: &mut mediawiki::api::Api) {
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

fn paper_from_id(id: &String, mut mw_api: &mut mediawiki::api::Api) {
    lazy_static! {
        static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#).unwrap();
        static ref RE_DOI: Regex = Regex::new(r#"^(.+/.+)$"#).unwrap();
        static ref RE_PMID: Regex = Regex::new(r#"(\d+)$"#).unwrap();
        static ref RE_PMCID: Regex = Regex::new(r#"PMCID(\d+)$"#).unwrap();
    }

    let mut wdp = WikidataPapers::new();
    wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));

    match RE_WD.captures(&id) {
        Some(caps) => {
            let q = caps.get(1).unwrap().as_str().to_string();
            match wdp.create_or_update_item_from_q(&mut mw_api, &q) {
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
    }

    let mut ids = vec![];
    match RE_DOI.captures(&id) {
        Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
            PROP_DOI,
            caps.get(1).unwrap().as_str(),
        )),
        None => {}
    };
    match RE_PMID.captures(&id) {
        Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
            PROP_PMID,
            caps.get(1).unwrap().as_str(),
        )),
        None => {}
    };
    match RE_PMCID.captures(&id) {
        Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
            PROP_PMCID,
            caps.get(1).unwrap().as_str(),
        )),
        None => {}
    };

    if ids.len() == 0 {
        println!("Can't find a valid ID in '{}'", &id);
        return;
    }
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

fn main() {
    let mut mw_api = mediawiki::api::Api::new("https://www.wikidata.org/w/api.php").unwrap();

    let mut settings = Config::default();
    // File::with_name(..) is shorthand for File::from(Path::new(..))
    settings.merge(File::with_name("test.ini")).unwrap();
    let lgname = settings.get_str("user.user").unwrap();
    let lgpass = settings.get_str("user.pass").unwrap();
    mw_api.login(lgname, lgpass).unwrap();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        return;
    }
    match args[1].as_str() {
        "papers" => command_papers(&mut mw_api),
        _ => usage(&args[0]),
    }
}
