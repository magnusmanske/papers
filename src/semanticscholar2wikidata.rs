use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use semanticscholar::*;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct Semanticscholar2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, Work>,
    client: Client,
}

impl Default for Semanticscholar2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Semanticscholar2Wikidata {
    pub fn new() -> Self {
        Semanticscholar2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&Work> {
        self.work_cache.get(publication_id)
    }

    fn publication_ids_from_doi(&mut self, doi: &str) -> Vec<String> {
        let work = match self.client.work(doi) {
            Ok(w) => w,
            _ => return vec![], // No such work
        };

        let publication_id = match &work.paper_id {
            Some(paper_id) => paper_id.to_string(),
            None => return vec![], // No ID
        };

        self.work_cache.insert(publication_id.clone(), work);
        vec![publication_id]
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let my_prop = match self.publication_property() {
            Some(prop) => prop,
            None => return,
        };

        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        ret.push(GenericWorkIdentifier::new_prop(my_prop, publication_id));

        match &work.doi {
            Some(id) => {
                ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id));
            }
            None => {}
        }

        /*
        This works, but might somehow merge separate items for "reviewed publication" and arxiv version
        match &work.arxiv_id {
            Some(id) => {
                ret.push(GenericWorkIdentifier {
                    work_type: GenericWorkType::Property(PROP_ARXIV.to_string()),
                    id: id.clone(),
                });
            }
            None => {}
        }
        */
    }
}

#[async_trait]
impl ScientificPublicationAdapter for Semanticscholar2Wikidata {
    fn name(&self) -> &str {
        "Semanticscholar2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        Some("P4012".to_string())
    }

    fn publication_property(&self) -> Option<IdProp> {
        Some(IdProp::SematicScholar)
    }

    /*
    // TODO load direct from SS via own ID
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let publication_id = match self
            .get_external_identifier_from_item(item, &self.publication_property().unwrap())
        {
            Some(s) => s,
            None => return None,
        };
        self.publication_id_from_pubmed(&publication_id)
    }
    */

    fn topic_property(&self) -> Option<String> {
        Some("P6611".to_string())
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn get_identifier_list(&mut self, ids: &[GenericWorkIdentifier]) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            if let GenericWorkType::Property(prop) = id.work_type() {
                if *prop == IdProp::DOI {
                    for publication_id in self.publication_ids_from_doi(id.id()) {
                        self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                    }
                }
            }
        }
        ret
    }

    fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
        let work = match self.client.work(publication_id) {
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

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.title {
                Some(title) => vec![LocaleString::new("en", title)],
                None => vec![],
            },
            None => vec![],
        }
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        if !item.has_claims_with_property("P577") {
            if let Some(year) = work.year {
                let statement =
                    self.get_wb_time_from_partial("P577".to_string(), year as u32, None, None);
                item.add_claim(statement);
            }
        }
    }

    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let mut ret: Vec<GenericAuthorInfo> = vec![];
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return ret,
        };

        let author_property = match self.author_property() {
            Some(p) => p,
            None => return ret,
        };

        for num in 0..work.authors.len() {
            let author = &work.authors[num];
            let mut entry = GenericAuthorInfo {
                name: author.name.clone(),
                prop2id: HashMap::new(),
                wikidata_item: None,
                list_number: Some((num + 1).to_string()),
                alternative_names: vec![],
            };
            match &author.author_id {
                Some(id) => {
                    entry
                        .prop2id
                        .insert(author_property.to_owned(), id.to_string());
                }
                None => {}
            }
            ret.push(entry);
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
    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&Work> {
    fn publication_ids_from_doi(&mut self, doi: &str) -> Vec<String> {
    fn add_identifiers_from_cached_publication(
    fn name(&self) -> &str {
    fn author_property(&self) -> Option<String> {
    fn publication_property(&self) -> Option<String> {
    fn topic_property(&self) -> Option<String> {
    fn author_cache(&self) -> &HashMap<String, String> {
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
    fn get_identifier_list(
    fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
    fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
    */
}
