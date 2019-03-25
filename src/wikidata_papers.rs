extern crate config;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;

use crate::ScientificPublicationAdapter;
use crossref::Crossref;
use mediawiki::entity_diff::*;
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

    pub fn update_item_from_adapters(
        &mut self,
        mut item: &mut Entity,
        adapter2work_id: &mut HashMap<usize, String>,
    ) {
        for adapter_id in 0..self.adapters.len() {
            let publication_id = match self.adapters[adapter_id].publication_id_from_item(item) {
                Some(id) => id,
                _ => continue,
            };
            adapter2work_id.insert(adapter_id, publication_id.clone());
            self.adapters[adapter_id]
                .update_statements_for_publication_id(&publication_id, &mut item);
        }
    }

    pub fn update_authors_from_adapters(
        &mut self,
        item: &mut Entity,
        adapter2work_id: &HashMap<usize, String>,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let mut entities = mediawiki::entity_container::EntityContainer::new();
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
            let author_name = match datavalue.value() {
                Value::StringValue(s) => s,
                _ => continue,
            };
            // TODO qualifier
            // TODO copy reference(s)
            let mut author_q: Option<String> = None;
            for adapter_num in 0..self.adapters.len() {
                match self.adapters[adapter_num].author2item(
                    &author_name,
                    mw_api,
                    adapter2work_id.get(&adapter_num),
                    None,
                ) {
                    Some(q) => {
                        author_q = Some(q);
                        break;
                    }
                    None => continue,
                }
            }

            let mut item: Entity;
            let original_item: Entity;
            let target;
            match author_q {
                Some(q) => {
                    if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                        continue;
                    }
                    item = match entities.get_entity(q.clone()) {
                        Some(the_item) => the_item.clone(),
                        None => continue,
                    };
                    original_item = item.clone();
                    target = EditTarget::Entity(q);
                }
                None => {
                    original_item = Entity::new_empty();
                    item = Entity::new_empty();
                    target = EditTarget::New("item".to_string());
                }
            };

            let mut adapter_new_author: HashMap<usize, String> = HashMap::new();
            for adapter_num in 0..self.adapters.len() {
                let res = self.adapters[adapter_num].author2item(
                    &author_name,
                    mw_api,
                    adapter2work_id.get(&adapter_num),
                    Some(&mut item),
                );
                match res {
                    Some(author_id) => adapter_new_author.insert(adapter_num, author_id),
                    None => continue,
                };
            }

            let mut diff_params = EntityDiffParams::none();
            diff_params.labels.add = vec!["*".to_string()];
            diff_params.aliases.add = vec!["*".to_string()];
            diff_params.descriptions.add = vec!["*".to_string()];
            diff_params.claims.add = vec!["*".to_string()];

            let diff = EntityDiff::new(&original_item, &item, &diff_params);
            if diff.is_empty() {
                println!("No change for author");
                continue;
            }
            let new_json = EntityDiff::apply_diff(mw_api, &diff, target).unwrap();
            println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
            let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
            println!("https://www.wikidata.org/wiki/{}", &entity_id);

            for adapter_num in 0..self.adapters.len() {
                match adapter_new_author.get(&adapter_num) {
                    Some(author_id) => {
                        self.adapters[adapter_num].set_author_cache_entry(&author_id, &entity_id);
                    }
                    None => continue,
                }
            }
        }
    }

    fn create_blank_item_for_publication_from_doi(&self, doi: &String) -> Entity {
        let mut item = Entity::new_empty();
        item.add_claim(Statement::new(
            "statement",
            StatementRank::Normal,
            Snak::new(
                "external-id",
                "P356",
                SnakType::Value,
                Some(DataValue::new(
                    DataValueType::StringType,
                    Value::StringValue(doi.clone()),
                )),
            ),
            vec![],
            vec![],
        ));

        item
    }

    pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
        let mut entities = mediawiki::entity_container::EntityContainer::new();

        for doi in dois {
            let mut item;
            let original_item;
            let target;
            match self.get_wikidata_item_for_doi(&mw_api, &doi.to_string()) {
                Some(q) => {
                    if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                        continue;
                    }

                    item = match entities.get_entity(q.clone()) {
                        Some(the_item) => the_item.clone(),
                        None => continue,
                    };
                    original_item = item.clone();
                    target = EditTarget::Entity(q);
                }
                None => {
                    original_item = Entity::new_empty();
                    item = self.create_blank_item_for_publication_from_doi(&doi.to_string());
                    target = EditTarget::New("item".to_string());
                }
            };
            let mut adapter2work_id = HashMap::new();
            self.update_item_from_adapters(&mut item, &mut adapter2work_id);
            self.update_authors_from_adapters(&mut item, &adapter2work_id, mw_api);

            let mut diff_params = EntityDiffParams::none();
            diff_params.labels.add = vec!["*".to_string()];
            diff_params.aliases.add = vec!["*".to_string()];
            diff_params.descriptions.add = vec!["*".to_string()];
            for adapter in &self.adapters {
                match adapter.publication_property() {
                    Some(p) => diff_params.claims.add.push(p),
                    None => {}
                }
            }

            let diff = EntityDiff::new(&original_item, &item, &diff_params);
            if diff.is_empty() {
                println!("No change");
                continue;
            }
            println!("{:?}", &diff);
            let new_json = EntityDiff::apply_diff(mw_api, &diff, target).unwrap();
            //println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
            let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
            println!("https://www.wikidata.org/wiki/{}", &entity_id);
        }
    }

    pub fn _test_crossref() {
        let client = Crossref::builder().build().unwrap();
        let work = client.work("10.1037/0003-066X.59.1.29").unwrap();
        dbg!(work);
    }
}
