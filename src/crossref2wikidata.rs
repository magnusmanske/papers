use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use chrono::prelude::*;
use crossref::Crossref;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct Crossref2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, crossref::Work>,
}

impl Default for Crossref2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Crossref2Wikidata {
    pub fn new() -> Self {
        Crossref2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    fn get_client(&self) -> crossref::Crossref {
        Crossref::builder()
            .build()
            .expect("Crossref2Wikidata::new: Could not build Crossref client")
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&crossref::Work> {
        self.work_cache.get(publication_id)
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        if !work.doi.is_empty() {
            //println!("Added DOI {} from CrossRef", &work.doi);
            ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, &work.doi));
        }
    }

    fn should_add_string(&self, s: &str) -> bool {
        if s == "n/a" || s == "n/a-n/a" {
            return false;
        }
        true
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Crossref2Wikidata {
    fn name(&self) -> &str {
        "Crossref2Wikidata"
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.issn {
                Some(array) => match array.len() {
                    0 => None,
                    _ => Some(array[0].clone()),
                },
                None => None,
            },
            None => None,
        }
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    async fn get_identifier_list(
        &mut self,
        ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            if let GenericWorkType::Property(prop) = &id.work_type() {
                if *prop == IdProp::DOI {
                    if let Ok(work) = self.get_client().work(id.id()) {
                        self.work_cache.insert(work.doi.clone(), work.clone());
                        self.add_identifiers_from_cached_publication(&work.doi, &mut ret);
                    }
                }
            }
        }
        ret
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let doi = match self.get_external_identifier_from_item(item, IdProp::DOI.as_str()) {
            Some(s) => s,
            None => return None,
        };
        let work = match self.get_client().work(&doi) {
            Ok(w) => w,
            _ => return None, // No such work
        };

        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn reference(&self) -> Vec<Reference> {
        let now = Utc::now().format("+%Y-%m-%dT00:00:00Z").to_string();
        vec![Reference::new(vec![Snak::new_time("P813", &now, 11)])]
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => work
                .title
                .iter()
                .map(|t| LocaleString::new("en", t))
                .collect(),
            None => vec![],
        }
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // Date
        if !item.has_claims_with_property("P577") {
            let j = json!(work.issued);
            if let Some(dp) = j["date-parts"][0].as_array() {
                if !dp.is_empty() {
                    if let Some(year) = dp[0].as_u64() {
                        let month: Option<u8> = match dp.len() {
                            1 => None,
                            _ => dp[1].as_u64().map(|x| x as u8),
                        };
                        let day: Option<u8> = match dp.len() {
                            3 => dp[2].as_u64().map(|x| x as u8),
                            _ => None,
                        };
                        let statement = self.get_wb_time_from_partial(
                            "P577".to_string(),
                            year as u32,
                            month,
                            day,
                        );
                        item.add_claim(statement);
                    }
                }
            }
        }

        // Issue/volume/page
        let string_options = vec![
            ("P433", &work.issue),
            ("P478", &work.volume),
            ("P304", &work.page),
        ];
        for option in string_options {
            if !item.has_claims_with_property(option.0) {
                match option.1 {
                    Some(v) => {
                        if self.should_add_string(v) {
                            item.add_claim(Statement::new_normal(
                                Snak::new_string(option.0, v),
                                vec![],
                                self.reference(),
                            ));
                        }
                    }
                    None => {}
                }
            }
        }

        match &work.subject {
            Some(subjects) => {
                for _subject in subjects {
                    //println!("Subject:{}", &subject);
                    // TODO
                }
            }
            None => {}
        }

        // TODO journal (already done via ISSN?)
        // TODO ISBN
        // TODO authors
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    pub fn new() -> Self {
    pub fn get_cached_publication_from_id(
    fn add_identifiers_from_cached_publication(
    fn should_add_string(&self, s: &str) -> bool {
    fn name(&self) -> &str {
    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
    fn author_cache(&self) -> &HashMap<String, String> {
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
    fn get_identifier_list(
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
    fn reference(&self) -> Vec<Reference> {
    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
    fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
    */
}
