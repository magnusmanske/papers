extern crate config;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;
extern crate wikibase;

use crate::AuthorItemInfo;
use crate::ScientificPublicationAdapter;
use std::collections::HashMap;
use std::collections::HashSet;
use wikibase::entity_diff::*;
use wikibase::*;

pub struct WikidataPapersCache {
    issn2q: HashMap<String, String>,
    is_initialized: bool,
    mw_api: Option<mediawiki::api::Api>,
}

impl WikidataPapersCache {
    pub fn new() -> Self {
        Self {
            issn2q: HashMap::new(),
            is_initialized: false,
            mw_api: None,
        }
    }

    fn search_issn2q(&mut self, issn: &String) -> Option<String> {
        match self.search_wikibase(&("haswbstatement:P236=".to_string() + issn)) {
            Ok(items) => match items.len() {
                1 => Some(items[0].to_string()),
                _ => None,
            },
            Err(e) => {
                println!("ERROR:{}", e);
                None
            }
        }
    }

    pub fn issn2q(&mut self, issn: &String) -> Option<String> {
        match self.issn2q.get(issn) {
            Some(q) => {
                if q.is_empty() {
                    None
                } else {
                    Some(q.to_string())
                }
            }
            None => match self.search_issn2q(issn) {
                Some(q) => {
                    self.issn2q.insert(issn.to_string(), q.clone());
                    Some(q)
                }
                None => None,
            },
        }
    }

    pub fn search_wikibase(&self, query: &String) -> Result<Vec<String>, String> {
        let mw_api = match &self.mw_api {
            Some(x) => x,
            None => return Err("no mw_api set in WikidataPapersCache".to_string()),
        };
        let params: HashMap<_, _> = vec![
            ("action", "query"),
            ("list", "search"),
            ("srnamespace", "0"),
            ("srsearch", &query.as_str()),
        ]
        .into_iter()
        .map(|(x, y)| (x.to_string(), y.to_string()))
        .collect();
        let res = mw_api.get_query_api_json(&params).unwrap();
        match res["query"]["search"].as_array() {
            Some(items) => Ok(items
                .iter()
                .map(|item| item["title"].as_str().unwrap().to_string())
                .collect()),
            None => Ok(vec![]),
        }
    }

    pub fn init(&mut self, mw_api: &mediawiki::api::Api) {
        if self.is_initialized {
            return;
        }

        self.mw_api = Some(mw_api.clone());

        // DEACTIVATE FOR TESTING
        if false {
            self.init_issn_cache(&mw_api);
        }

        self.is_initialized = true;
    }

    /// Loads all ISSNs from Wikidata via SPARQL
    fn init_issn_cache(&mut self, mw_api: &mediawiki::api::Api) {
        match mw_api.sparql_query("SELECT ?q ?issn { ?q wdt:P236 ?issn }") {
            Ok(sparql_result) => {
                for b in sparql_result["results"]["bindings"].as_array().unwrap() {
                    match b["q"]["value"].as_str() {
                        Some(entity_url) => {
                            let q = mw_api.extract_entity_from_uri(entity_url).unwrap();
                            match b["issn"]["value"].as_str() {
                                Some(issn) => {
                                    if self.issn2q.contains_key(issn) {
                                        self.issn2q.insert(issn.to_string(), "".to_string());
                                    } else {
                                        self.issn2q.insert(issn.to_string(), q);
                                    }
                                }
                                None => {}
                            }
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
        //println!("ISSN cache size: {}", self.issn2q.len());
    }
}

pub struct WikidataPapers {
    adapters: Vec<Box<ScientificPublicationAdapter>>,
    caches: WikidataPapersCache,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers {
            adapters: vec![],
            caches: WikidataPapersCache::new(),
        }
    }

    /*
    pub fn adapters_mut(&mut self) -> &mut Vec<Box<ScientificPublicationAdapter>> {
        &mut self.adapters
    }
    */

    pub fn add_adapter(&mut self, adapter_box: Box<ScientificPublicationAdapter>) {
        self.adapters.push(adapter_box);
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

    /*
        fn try_wikidata_edit(
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
                self.try_wikidata_edit(mw_api, item, params, num_tries_left - 1)
            } else {
                Err(From::from(format!(
                    "Failed to edit with params '{:?}', result:{:?}",
                    &params, &res
                )))
            }
        }
    */
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

            let adapter = &self.adapters[adapter_id];
            adapter2work_id.insert(adapter_id, publication_id.clone());
            println!("Applying adapter {}", adapter.name());

            adapter.update_statements_for_publication_id_default(
                &publication_id,
                &mut item,
                &mut self.caches,
            );
            adapter.update_statements_for_publication_id(&publication_id, &mut item);
            //println!("{}", serde_json::to_string_pretty(&new_item).unwrap());
        }
    }

    pub fn update_authors_from_adapters(
        &mut self,
        item: &mut Entity,
        adapter2work_id: &HashMap<usize, String>,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let mut entities = entity_container::EntityContainer::new();
        let mut claims = item.claims().to_owned();

        // SS authors (P50) match
        let mut p50_authors: HashSet<String> = HashSet::new();
        for claim_num in 0..claims.len() {
            let claim = &claims[claim_num];
            if claim.claim_type() != "statement"
                || *claim.main_snak().datatype() != wikibase::SnakDataType::WikibaseItem
                || claim.main_snak().property() != "P50"
            {
                continue;
            }
            let datavalue = match claim.main_snak().data_value() {
                Some(dv) => dv,
                None => continue,
            };
            match datavalue.value() {
                Value::Entity(entity) => {
                    let q = entity.id();
                    p50_authors.insert(q.into());
                }
                _ => continue,
            }
        }

        // SS authors (P2093) match
        let mut claims_to_replace = vec![];
        for claim_num in 0..claims.len() {
            let claim = &claims[claim_num];
            if claim.claim_type() != "statement"
                || *claim.main_snak().datatype() != wikibase::SnakDataType::String
                || claim.main_snak().property() != "P2093"
            {
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
            let mut author_q: Option<String> = None;
            for adapter_num in 0..self.adapters.len() {
                match self.adapters[adapter_num].author2item(
                    &author_name,
                    mw_api,
                    adapter2work_id.get(&adapter_num),
                    None,
                ) {
                    AuthorItemInfo::WikidataItem(q) => {
                        println!(
                            "{}: {} => {}",
                            self.adapters[adapter_num].name(),
                            &author_name,
                            &q
                        );
                        author_q = Some(q);
                        break;
                    }
                    _ => continue,
                }
            }

            let mut author_item: Entity;
            let original_item: Entity;
            let target;
            match author_q {
                Some(q) => {
                    if p50_authors.contains(&q) {
                        // Paranoia
                        continue;
                    }
                    if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                        continue;
                    }
                    author_item = match entities.get_entity(q.clone()) {
                        Some(the_item) => the_item.clone(),
                        None => continue,
                    };
                    original_item = author_item.clone();
                    target = EditTarget::Entity(q);
                }
                None => {
                    original_item = Entity::new_empty();
                    author_item = Entity::new_empty();
                    target = EditTarget::New("item".to_string());
                }
            };

            let mut adapter_new_author: HashMap<usize, String> = HashMap::new();
            for adapter_num in 0..self.adapters.len() {
                let res = self.adapters[adapter_num].author2item(
                    &author_name,
                    mw_api,
                    adapter2work_id.get(&adapter_num),
                    Some(&mut author_item),
                );
                match res {
                    AuthorItemInfo::CatalogId(author_id) => {
                        adapter_new_author.insert(adapter_num, author_id)
                    }
                    _ => continue,
                };
            }

            let mut diff_params = EntityDiffParams::none();
            diff_params.labels.add = vec!["*".to_string()];
            diff_params.aliases.add = vec!["*".to_string()];
            diff_params.descriptions.add = vec!["*".to_string()];
            diff_params.claims.add = vec!["*".to_string()];

            let diff = EntityDiff::new(&original_item, &author_item, &diff_params);
            if diff.is_empty() {
                if author_item.id().is_empty() {
                    // Diff is empty, no ID => don't bother
                    continue;
                }
                println!(
                    "No change for author '{}' https://www.wikidata.org/wiki/{}",
                    &author_name,
                    author_item.id()
                );
                claims_to_replace.push((claim_num, author_item.id().to_string()));
                continue;
            }
            //println!("{:?}", &diff_params);
            let new_json = EntityDiff::apply_diff(mw_api, &diff, target).unwrap();
            println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
            let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
            println!("https://www.wikidata.org/wiki/{}", &entity_id);

            // Update author caches
            for adapter_num in 0..self.adapters.len() {
                match adapter_new_author.get(&adapter_num) {
                    Some(author_id) => {
                        self.adapters[adapter_num].set_author_cache_entry(&author_id, &entity_id);
                    }
                    None => continue,
                }
            }

            claims_to_replace.push((claim_num, entity_id.to_string()));
        }

        self.replace_author_string_with_author_items(&mut claims_to_replace, &mut claims);
        item.set_claims(claims);
    }

    pub fn replace_author_string_with_author_items(
        &mut self,
        claims_to_replace: &mut std::vec::Vec<(usize, std::string::String)>,
        claims: &mut Vec<Statement>,
    ) {
        // Replace P2093 claims with P50
        if claims_to_replace.is_empty() {
            // Nothing to do
            return;
        }
        while !claims_to_replace.is_empty() {
            let (claim_num, q) = claims_to_replace.pop().unwrap();
            if q.is_empty() {
                continue; // Paranoia
            }
            let claim = claims[claim_num].to_owned();
            let datavalue = match claim.main_snak().data_value() {
                Some(dv) => dv,
                None => continue,
            };
            let author_name = match datavalue.value() {
                Value::StringValue(s) => s,
                _ => continue,
            };
            let mut qualifiers = claim.qualifiers().to_owned();
            let references = claim.references().to_owned();

            // Add original name as qualifier
            // or rather P1810?
            qualifiers.push(Snak::new_string("P1932", author_name));

            let new_claim =
                Statement::new_normal(Snak::new_item("P50", &q), qualifiers, references);

            // Add new claim
            claims.push(new_claim);

            // Remove string claim
            claims.remove(claim_num);
        }
    }

    fn create_blank_item_for_publication_from_doi(&self, doi: &String) -> Entity {
        let mut item = Entity::new_empty();
        item.add_claim(Statement::new_normal(
            Snak::new_external_id("P356", doi),
            vec![],
            vec![],
        ));

        item
    }

    pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
        self.caches.init(mw_api);
        let mut entities = wikibase::entity_container::EntityContainer::new();

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
            diff_params.claims.add.push("P50".to_string());
            diff_params.claims.remove.push("P2093".to_string());

            let diff = EntityDiff::new(&original_item, &item, &diff_params);
            if diff.is_empty() {
                println!("No change");
                continue;
            }
            //println!("{}", diff.to_string_pretty().unwrap());
            if true {
                let new_json = EntityDiff::apply_diff(mw_api, &diff, target).unwrap();
                //println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
                let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
                println!("https://www.wikidata.org/wiki/{}", &entity_id);
            }
        }
    }
}
