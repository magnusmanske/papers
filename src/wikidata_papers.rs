extern crate config;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;

use crate::ScientificPublicationAdapter;
use crossref::Crossref;
use regex::Regex;
use std::collections::HashMap;
use wikibase::*;

pub struct WikidataPapers {
    adapters: Vec<Box<ScientificPublicationAdapter>>,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers { adapters: vec![] }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Box<ScientificPublicationAdapter>> {
        &mut self.adapters
    }

    pub fn get_wikidata_item_for_doi(
        &self,
        mw_api: &mediawiki::api::Api,
        doi: &String,
    ) -> Option<String> {
        let sparql = format!(
            "SELECT DISTINCT ?q {{ VALUES ?doi {{ '{}' '{}' '{}' }} . ?q wdt:P356 ?doi }}",
            doi,
            doi.to_uppercase(),
            doi.to_lowercase()
        ); // DOIs in Wikidata can be any upper/lowercase :-(
        let res = match mw_api.sparql_query(&sparql) {
            Ok(res) => res,
            _ => return None,
        };
        let qs = mw_api.entities_from_sparql_result(&res, "q");

        match qs.len() {
            0 => None,
            1 => Some(qs[0].clone()),
            _ => {
                println!(
                    "Multiple Wikidata items for DOI '{}' : {}",
                    &doi,
                    qs.join(", ")
                );
                None
            }
        }
    }

    fn _author_names_match(&self, name1: &str, name2: &str) -> bool {
        lazy_static! {
            static ref RE1: Regex = Regex::new(r"\b(\w{3,})\b").unwrap();
        }
        if RE1.is_match(name1) && RE1.is_match(name2) {
            let mut parts1: Vec<String> = vec![];
            for cap in RE1.captures_iter(name1) {
                parts1.push(cap[1].to_string());
            }
            parts1.sort();
            let mut parts2: Vec<String> = vec![];
            for cap in RE1.captures_iter(name2) {
                parts2.push(cap[1].to_string());
            }
            parts2.sort();
            return parts1 == parts2;
        }
        false
    }

    fn _try_wikidata_edit(
        &self,
        mw_api: &mut mediawiki::api::Api,
        item: &mut wikibase::Entity,
        params: &HashMap<&str, &str>,
        num_tries_left: i64,
    ) -> Result<(), Box<::std::error::Error>> {
        let res = mw_api.post_query_api_json(&params).unwrap();

        match res["success"].as_i64() {
            Some(num) => {
                if num == 1 {
                    // Success, now use updated item JSON
                    match &res["entity"] {
                        serde_json::Value::Null => {}
                        entity_json => {
                            //entity_json => entities.set_entity_from_json(&entity_json).unwrap(),
                            let x = from_json::entity_from_json(entity_json).unwrap();
                            *item = x;
                            return Ok(());
                        }
                    };
                }
            }
            None => {}
        }

        if num_tries_left > 0 {
            // TODO sleep 5 sec
            self._try_wikidata_edit(mw_api, item, params, num_tries_left - 1)
        } else {
            Err(From::from(format!(
                "Failed to edit with params '{:?}', result:{:?}",
                &params, &res
            )))
        }
    }

    pub fn update_item_from_adapters(&mut self, mut item: &mut Entity) {
        for adapter in &mut self.adapters {
            let publication_id = match adapter.publication_id_from_item(item) {
                Some(id) => id,
                _ => continue,
            };
            println!(
                "Found publication ID '{}' for item {}",
                &publication_id,
                item.id()
            );
            adapter.update_statements_for_publication_id(&publication_id, &mut item);
        }
    }

    pub fn update_authors_from_adapters(&mut self, item: &mut Entity) {
        // SS authors (P50) match

        // SS authors (P2093) match
        for claim in item.claims_with_property("P2093") {
            if claim.claim_type() != "statement" || claim.main_snak().datatype() != "string" {
                continue;
            }
            let datavalue = match claim.main_snak().data_value() {
                Some(dv) => dv,
                None => continue,
            };
            let _author_name = match datavalue.value() {
                Value::StringValue(s) => s,
                _ => continue,
            };
            /*
                        let mut ss_candidates: Vec<usize> = vec![];
                        for num in 0..ss_work.authors.len() {
                            let ss_author = &ss_work.authors[num];
                            if None == ss_author.author_id {
                                continue;
                            }
                            let ss_author_name = match &ss_author.name {
                                Some(s) => s,
                                _ => continue,
                            };
                            if self.author_names_match(&author_name, &ss_author_name) {
                                ss_candidates.push(num);
                            }
                        }
                        if ss_candidates.len() != 1 {
                            continue;
                        }
                        let ss_author = &ss_work.authors[ss_candidates[0]];
                        let author_q =
                            self.get_or_create_semanticscholar_author_item_id(&ss_author, &author_name, mw_api);
                        match author_q {
                            Some(q) => println!("Found author: https://www.wikidata.org/wiki/{}", &q),
                            None => println!("Found no author '{:?}'", &ss_author),
                        }
            */
        }
    }

    pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
        let mut entities = mediawiki::entity_container::EntityContainer::new();

        for doi in dois {
            let mut item;
            let q;
            match self.get_wikidata_item_for_doi(&mw_api, &doi.to_string()) {
                Some(i) => {
                    q = i;
                    if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                        continue;
                    }

                    let item_opt = entities.get_entity(q.clone());
                    item = match item_opt {
                        Some(the_item) => the_item.clone(),
                        None => continue,
                    };
                }
                None => {
                    // TODO create blank item
                    continue;
                }
            };
            self.update_item_from_adapters(&mut item);
            self.update_authors_from_adapters(&mut item);
        }
    }

    pub fn _test_crossref() {
        let client = Crossref::builder().build().unwrap();
        let work = client.work("10.1037/0003-066X.59.1.29").unwrap();
        dbg!(work);
    }
}
