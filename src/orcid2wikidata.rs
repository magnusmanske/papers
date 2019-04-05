//extern crate chrono;
extern crate config;
extern crate mediawiki;
extern crate serde_json;

use crate::AuthorItemInfo;
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
    author_data: HashMap<String, Option<Author>>,
}

impl Orcid2Wikidata {
    pub fn new() -> Self {
        Orcid2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
            author_data: HashMap::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &String) -> Option<&PseudoWork> {
        self.work_cache.get(publication_id)
    }

    pub fn get_or_load_author_data(&mut self, orcid_author_id: &String) -> &Option<Author> {
        if !self.author_data.contains_key(orcid_author_id) {
            match self.client.author(orcid_author_id) {
                Ok(data) => self
                    .author_data
                    .insert(orcid_author_id.to_string(), Some(data)),
                Err(_) => self.author_data.insert(orcid_author_id.to_string(), None),
            };
        }
        self.author_data.get(orcid_author_id).unwrap()
    }

    fn get_author_name_variations(&self, author: &Author) -> Vec<String> {
        let mut ret: Vec<String> = vec![];

        match author.credit_name() {
            Some(name) => ret.push(name.to_string()),
            None => {}
        }

        match author.json()["person"]["name"].as_object() {
            Some(n) => {
                let family_name = n["family-name"]["value"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let given_names = n["given-names"]["value"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !family_name.is_empty() {
                    if given_names.is_empty() {
                        ret.push(family_name);
                    } else {
                        ret.push(given_names + " " + &family_name);
                        // TODO initials?
                    }
                }
            }
            None => {}
        }

        ret
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

    fn author2item(
        &mut self,
        author_name: &String,
        mw_api: &mut mediawiki::api::Api,
        publication_id: Option<&String>,
        item: Option<&mut Entity>,
    ) -> AuthorItemInfo {
        // RETURNS WIKIDATA ITEM ID, CATALOG AUHTOR ID, OR None, DEPENDING ON CONTEXT
        let work: PseudoWork;
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
        for num in 0..work.author_ids.len() {
            let orcid_author_id = &work.author_ids[num];
            let author = self.get_or_load_author_data(orcid_author_id).to_owned();
            let author = match author {
                Some(author) => author,
                None => continue,
            };
            if self
                .get_author_name_variations(&author)
                .iter()
                .filter(|a| self.author_names_match(&author_name, &a))
                .count()
                > 0
            {
                candidates.push(num);
            }
        }
        if candidates.len() != 1 {
            return AuthorItemInfo::None;
        }

        let author_id = &work.author_ids[candidates[0]];
        let author = self.get_or_load_author_data(&author_id).clone().unwrap();
        let author = self.get_author_name_variations(&author).clone();
        let author = author.first().unwrap();

        match item {
            None => {
                match self.get_author_item_id(&author_id, mw_api) {
                    Some(x) => return AuthorItemInfo::WikidataItem(x), // RETURNS ITEM ID
                    None => return AuthorItemInfo::None,
                }
            }

            Some(item) => {
                self.update_author_item(&author, &author_id, &author_name, item);
                AuthorItemInfo::CatalogId(author_id.to_string()) // RETURNS AUTHOR ID
            }
        }
    }
}
