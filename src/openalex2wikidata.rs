use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct OpenAlex2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, serde_json::Value>,
}

impl Default for OpenAlex2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAlex2Wikidata {
    pub fn new() -> Self {
        OpenAlex2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &str,
    ) -> Option<&serde_json::Value> {
        self.work_cache.get(publication_id)
    }

    async fn fetch_doi_data(doi: &str) -> Option<(String, serde_json::Value)> {
        let url = format!("https://api.openalex.org/works/doi:{}", doi);
        let json: serde_json::Value = reqwest::get(&url).await.ok()?.json().await.ok()?;
        Some((doi.to_uppercase(), json))
    }

    async fn fetch_work_by_doi(&mut self, doi: &str) -> Option<String> {
        let (pub_id, json) = Self::fetch_doi_data(doi).await?;
        self.work_cache.insert(pub_id.clone(), json);
        Some(pub_id)
    }

    fn add_identifiers_from_cached_publication(
        &self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // DOI
        if let Some(doi) = work["doi"].as_str() {
            // OpenAlex returns DOI as full URL like "https://doi.org/10.1234/..."
            let doi = doi.strip_prefix("https://doi.org/").unwrap_or(doi);
            ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, doi));
        }

        // PMID
        if let Some(pmid_url) = work["ids"]["pmid"].as_str() {
            // Format: "https://pubmed.ncbi.nlm.nih.gov/12345678"
            if let Some(pmid) = pmid_url.rsplit('/').next() {
                if !pmid.is_empty() {
                    ret.push(GenericWorkIdentifier::new_prop(IdProp::PMID, pmid));
                }
            }
        }

        // PMCID
        if let Some(pmcid_url) = work["ids"]["pmcid"].as_str() {
            // Format: "https://www.ncbi.nlm.nih.gov/pmc/articles/PMC1234567"
            if let Some(pmcid) = pmcid_url.rsplit('/').next() {
                if !pmcid.is_empty() {
                    ret.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, pmcid));
                }
            }
        }
    }

    /// Maps an OpenAlex type_crossref value to a Wikidata Q-item.
    fn openalex_type_to_q(type_crossref: &str) -> Option<&'static str> {
        match type_crossref {
            "journal-article" => Some("Q13442814"),
            "book" | "edited-book" | "reference-book" => Some("Q571"),
            "monograph" => Some("Q193495"),
            "book-chapter" | "book-section" => Some("Q1980247"),
            "proceedings-article" => Some("Q23927052"),
            "proceedings" => Some("Q1143604"),
            "dissertation" => Some("Q187685"),
            "posted-content" => Some("Q580922"),
            "dataset" => Some("Q1172284"),
            "report" | "report-series" => Some("Q10870555"),
            "standard" => Some("Q317623"),
            "peer-review" => Some("Q7161778"),
            _ => None,
        }
    }

    /// Parses a date string like "2023-01-15" into (year, month, day).
    fn parse_date(date_str: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let parts: Vec<&str> = date_str.split('-').collect();
        let year: u32 = parts.first()?.parse().ok()?;
        let month: Option<u8> = parts.get(1).and_then(|s| s.parse().ok());
        let day: Option<u8> = parts.get(2).and_then(|s| s.parse().ok());
        Some((year, month, day))
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for OpenAlex2Wikidata {
    fn name(&self) -> &str {
        "OpenAlex2Wikidata"
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
        let dois: Vec<&str> = ids
            .iter()
            .filter_map(|id| {
                if let GenericWorkType::Property(prop) = id.work_type() {
                    if *prop == IdProp::DOI {
                        return Some(id.id());
                    }
                }
                None
            })
            .collect();
        let futures: Vec<_> = dois.iter().map(|doi| Self::fetch_doi_data(doi)).collect();
        for result in futures::future::join_all(futures).await.into_iter().flatten() {
            let (pub_id, json) = result;
            self.work_cache.insert(pub_id, json);
        }
        let mut ret = vec![];
        for doi in &dois {
            let pub_id = doi.to_uppercase();
            if self.work_cache.contains_key(&pub_id) {
                self.add_identifiers_from_cached_publication(&pub_id, &mut ret);
            }
        }
        ret
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let doi = self.get_external_identifier_from_item(item, &IdProp::DOI)?;
        self.fetch_work_by_doi(&doi).await
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => {
                if let Some(title) = work["display_name"].as_str() {
                    if !title.is_empty() {
                        return vec![LocaleString::new("en", title)];
                    }
                }
                if let Some(title) = work["title"].as_str() {
                    if !title.is_empty() {
                        return vec![LocaleString::new("en", title)];
                    }
                }
                vec![]
            }
            None => vec![],
        }
    }

    fn get_work_type(&self, publication_id: &str) -> Option<String> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        let type_crossref = work["type_crossref"].as_str()?;
        Self::openalex_type_to_q(type_crossref).map(|s| s.to_string())
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        let date_str = work["publication_date"].as_str()?;
        Self::parse_date(date_str)
    }

    fn get_volume(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["biblio"]["volume"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_issue(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["biblio"]["issue"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        // Primary location's source has ISSN
        work["primary_location"]["source"]["issn_l"]
            .as_str()
            .map(|s| s.to_string())
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return vec![],
        };
        let authorships = match work["authorships"].as_array() {
            Some(a) => a,
            None => return vec![],
        };
        authorships
            .iter()
            .enumerate()
            .filter_map(|(num, authorship)| {
                let name = authorship["author"]["display_name"].as_str()?;
                let mut entry = GenericAuthorInfo::new_from_name_num(name, num + 1);
                // Try to extract ORCID
                if let Some(orcid_url) = authorship["author"]["orcid"].as_str() {
                    // Format: "https://orcid.org/0000-0001-2345-6789"
                    if let Some(orcid) = orcid_url.rsplit('/').next() {
                        if !orcid.is_empty() {
                            entry.prop2id.insert("P496".to_string(), orcid.to_string());
                        }
                    }
                }
                Some(entry)
            })
            .collect()
    }

    async fn update_statements_for_publication_id(
        &self,
        _publication_id: &str,
        _item: &mut Entity,
    ) {
        // No extra statements beyond the defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_work() -> serde_json::Value {
        json!({
            "doi": "https://doi.org/10.1234/TEST",
            "display_name": "Test Paper Title",
            "title": "Test Paper Title",
            "type_crossref": "journal-article",
            "publication_date": "2023-06-15",
            "ids": {
                "doi": "https://doi.org/10.1234/TEST",
                "pmid": "https://pubmed.ncbi.nlm.nih.gov/12345678",
                "pmcid": "https://www.ncbi.nlm.nih.gov/pmc/articles/PMC9876543"
            },
            "biblio": {
                "volume": "42",
                "issue": "3",
                "first_page": "100",
                "last_page": "110"
            },
            "primary_location": {
                "source": {
                    "issn_l": "1234-5678"
                }
            },
            "authorships": [
                {
                    "author": {
                        "display_name": "Alice Smith",
                        "orcid": "https://orcid.org/0000-0001-2345-6789"
                    },
                    "author_position": "first"
                },
                {
                    "author": {
                        "display_name": "Bob Jones",
                        "orcid": null
                    },
                    "author_position": "last"
                }
            ]
        })
    }

    #[test]
    fn test_name() {
        let adapter = OpenAlex2Wikidata::new();
        assert_eq!(adapter.name(), "OpenAlex2Wikidata");
    }

    #[test]
    fn test_get_work_titles() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        let titles = adapter.get_work_titles("10.1234/TEST");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "Test Paper Title");
    }

    #[test]
    fn test_get_work_titles_missing() {
        let adapter = OpenAlex2Wikidata::new();
        assert!(adapter.get_work_titles("nonexistent").is_empty());
    }

    #[test]
    fn test_get_work_titles_fallback_to_title() {
        let mut adapter = OpenAlex2Wikidata::new();
        let mut work = make_work();
        work["display_name"] = json!(null);
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        let titles = adapter.get_work_titles("10.1234/TEST");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "Test Paper Title");
    }

    #[test]
    fn test_get_work_type() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        assert_eq!(
            adapter.get_work_type("10.1234/TEST"),
            Some("Q13442814".to_string())
        );
    }

    #[test]
    fn test_get_work_type_book() {
        let mut adapter = OpenAlex2Wikidata::new();
        let mut work = make_work();
        work["type_crossref"] = json!("book");
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        assert_eq!(
            adapter.get_work_type("10.1234/TEST"),
            Some("Q571".to_string())
        );
    }

    #[test]
    fn test_get_publication_date() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        assert_eq!(
            adapter.get_publication_date("10.1234/TEST"),
            Some((2023, Some(6), Some(15)))
        );
    }

    #[test]
    fn test_get_volume_and_issue() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        assert_eq!(adapter.get_volume("10.1234/TEST"), Some("42".to_string()));
        assert_eq!(adapter.get_issue("10.1234/TEST"), Some("3".to_string()));
    }

    #[test]
    fn test_get_work_issn() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        assert_eq!(
            adapter.get_work_issn("10.1234/TEST"),
            Some("1234-5678".to_string())
        );
    }

    #[tokio::test]
    async fn test_get_author_list() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        let authors = adapter.get_author_list("10.1234/TEST").await;
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name, Some("Alice Smith".to_string()));
        assert_eq!(authors[0].list_number, Some("1".to_string()));
        assert_eq!(
            authors[0].prop2id.get("P496"),
            Some(&"0000-0001-2345-6789".to_string())
        );
        assert_eq!(authors[1].name, Some("Bob Jones".to_string()));
        assert_eq!(authors[1].list_number, Some("2".to_string()));
        assert!(!authors[1].prop2id.contains_key("P496"));
    }

    #[tokio::test]
    async fn test_get_author_list_empty() {
        let mut adapter = OpenAlex2Wikidata::new();
        let mut work = make_work();
        work["authorships"] = json!([]);
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        assert!(adapter.get_author_list("10.1234/TEST").await.is_empty());
    }

    #[test]
    fn test_add_identifiers_from_cached_publication() {
        let mut adapter = OpenAlex2Wikidata::new();
        adapter
            .work_cache
            .insert("10.1234/TEST".to_string(), make_work());
        let mut ret = vec![];
        adapter.add_identifiers_from_cached_publication("10.1234/TEST", &mut ret);
        assert!(ret.iter().any(
            |id| *id.work_type() == GenericWorkType::Property(IdProp::DOI)
                && id.id() == "10.1234/TEST"
        ));
        assert!(ret.iter().any(
            |id| *id.work_type() == GenericWorkType::Property(IdProp::PMID)
                && id.id() == "12345678"
        ));
        assert!(ret.iter().any(
            |id| *id.work_type() == GenericWorkType::Property(IdProp::PMCID)
                && id.id() == "PMC9876543"
        ));
    }

    #[test]
    fn test_parse_date_full() {
        assert_eq!(
            OpenAlex2Wikidata::parse_date("2023-06-15"),
            Some((2023, Some(6), Some(15)))
        );
    }

    #[test]
    fn test_parse_date_year_only() {
        assert_eq!(
            OpenAlex2Wikidata::parse_date("2023"),
            Some((2023, None, None))
        );
    }

    #[test]
    fn test_parse_date_invalid() {
        assert_eq!(OpenAlex2Wikidata::parse_date(""), None);
    }
}
