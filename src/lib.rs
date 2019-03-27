extern crate crossref;
extern crate reqwest;
#[macro_use]
extern crate lazy_static;

//use mediawiki::entity_diff::*;
use regex::Regex;
use std::collections::HashMap;
use wikibase::{Entity, Reference, SnakType, Value};

pub enum AuthorItemInfo {
    WikidataItem(String),
    CatalogId(String),
    None,
}

pub trait ScientificPublicationAdapter {
    fn author_property(&self) -> Option<String> {
        None
    }
    fn publication_property(&self) -> Option<String> {
        None
    }
    fn topic_property(&self) -> Option<String> {
        None
    }
    fn author_cache(&self) -> &HashMap<String, String>;
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String>;
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String>;
    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity);

    // Pre-filled methods
    /*
        fn create_item(&self, item: &Entity, mw_api: &mut mediawiki::api::Api) -> Option<String> {
            let params = EntityDiffParams::all();
            let diff = EntityDiff::new(&Entity::new_empty(), item, &params);
            if diff.is_empty() {
                return None;
            }
            let new_json =
                EntityDiff::apply_diff(mw_api, &diff, EditTarget::New("item".to_string())).unwrap();
            EntityDiff::get_entity_id(&new_json)
        }
    */

    fn reference(&self) -> Vec<Reference> {
        vec![]
    }

    fn get_external_identifier_from_item(&self, item: &Entity, property: &str) -> Option<String> {
        for claim in item.claims() {
            if claim.main_snak().property() == property
                && claim.main_snak().snak_type().to_owned() == SnakType::Value
            {
                match claim.main_snak().data_value() {
                    Some(dv) => {
                        let value = dv.value().clone();
                        match &value {
                            Value::StringValue(s) => return Some(s.to_string()),
                            _ => continue,
                        }
                    }
                    None => continue,
                }
            }
        }
        None
    }

    /// Compares long (3+ characters) name parts and returns true if identical
    fn author_names_match(&self, name1: &str, name2: &str) -> bool {
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
            return !parts1.is_empty() && parts1 == parts2;
        }
        false
    }

    fn set_author_cache_entry(&mut self, catalog_author_id: &String, q: &String) {
        self.author_cache_mut()
            .insert(catalog_author_id.to_string(), q.to_string());
    }

    fn get_author_item_from_cache(&self, catalog_author_id: &String) -> Option<&String> {
        self.author_cache().get(catalog_author_id)
    }

    fn author_cache_is_empty(&self) -> bool {
        self.author_cache().is_empty()
    }

    fn author2item(
        &mut self,
        _author_name: &String,
        _mw_api: &mut mediawiki::api::Api,
        _publication_id: Option<&String>,
        _item: Option<&mut Entity>,
    ) -> AuthorItemInfo {
        AuthorItemInfo::None
    }

    fn get_author_item_id(
        &mut self,
        catalog_author_id: &String,
        mw_api: &mediawiki::api::Api,
    ) -> Option<String> {
        let author_property = match self.author_property() {
            Some(p) => p,
            None => return None,
        };
        // Load all authors from Wikidata, if not done so already
        if self.author_cache_is_empty() {
            let res = mw_api
                .sparql_query(&("SELECT ?q ?id { ?q wdt:".to_owned() + &author_property + " ?id }"))
                .unwrap();

            for b in res["results"]["bindings"].as_array().unwrap() {
                match (b["q"]["value"].as_str(), b["id"]["value"].as_str()) {
                    (Some(entity_url), Some(id)) => {
                        let q = mw_api.extract_entity_from_uri(entity_url).unwrap();
                        self.set_author_cache_entry(&id.to_string(), &q);
                    }
                    _ => {}
                }
            }
        }

        // Now check cache
        match self.get_author_item_from_cache(catalog_author_id) {
            Some(q) => return Some(q.to_string()),
            _ => {}
        }

        // Paranoia check via Wikidata search
        let query: String =
            "haswbstatement:".to_owned() + &author_property + &"=".to_owned() + &catalog_author_id;
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
            Some(items) => {
                if items.len() > 0 {
                    let author_q = items[0]["title"].as_str()?;
                    self.set_author_cache_entry(&query, &author_q.to_string());
                    return Some(author_q.to_string());
                }
            }
            None => {}
        }

        None
    }
}

pub mod crossref2wikidata;
pub mod semanticscholar2wikidata;
pub mod wikidata_papers;
