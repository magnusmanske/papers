extern crate config;
extern crate mediawiki;
extern crate serde_json;

use crate::AuthorItemInfo;
use crate::ScientificPublicationAdapter;
use semanticscholar::*;
use std::collections::HashMap;
use wikibase::*;

pub struct Semanticscholar2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, Work>,
    client: Client,
}

impl Semanticscholar2Wikidata {
    pub fn new() -> Self {
        Semanticscholar2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &String) -> Option<&Work> {
        self.work_cache.get(publication_id)
    }
    /*
        fn update_author_item(&mut self, author: &Author, author_name: &String, item: &mut Entity) {
            let semanticscholar_author_name: String = author.name.clone().unwrap();
            let author_id = author.author_id.clone().unwrap();
        }
    */
}

impl ScientificPublicationAdapter for Semanticscholar2Wikidata {
    fn name(&self) -> &str {
        "Semanticscholar2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        return Some("P4012".to_string());
    }

    fn publication_property(&self) -> Option<String> {
        return Some("P4011".to_string());
    }

    fn topic_property(&self) -> Option<String> {
        return Some("P6611".to_string());
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
        let work = match self.client.work(&doi) {
            Ok(w) => w,
            _ => return None, // No such work
        };

        let publication_id = match &work.paper_id {
            Some(paper_id) => paper_id.to_string(),
            None => return None, // No ID
        };

        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn get_work_titles(&self, publication_id: &String) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.title {
                Some(title) => dbg!(vec![LocaleString::new("en", &title)]),
                None => vec![],
            },
            None => vec![],
        }
    }

    fn update_statements_for_publication_id(&self, publication_id: &String, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }

    fn author2item(
        &mut self,
        author_name: &String,
        mw_api: &mut mediawiki::api::Api,
        publication_id: Option<&String>,
        item: Option<&mut Entity>,
    ) -> AuthorItemInfo {
        // RETURNS WIKIDATA ITEM ID, CATALOG AUHTOR ID, OR None, DEPENDING ON CONTEXT
        let work: Work;
        match publication_id {
            Some(id) => {
                let publication_id_option = self.get_cached_publication_from_id(id).to_owned();
                work = match publication_id_option {
                    Some(w) => w.clone(),
                    None => return AuthorItemInfo::None,
                };
            }
            None => return AuthorItemInfo::None,
        }

        let author_name = self.sanitize_author_name(author_name);

        let mut candidates: Vec<usize> = vec![];
        for num in 0..work.authors.len() {
            let author = &work.authors[num];
            if None == author.author_id {
                continue;
            }
            let current_author_name = match &author.name {
                Some(s) => s,
                _ => continue,
            };
            if self.author_names_match(&author_name, &current_author_name) {
                candidates.push(num);
            }
        }
        if candidates.len() != 1 {
            return AuthorItemInfo::None;
        }
        let author = &work.authors[candidates[0]];
        let author_id = author.author_id.clone().unwrap();
        match item {
            None => {
                match self.get_author_item_id(&author_id, mw_api) {
                    Some(x) => return AuthorItemInfo::WikidataItem(x), // RETURNS ITEM ID
                    None => return AuthorItemInfo::None,
                }
            }

            Some(item) => {
                let semanticscholar_author_name: String = author.name.clone().unwrap();
                self.update_author_item(
                    &semanticscholar_author_name,
                    &author_id,
                    &author_name,
                    item,
                );
                AuthorItemInfo::CatalogId(author_id) // RETURNS AUTHOR ID
            }
        }
    }
}
