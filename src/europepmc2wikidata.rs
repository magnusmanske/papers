use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};
use crate::{
    adapter_helpers::get_external_identifier_from_item,
    generic_author_info::GenericAuthorInfo,
    http_client::{HttpJsonFetcher, JsonFetcher},
    scientific_publication_adapter::ScientificPublicationAdapter,
    *,
};

pub struct EuropePMC2Wikidata {
    fetcher: Arc<dyn JsonFetcher>,
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, serde_json::Value>,
}

impl Default for EuropePMC2Wikidata {
    fn default() -> Self {
        Self::new(Arc::new(HttpJsonFetcher::default()))
    }
}

impl EuropePMC2Wikidata {
    /// New adapter with the given JSON fetcher. Production callers pass
    /// `Arc::new(HttpJsonFetcher::default())`; tests pass an
    /// `Arc::new(MockJsonFetcher::new())`.
    pub fn new(fetcher: Arc<dyn JsonFetcher>) -> Self {
        EuropePMC2Wikidata {
            fetcher,
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

    async fn fetch_doi_data(&self, doi: &str) -> Option<(String, serde_json::Value)> {
        let url = format!(
            "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:{}&resulttype=core&format=json",
            doi
        );
        let json = self.fetcher.fetch_json(&url).await?;
        let results = json["resultList"]["result"].as_array()?;
        let work = results.first()?.clone();
        Some((doi.to_uppercase(), work))
    }

    async fn fetch_work_by_doi(&mut self, doi: &str) -> Option<String> {
        let (pub_id, work) = self.fetch_doi_data(doi).await?;
        self.work_cache.insert(pub_id.clone(), work);
        Some(pub_id)
    }

}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for EuropePMC2Wikidata {
    fn name(&self) -> &str {
        "EuropePMC2Wikidata"
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn has_cached_publication(&self, publication_id: &str) -> bool {
        self.get_cached_publication_from_id(publication_id).is_some()
    }

    fn extract_extra_ids(&self, publication_id: &str) -> Vec<GenericWorkIdentifier> {
        let Some(work) = self.get_cached_publication_from_id(publication_id) else {
            return vec![];
        };
        let mut extras = Vec::new();
        if let Some(doi) = work["doi"].as_str().filter(|s| !s.is_empty()) {
            extras.push(GenericWorkIdentifier::new_prop(IdProp::DOI, doi));
        }
        if let Some(pmid) = work["pmid"].as_str().filter(|s| !s.is_empty()) {
            extras.push(GenericWorkIdentifier::new_prop(IdProp::PMID, pmid));
        }
        if let Some(pmcid) = work["pmcid"].as_str().filter(|s| !s.is_empty()) {
            extras.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, pmcid));
        }
        extras
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
        // Build the per-DOI futures with shared & borrows of self, await
        // them all together, then drop the futures (releasing the borrow)
        // before mutating self.work_cache below.
        let results: Vec<_> = {
            let futures = dois.iter().map(|doi| self.fetch_doi_data(doi));
            futures::future::join_all(futures).await
        };
        for (pub_id, work) in results.into_iter().flatten() {
            self.work_cache.insert(pub_id, work);
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
        let doi = get_external_identifier_from_item(item, &IdProp::DOI)?;
        self.fetch_work_by_doi(&doi).await
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match work["title"].as_str() {
                Some(title) if !title.is_empty() => vec![LocaleString::new("en", title)],
                _ => vec![],
            },
            None => vec![],
        }
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let work = self.get_cached_publication_from_id(publication_id)?;

        // Try firstPublicationDate (format: "2023-06-15")
        if let Some(date_str) = work["firstPublicationDate"].as_str() {
            let parts: Vec<&str> = date_str.split('-').collect();
            if let Some(year) = parts.first().and_then(|s| s.parse::<u32>().ok()) {
                let month: Option<u8> = parts.get(1).and_then(|s| s.parse().ok());
                let day: Option<u8> = parts.get(2).and_then(|s| s.parse().ok());
                return Some((year, month, day));
            }
        }

        // Fall back to journalInfo
        let year = work["journalInfo"]["yearOfPublication"].as_u64()? as u32;
        let month = work["journalInfo"]["monthOfPublication"].as_u64().map(|x| x as u8);
        let day = work["journalInfo"]["dayOfPublication"].as_u64().map(|x| x as u8);
        Some((year, month, day))
    }

    fn get_volume(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["volume"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_issue(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["issue"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["journal"]["issn"]
            .as_str()
            .map(|s| s.to_string())
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return vec![],
        };
        match work["authorList"]["author"].as_array() {
            Some(authors) => authors
                .iter()
                .filter_map(|author| author["fullName"].as_str())
                .enumerate()
                .map(|(num, name)| GenericAuthorInfo::new_from_name_num(name, num + 1))
                .collect(),
            None => vec![],
        }
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

    fn make_epmc_work() -> serde_json::Value {
        json!({
            "doi": "10.1234/test",
            "pmid": "12345678",
            "pmcid": "PMC9876543",
            "title": "Test Article Title",
            "firstPublicationDate": "2023-06-15",
            "journalInfo": {
                "volume": "42",
                "issue": "3",
                "yearOfPublication": 2023,
                "monthOfPublication": 6,
                "journal": {
                    "issn": "1234-5678",
                    "title": "Test Journal"
                }
            },
            "authorList": {
                "author": [
                    {"fullName": "Alice Smith", "firstName": "Alice", "lastName": "Smith"},
                    {"fullName": "Bob Jones", "firstName": "Bob", "lastName": "Jones"}
                ]
            }
        })
    }

    #[test]
    fn test_name() {
        let adapter = EuropePMC2Wikidata::default();
        assert_eq!(adapter.name(), "EuropePMC2Wikidata");
    }

    #[test]
    fn test_get_work_titles() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        let titles = adapter.get_work_titles("10.1234/TEST");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "Test Article Title");
    }

    #[test]
    fn test_get_work_titles_missing() {
        let adapter = EuropePMC2Wikidata::default();
        assert!(adapter.get_work_titles("nonexistent").is_empty());
    }

    #[test]
    fn test_get_work_titles_empty_title() {
        let mut adapter = EuropePMC2Wikidata::default();
        let mut work = make_epmc_work();
        work["title"] = json!("");
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        assert!(adapter.get_work_titles("10.1234/TEST").is_empty());
    }

    #[test]
    fn test_get_publication_date_from_first_publication_date() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        assert_eq!(adapter.get_publication_date("10.1234/TEST"), Some((2023, Some(6), Some(15))));
    }

    #[test]
    fn test_get_publication_date_fallback_to_journal_info() {
        let mut adapter = EuropePMC2Wikidata::default();
        let mut work = make_epmc_work();
        work["firstPublicationDate"] = json!(null);
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        assert_eq!(adapter.get_publication_date("10.1234/TEST"), Some((2023, Some(6), None)));
    }

    #[test]
    fn test_get_volume_and_issue() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        assert_eq!(adapter.get_volume("10.1234/TEST"), Some("42".to_string()));
        assert_eq!(adapter.get_issue("10.1234/TEST"), Some("3".to_string()));
    }

    #[test]
    fn test_get_work_issn() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        assert_eq!(adapter.get_work_issn("10.1234/TEST"), Some("1234-5678".to_string()));
    }

    #[tokio::test]
    async fn test_get_author_list() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        let authors = adapter.get_author_list("10.1234/TEST").await;
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name(), Some("Alice Smith"));
        assert_eq!(authors[0].list_number(), Some("1"));
        assert_eq!(authors[1].name(), Some("Bob Jones"));
        assert_eq!(authors[1].list_number(), Some("2"));
    }

    #[tokio::test]
    async fn test_get_author_list_empty() {
        let mut adapter = EuropePMC2Wikidata::default();
        let mut work = make_epmc_work();
        work["authorList"]["author"] = json!([]);
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        assert!(adapter.get_author_list("10.1234/TEST").await.is_empty());
    }

    #[tokio::test]
    async fn test_get_author_list_missing() {
        let mut adapter = EuropePMC2Wikidata::default();
        assert!(adapter.get_author_list("nonexistent").await.is_empty());
    }

    #[test]
    fn test_add_identifiers_from_cached_publication() {
        let mut adapter = EuropePMC2Wikidata::default();
        adapter.work_cache.insert("10.1234/TEST".to_string(), make_epmc_work());
        let mut ret = vec![];
        adapter.add_identifiers_from_cached_publication("10.1234/TEST", &mut ret);
        assert!(ret.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::DOI)
            && id.id() == "10.1234/TEST"));
        assert!(ret.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::PMID)
            && id.id() == "12345678"));
        assert!(ret.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::PMCID)
            && id.id() == "PMC9876543"));
    }

    #[test]
    fn test_add_identifiers_partial() {
        let mut adapter = EuropePMC2Wikidata::default();
        let mut work = make_epmc_work();
        work["pmcid"] = json!(null);
        adapter.work_cache.insert("10.1234/TEST".to_string(), work);
        let mut ret = vec![];
        adapter.add_identifiers_from_cached_publication("10.1234/TEST", &mut ret);
        assert_eq!(ret.len(), 2); // DOI + PMID, no PMCID
        assert!(!ret.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::PMCID)));
    }

    // === HTTP-injected tests (P2-10) =======================================

    use crate::http_client::MockJsonFetcher;

    #[tokio::test]
    async fn fetch_work_by_doi_hits_expected_url_and_caches() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:10.1234/test&resulttype=core&format=json";
        fetcher.add_response(
            url,
            json!({"resultList": {"result": [
                {"doi": "10.1234/test", "title": "Test"}
            ]}}),
        );
        let mut adapter = EuropePMC2Wikidata::new(fetcher.clone());

        // Note: fetch_work_by_doi (and its callers) feed the *original*
        // DOI in unchanged; the cache key is the uppercased form because
        // fetch_doi_data returns `doi.to_uppercase()`.
        let pub_id = adapter.fetch_work_by_doi("10.1234/test").await;
        assert_eq!(pub_id, Some("10.1234/TEST".to_string()));
        assert_eq!(fetcher.captured_urls(), vec![url.to_string()]);
        assert!(adapter.get_cached_publication_from_id("10.1234/TEST").is_some());
    }

    #[tokio::test]
    async fn fetch_work_by_doi_returns_none_on_fetch_failure() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:10.1234/test&resulttype=core&format=json";
        fetcher.add_failure(url);
        let mut adapter = EuropePMC2Wikidata::new(fetcher);
        assert!(adapter.fetch_work_by_doi("10.1234/test").await.is_none());
    }

    #[tokio::test]
    async fn fetch_work_by_doi_returns_none_on_empty_results() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:10.1234/missing&resulttype=core&format=json";
        fetcher.add_response(url, json!({"resultList": {"result": []}}));
        let mut adapter = EuropePMC2Wikidata::new(fetcher);
        assert!(adapter.fetch_work_by_doi("10.1234/missing").await.is_none());
    }

    #[tokio::test]
    async fn get_identifier_list_fetches_each_doi_and_merges_ids() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        // GenericWorkIdentifier::new_prop uppercases DOIs on construction,
        // so the upstream URLs use the uppercase form.
        let url_a = "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:10.1/A&resulttype=core&format=json";
        let url_b = "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=DOI:10.1/B&resulttype=core&format=json";
        fetcher.add_response(
            url_a,
            json!({"resultList": {"result": [
                {"doi": "10.1/A", "pmid": "111"}
            ]}}),
        );
        fetcher.add_response(
            url_b,
            json!({"resultList": {"result": [
                {"doi": "10.1/B", "pmcid": "PMC222"}
            ]}}),
        );
        let mut adapter = EuropePMC2Wikidata::new(fetcher.clone());

        let inputs = vec![
            GenericWorkIdentifier::new_prop(IdProp::DOI, "10.1/a"),
            GenericWorkIdentifier::new_prop(IdProp::DOI, "10.1/b"),
        ];
        let out = adapter.get_identifier_list(&inputs).await;

        let captured = fetcher.captured_urls();
        assert!(
            captured.iter().any(|u| u.contains("DOI:10.1/A")),
            "expected DOI:10.1/A in captured URLs, got {captured:?}"
        );
        assert!(
            captured.iter().any(|u| u.contains("DOI:10.1/B")),
            "expected DOI:10.1/B in captured URLs, got {captured:?}"
        );

        // Output contains the PMID/PMCID we mocked.
        assert!(out.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::PMID)
            && id.id() == "111"));
        assert!(out.iter().any(|id| *id.work_type() == GenericWorkType::Property(IdProp::PMCID)
            && id.id() == "PMC222"));
    }
}
