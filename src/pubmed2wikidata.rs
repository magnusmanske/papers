//extern crate chrono;
extern crate config;
extern crate mediawiki;
extern crate serde_json;

//use crate::AuthorItemInfo;
use crate::ScientificPublicationAdapter;
//use chrono::prelude::*;
use pubmed::*;
use std::collections::HashMap;
use wikibase::*;

#[derive(Debug, Clone)]
pub struct Pubmed2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PubmedArticle>,
    client: Client,
}

impl Pubmed2Wikidata {
    pub fn new() -> Self {
        Pubmed2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&PubmedArticle> {
        self.work_cache.get(publication_id)
    }
}

impl ScientificPublicationAdapter for Pubmed2Wikidata {
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
        let query = "".to_string() + &doi + "";
        let work_ids = match self.client.article_ids_from_query(&query, 10) {
            Ok(work_ids) => work_ids,
            _ => return None, // No such work
        };
        if work_ids.len() != 1 {
            return None;
        }
        let publication_id = work_ids[0];
        let work = self.client.article(publication_id).unwrap();

        self.work_cache.insert(publication_id.to_string(), work);
        Some(publication_id.to_string())
    }

    fn get_work_issn(&self, publication_id: &String) -> Option<String> {
        let work = self
            .get_cached_publication_from_id(publication_id)?
            .to_owned();
        let medline_citation = work.medline_citation?.to_owned();
        let article = medline_citation.article?.to_owned();
        let journal = article.journal?.to_owned();
        let issn = journal.issn?.to_owned();
        Some(issn)
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
