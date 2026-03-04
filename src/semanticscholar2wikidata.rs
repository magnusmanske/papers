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

    async fn publication_ids_from_doi(&mut self, doi: &str) -> Vec<String> {
        let work = match self.client.work(doi).await {
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

        if let Some(id) = &work.doi {
            ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id));
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

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Semanticscholar2Wikidata {
    fn name(&self) -> &str {
        "Semanticscholar2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        Some("P4012".to_string())
    }

    fn publication_property(&self) -> Option<IdProp> {
        Some(IdProp::SemanticScholar)
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

    async fn get_identifier_list(
        &mut self,
        ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            if let GenericWorkType::Property(prop) = id.work_type() {
                if *prop == IdProp::DOI {
                    for publication_id in self.publication_ids_from_doi(id.id()).await {
                        self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                    }
                }
            }
        }
        ret
    }

    async fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
        let work = match self.client.work(publication_id).await {
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

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let mut ret: Vec<GenericAuthorInfo> = vec![];
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return ret,
        };

        let author_property = match self.author_property() {
            Some(p) => p,
            None => return ret,
        };

        for (num, author) in work.authors.iter().enumerate() {
            let mut entry = GenericAuthorInfo {
                name: author.name.clone(),
                prop2id: HashMap::new(),
                wikidata_item: None,
                list_number: Some((num + 1).to_string()),
                alternative_names: vec![],
            };
            if let Some(id) = &author.author_id {
                entry
                    .prop2id
                    .insert(author_property.to_owned(), id.to_string());
            }
            ret.push(entry);
        }

        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_work(title: Option<&str>, doi: Option<&str>, paper_id: Option<&str>) -> Work {
        Work {
            arxiv_id: None,
            authors: vec![],
            citation_velocity: None,
            citations: vec![],
            doi: doi.map(|s| s.to_string()),
            influential_citation_count: None,
            paper_id: paper_id.map(|s| s.to_string()),
            references: vec![],
            title: title.map(|s| s.to_string()),
            topics: vec![],
            url: None,
            venue: None,
            year: None,
        }
    }

    fn make_ss(id: &str, work: Work) -> Semanticscholar2Wikidata {
        let mut ss = Semanticscholar2Wikidata::new();
        ss.work_cache.insert(id.to_string(), work);
        ss
    }

    // === property getters ===

    #[test]
    fn name_returns_expected_string() {
        let ss = Semanticscholar2Wikidata::new();
        assert_eq!(ss.name(), "Semanticscholar2Wikidata");
    }

    #[test]
    fn author_property_returns_p4012() {
        let ss = Semanticscholar2Wikidata::new();
        assert_eq!(ss.author_property(), Some("P4012".to_string()));
    }

    #[test]
    fn publication_property_returns_semantic_scholar() {
        let ss = Semanticscholar2Wikidata::new();
        assert_eq!(ss.publication_property(), Some(IdProp::SemanticScholar));
    }

    #[test]
    fn topic_property_returns_p6611() {
        let ss = Semanticscholar2Wikidata::new();
        assert_eq!(ss.topic_property(), Some("P6611".to_string()));
    }

    // === get_work_titles ===

    #[test]
    fn get_work_titles_returns_title() {
        let ss = make_ss("abc123", make_work(Some("A paper title"), None, None));
        let titles = ss.get_work_titles("abc123");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "A paper title");
        assert_eq!(titles[0].language(), "en");
    }

    #[test]
    fn get_work_titles_returns_empty_when_no_title() {
        let ss = make_ss("abc123", make_work(None, None, None));
        assert!(ss.get_work_titles("abc123").is_empty());
    }

    #[test]
    fn get_work_titles_returns_empty_for_missing_publication() {
        let ss = Semanticscholar2Wikidata::new();
        assert!(ss.get_work_titles("nonexistent").is_empty());
    }

    // === add_identifiers_from_cached_publication ===

    #[test]
    fn add_identifiers_includes_semantic_scholar_id() {
        let mut ss = make_ss("abc123", make_work(None, None, None));
        let mut ret = vec![];
        ss.add_identifiers_from_cached_publication("abc123", &mut ret);
        assert!(ret
            .iter()
            .any(|id| id.work_type() == &identifiers::GenericWorkType::Property(IdProp::SemanticScholar)
                && id.id() == "abc123"));
    }

    #[test]
    fn add_identifiers_includes_doi_when_present() {
        let mut ss = make_ss("abc123", make_work(None, Some("10.1234/test"), None));
        let mut ret = vec![];
        ss.add_identifiers_from_cached_publication("abc123", &mut ret);
        assert!(ret
            .iter()
            .any(|id| id.work_type() == &identifiers::GenericWorkType::Property(IdProp::DOI)));
    }

    #[test]
    fn add_identifiers_no_doi_when_absent() {
        let mut ss = make_ss("abc123", make_work(None, None, None));
        let mut ret = vec![];
        ss.add_identifiers_from_cached_publication("abc123", &mut ret);
        assert!(!ret
            .iter()
            .any(|id| id.work_type() == &identifiers::GenericWorkType::Property(IdProp::DOI)));
    }

    #[test]
    fn add_identifiers_does_nothing_for_missing_publication() {
        let mut ss = Semanticscholar2Wikidata::new();
        let mut ret = vec![];
        ss.add_identifiers_from_cached_publication("nonexistent", &mut ret);
        assert!(ret.is_empty());
    }

    // === get_author_list ===

    #[tokio::test]
    async fn get_author_list_returns_authors_with_names() {
        let work = Work {
            authors: vec![
                Author {
                    author_id: Some("auth1".to_string()),
                    name: Some("Alice Smith".to_string()),
                    url: None,
                },
                Author {
                    author_id: None,
                    name: Some("Bob Jones".to_string()),
                    url: None,
                },
            ],
            ..make_work(None, None, None)
        };
        let mut ss = make_ss("abc123", work);
        let authors = ss.get_author_list("abc123").await;
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name, Some("Alice Smith".to_string()));
        assert_eq!(authors[0].list_number, Some("1".to_string()));
        assert_eq!(authors[1].name, Some("Bob Jones".to_string()));
        assert_eq!(authors[1].list_number, Some("2".to_string()));
    }

    #[tokio::test]
    async fn get_author_list_includes_author_id_in_prop2id() {
        let work = Work {
            authors: vec![Author {
                author_id: Some("ss_auth_42".to_string()),
                name: Some("Carol White".to_string()),
                url: None,
            }],
            ..make_work(None, None, None)
        };
        let mut ss = make_ss("abc123", work);
        let authors = ss.get_author_list("abc123").await;
        assert_eq!(authors.len(), 1);
        assert_eq!(
            authors[0].prop2id.get("P4012"),
            Some(&"ss_auth_42".to_string())
        );
    }

    #[tokio::test]
    async fn get_author_list_empty_for_missing_publication() {
        let mut ss = Semanticscholar2Wikidata::new();
        assert!(ss.get_author_list("nonexistent").await.is_empty());
    }
}
