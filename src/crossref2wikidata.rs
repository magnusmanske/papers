extern crate chrono;
extern crate config;
extern crate mediawiki;
extern crate serde_json;

//use crate::AuthorItemInfo;
use crate::ScientificPublicationAdapter;
use chrono::prelude::*;
use crossref::Crossref;
use std::collections::HashMap;
use wikibase::*;

pub struct Crossref2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, crossref::Work>,
    client: crossref::Crossref,
}

impl Crossref2Wikidata {
    pub fn new() -> Self {
        Crossref2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Crossref::builder().build().unwrap(),
        }
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&crossref::Work> {
        self.work_cache.get(publication_id)
    }
}

impl ScientificPublicationAdapter for Crossref2Wikidata {
    fn get_work_issn(&self, publication_id: &String) -> Option<String> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return None,
        };
        match &work.issn {
            Some(array) => match array.len() {
                0 => None,
                _ => Some(array[0].clone()),
            },
            _ => None,
        }
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

        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn reference(&self) -> Vec<Reference> {
        let now = Utc::now().format("+%Y-%m-%dT%H:%M:%SZ").to_string();
        vec![Reference::new(vec![Snak::new_time("P813", &now, 11)])]
    }

    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        if work.title.len() > 0 {
            match item.label_in_locale("en") {
                Some(_) => {}
                None => item.set_label(LocaleString::new("en", &work.title[0])),
            }
        }

        let string_options = vec![
            ("P433", &work.issue),
            ("P478", &work.volume),
            ("P304", &work.page),
        ];
        for option in string_options {
            if !item.has_claims_with_property(option.0) {
                match option.1 {
                    Some(v) => {
                        item.add_claim(Statement::new_normal(
                            Snak::new_string(option.0, v),
                            vec![],
                            self.reference(),
                        ));
                    }
                    None => {}
                }
            }
        }
    }
}