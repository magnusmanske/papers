//extern crate chrono;
extern crate config;
extern crate mediawiki;
extern crate serde_json;

//use crate::AuthorItemInfo;
use crate::ScientificPublicationAdapter;
//use chrono::prelude::*;
use orcid::*;
use std::collections::HashMap;
use wikibase::*;

#[derive(Debug, Clone)]
pub struct PseudoWork {
    pub author_ids: Vec<String>,
}

impl PseudoWork {
    pub fn new() -> Self {
        Self { author_ids: vec![] }
    }
}

#[derive(Debug, Clone)]
pub struct Orcid2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PseudoWork>,
    client: Client,
}

impl Orcid2Wikidata {
    pub fn new() -> Self {
        Orcid2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &String) -> Option<&PseudoWork> {
        self.work_cache.get(publication_id)
    }
}

impl ScientificPublicationAdapter for Orcid2Wikidata {
    fn name(&self) -> &str {
        "Orcid2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        return Some("P496".to_string());
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        // TODO other ID types than DOI?
        let doi = match self.get_external_identifier_from_item(item, "P356") {
            Some(s) => s,
            None => return None,
        };
        let author_ids = match self.client.search_doi(&doi) {
            Ok(author_ids) => author_ids,
            _ => return None, // No such work
        };

        let work = PseudoWork {
            author_ids: author_ids,
        };
        //dbg!(&work);
        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn update_statements_for_publication_id(&self, publication_id: &String, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }
    /*
        fn author2item(
            &mut self,
            author_name: &String,
            mw_api: &mut mediawiki::api::Api,
            publication_id: Option<&String>,
            item: Option<&mut Entity>,
        ) -> AuthorItemInfo {
    */
}
