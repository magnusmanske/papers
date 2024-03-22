use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use orcid::*;
use std::collections::HashMap;

use self::identifiers::IdProp;

#[derive(Debug, Clone, Default)]
pub struct PseudoWork {
    pub author_ids: Vec<String>,
}

impl PseudoWork {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone)]
pub struct Orcid2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PseudoWork>,
    client: Client,
    author_data: HashMap<String, Option<Author>>,
}

impl Default for Orcid2Wikidata {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&PseudoWork> {
        self.work_cache.get(publication_id)
    }

    pub fn get_or_load_author_data(&mut self, orcid_author_id: &str) -> Option<Author> {
        if !self.author_data.contains_key(orcid_author_id) {
            match self.client.author(&orcid_author_id.to_string()) {
                Ok(data) => self
                    .author_data
                    .insert(orcid_author_id.to_string(), Some(data)),
                Err(_) => self.author_data.insert(orcid_author_id.to_string(), None),
            };
        }
        match self.author_data.get(orcid_author_id) {
            Some(ret) => ret.to_owned(),
            None => None,
        }
    }
}

#[async_trait]
impl ScientificPublicationAdapter for Orcid2Wikidata {
    fn name(&self) -> &str {
        "Orcid2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        Some("P496".to_string())
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        // TODO other ID types than DOI?
        let doi = match self.get_external_identifier_from_item(item, IdProp::DOI.as_str()) {
            Some(s) => s,
            None => return None,
        };
        let author_ids = match self.client.search_doi(&doi) {
            Ok(author_ids) => author_ids,
            _ => return None, // No such work
        };

        let work = PseudoWork { author_ids };
        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }

    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let mut ret: Vec<GenericAuthorInfo> = vec![];
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return ret,
        };
        let author_property = match self.author_property() {
            Some(p) => p,
            None => return vec![],
        };

        for num in 0..work.author_ids.len() {
            let orcid_author_id = &work.author_ids[num];
            if let Some(author) = self.get_or_load_author_data(orcid_author_id) {
                //println!("\n{}\n\n", author.json());
                let mut gai = GenericAuthorInfo {
                    name: None,
                    prop2id: HashMap::new(),
                    wikidata_item: None,
                    list_number: None,
                    alternative_names: vec![],
                };
                match author.credit_name() {
                    Some(name) => gai.name = Some(name.to_string()),
                    None => {
                        let j = author.json();
                        let last_name = j["person"]["name"]["family-name"]["value"].as_str();
                        let given_names = j["person"]["name"]["given-names"]["value"].as_str();
                        match (given_names, last_name) {
                            (Some(f), Some(l)) => gai.name = Some(format!("{} {}", &f, &l)),
                            (None, Some(l)) => gai.name = Some(l.to_string()),
                            _ => {}
                        }
                    }
                }
                if let Some(id) = author.orcid_id() {
                    gai.prop2id
                        .insert(author_property.to_string(), id.to_string());
                }
                let ext_ids = author.external_ids();
                for id in ext_ids {
                    match id.0.as_str() {
                        "ResearcherID" => {
                            gai.prop2id.insert("P1053".to_string(), id.1);
                        }
                        "Researcher ID" => {
                            gai.prop2id.insert("P1053".to_string(), id.1);
                        }
                        "Scopus Author ID" => {
                            gai.prop2id.insert("P1153".to_string(), id.1);
                        }
                        "Scopus ID" => {
                            gai.prop2id.insert("P1153".to_string(), id.1);
                        }
                        "Loop profile" => {
                            gai.prop2id.insert("P2798".to_string(), id.1);
                        }
                        other => {
                            println!("orcid2wikidata: Unknown ID '{}':'{}'", &other, &id.1);
                        }
                    }
                }
                ret.push(gai);
            }
        }

        ret
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:

    pub fn new() -> Self {
    pub fn new() -> Self {
    pub fn get_cached_publication_from_id(&self, publication_id: &String) -> Option<&PseudoWork> {
    pub fn get_or_load_author_data(&mut self, orcid_author_id: &String) -> Option<Author> {
    fn name(&self) -> &str {
    fn author_property(&self) -> Option<String> {
    fn author_cache(&self) -> &HashMap<String, String> {
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
    fn update_statements_for_publication_id(&self, publication_id: &String, _item: &mut Entity) {
    fn get_author_list(&mut self, publication_id: &String) -> Vec<GenericAuthorInfo> {
    */
}
