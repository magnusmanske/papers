use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use orcid::*;
use tokio::sync::Mutex;

use self::identifiers::IdProp;
use crate::{
    adapter_helpers::get_external_identifier_from_item, generic_author_info::GenericAuthorInfo,
    scientific_publication_adapter::ScientificPublicationAdapter, *,
};

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
    author_data: Arc<Mutex<HashMap<String, Option<Author>>>>,
}

impl Default for Orcid2Wikidata {
    fn default() -> Self {
        // Wire the shared papers reqwest::Client into the SDK so the
        // adapter shares the bot-wide connection pool + UA + timeout.
        let client = Client::new().http_client(crate::http_client::http_client().clone());
        Self::new_with_client(client)
    }
}

impl Orcid2Wikidata {
    pub fn new() -> Self {
        Self::default()
    }

    /// New adapter with a caller-provided SDK client. Tests construct
    /// an `orcid::Client::new().base_url(mock.uri())`; production uses
    /// `Default::default()` to share the bot-wide HTTP client.
    pub fn new_with_client(client: Client) -> Self {
        Orcid2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client,
            author_data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&PseudoWork> {
        self.work_cache.get(publication_id)
    }

    pub async fn get_or_load_author_data(&self, orcid_author_id: &str) -> Option<Author> {
        if !self.author_data.lock().await.contains_key(orcid_author_id) {
            let data = self.client.author(orcid_author_id).await.ok();
            self.author_data.lock().await.insert(orcid_author_id.to_string(), data);
        }
        self.author_data.lock().await.get(orcid_author_id).and_then(|r| r.clone())
    }

    async fn get_author_data(
        &self,
        orcid_author_id: &str,
        author_property: &str,
    ) -> Option<GenericAuthorInfo> {
        if let Some(author) = self.get_or_load_author_data(orcid_author_id).await {
            let mut gai = GenericAuthorInfo::new();
            match author.credit_name() {
                Some(name) => gai.set_name(Some(name.to_string())),
                None => {
                    let j = author.json();
                    let last_name = j["person"]["name"]["family-name"]["value"].as_str();
                    let given_names = j["person"]["name"]["given-names"]["value"].as_str();
                    match (given_names, last_name) {
                        (Some(f), Some(l)) => gai.set_name(Some(format!("{} {}", f, l))),
                        (None, Some(l)) => gai.set_name(Some(l.to_string())),
                        _ => {},
                    }
                },
            }
            if let Some(id) = author.orcid_id() {
                gai.prop2id_mut().insert(author_property.to_string(), id.to_string());
            }
            let ext_ids = author.external_ids();
            for id in ext_ids {
                match id.0.as_str() {
                    "ResearcherID" => {
                        gai.prop2id_mut().insert("P1053".to_string(), id.1);
                    },
                    "Researcher ID" => {
                        gai.prop2id_mut().insert("P1053".to_string(), id.1);
                    },
                    "Scopus Author ID" => {
                        gai.prop2id_mut().insert("P1153".to_string(), id.1);
                    },
                    "Scopus ID" => {
                        gai.prop2id_mut().insert("P1153".to_string(), id.1);
                    },
                    "Loop profile" => {
                        gai.prop2id_mut().insert("P2798".to_string(), id.1);
                    },
                    "SciProfiles" => {
                        gai.prop2id_mut().insert("P8159".to_string(), id.1);
                    },
                    "GitHub" => {
                        gai.prop2id_mut().insert("P2037".to_string(), id.1);
                    },
                    "Ciência ID" => {
                        gai.prop2id_mut().insert("P7893".to_string(), id.1);
                    },
                    // "Researcher Name Resolver ID" => {
                    //     gai.prop2id.insert("P9776".to_string(), id.1);
                    // }
                    "ISNI" => {
                        gai.prop2id_mut().insert("P213".to_string(), id.1.replace("-", ""));
                    },
                    other => {
                        self.warn(&format!("orcid2wikidata: Unknown ID '{}':'{}'", other, id.1));
                    },
                }
            }
            return Some(gai);
        }
        None
    }
}

#[async_trait(?Send)]
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

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        // TODO other ID types than DOI?
        let doi = get_external_identifier_from_item(item, &IdProp::DOI)?;
        let author_ids = self.client.search_doi(&doi).await.ok()?;

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

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return vec![],
        };
        let author_property = match self.author_property() {
            Some(p) => p,
            None => return vec![],
        };

        let mut futures = Vec::new();
        for orcid_author_id in &work.author_ids {
            let future = self.get_author_data(orcid_author_id, &author_property);
            futures.push(future);
        }
        futures::future::join_all(futures).await.into_iter().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudowork_new_has_empty_author_ids() {
        let w = PseudoWork::new();
        assert!(w.author_ids.is_empty());
    }

    #[test]
    fn pseudowork_default_matches_new() {
        let a = PseudoWork::default();
        let b = PseudoWork::new();
        assert_eq!(a.author_ids, b.author_ids);
    }

    #[test]
    fn orcid2wikidata_new_has_empty_caches() {
        let o = Orcid2Wikidata::new();
        assert!(o.author_cache.is_empty());
        assert!(o.work_cache.is_empty());
    }

    #[test]
    fn orcid2wikidata_default_matches_new() {
        let o = Orcid2Wikidata::default();
        assert!(o.author_cache.is_empty());
        assert!(o.work_cache.is_empty());
    }

    #[test]
    fn name_is_orcid2wikidata() {
        let o = Orcid2Wikidata::new();
        assert_eq!(o.name(), "Orcid2Wikidata");
    }

    #[test]
    fn author_property_is_p496() {
        let o = Orcid2Wikidata::new();
        assert_eq!(o.author_property(), Some("P496".to_string()));
    }

    #[test]
    fn author_cache_starts_empty() {
        let o = Orcid2Wikidata::new();
        assert!(o.author_cache().is_empty());
    }

    #[test]
    fn author_cache_mut_allows_insertion() {
        let mut o = Orcid2Wikidata::new();
        o.author_cache_mut().insert("0000-0002-1825-0097".to_string(), "Q42".to_string());
        assert_eq!(o.author_cache().get("0000-0002-1825-0097"), Some(&"Q42".to_string()));
    }

    #[test]
    fn get_cached_publication_from_id_returns_none_for_unknown() {
        let o = Orcid2Wikidata::new();
        assert!(o.get_cached_publication_from_id("10.0/unknown").is_none());
    }

    #[test]
    fn get_cached_publication_from_id_returns_inserted_value() {
        let mut o = Orcid2Wikidata::new();
        let mut work = PseudoWork::new();
        work.author_ids.push("0000-0002-1825-0097".to_string());
        o.work_cache.insert("10.0/test".to_string(), work);

        let got = o.get_cached_publication_from_id("10.0/test").expect("should be cached");
        assert_eq!(got.author_ids, vec!["0000-0002-1825-0097".to_string()]);
    }

    #[tokio::test]
    async fn get_author_list_for_unknown_publication_is_empty() {
        let mut o = Orcid2Wikidata::new();
        let authors = o.get_author_list("nonexistent-publication-id").await;
        assert!(authors.is_empty());
    }

    #[tokio::test]
    async fn update_statements_for_unknown_publication_is_a_noop() {
        let o = Orcid2Wikidata::new();
        let mut item = wikibase::Entity::new_empty_item();
        let before = item.claims().len();
        // Calling on an unknown publication ID should not mutate the item
        o.update_statements_for_publication_id("nonexistent", &mut item).await;
        assert_eq!(item.claims().len(), before);
    }

    // === P2-10b: SDK DI via base_url ======================================

    use wiremock::matchers::method as wm_method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_or_load_author_data_routes_through_injected_base_url() {
        // Valid-looking ORCID id triggers an SDK GET to the injected URL.
        // 500 makes the SDK error; the adapter swallows it and returns None.
        //
        // Note: the ORCID SDK builds URLs as `base_url + query` (no
        // separator), and its default base ends with `/` — so test mocks
        // must include the trailing slash.
        let server = MockServer::start().await;
        Mock::given(wm_method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1..)
            .mount(&server)
            .await;

        let sdk = Client::new().base_url(format!("{}/", server.uri()));
        let adapter = Orcid2Wikidata::new_with_client(sdk);
        let data = adapter.get_or_load_author_data("0000-0002-1825-0097").await;
        assert!(data.is_none(), "expected None on SDK error, got {data:?}");
    }

    #[tokio::test]
    async fn get_or_load_author_data_caches_negative_result() {
        // After a failure, a second call should reuse the cached None
        // without re-hitting the mock — verifies the author_data Mutex
        // cache actually short-circuits.
        let server = MockServer::start().await;
        Mock::given(wm_method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let sdk = Client::new().base_url(format!("{}/", server.uri()));
        let adapter = Orcid2Wikidata::new_with_client(sdk);
        assert!(adapter.get_or_load_author_data("0000-0002-1825-0097").await.is_none());
        assert!(adapter.get_or_load_author_data("0000-0002-1825-0097").await.is_none());
        // .expect(1) asserts the mock was hit exactly once (second call
        // should hit the cache, not the mock).
    }

    // === P2-T4b: external-ID dispatch =====================================
    //
    // get_author_data maps ORCID's external-id-type strings to Wikidata
    // properties. Silent data-corruption risk: if a new ID type is added
    // upstream and we miss it, those IDs vanish. These tests pin the
    // matrix.

    use orcid::Author;

    /// Builds an `orcid::Author` from a list of `(external-id-type,
    /// external-id-value)` pairs, with a fixed ORCID id at
    /// `orcid-identifier.path`. Injected into the adapter's author_data
    /// cache so `get_author_data` doesn't try to hit a real ORCID API.
    fn author_with_external_ids(orcid_id: &str, ext_ids: &[(&str, &str)]) -> Author {
        let ids_json: Vec<serde_json::Value> = ext_ids
            .iter()
            .map(|(t, v)| serde_json::json!({
                "external-id-type": t,
                "external-id-value": v,
            }))
            .collect();
        Author::new_from_json(serde_json::json!({
            "orcid-identifier": { "path": orcid_id },
            "person": {
                "name": { "credit-name": { "value": "Test Name" } },
                "external-identifiers": { "external-identifier": ids_json },
            },
        }))
    }

    /// Builds an adapter with the given ORCID id pre-mapped to the given
    /// constructed Author. Calls into `get_author_data` then skip the
    /// network entirely.
    async fn adapter_with_cached_author(orcid_id: &str, author: Author) -> Orcid2Wikidata {
        let adapter = Orcid2Wikidata::default();
        adapter
            .author_data
            .lock()
            .await
            .insert(orcid_id.to_string(), Some(author));
        adapter
    }

    #[tokio::test]
    async fn get_author_data_dispatches_all_known_external_id_types() {
        // One Author with every known external-id type → verify each
        // ends up under the right Wikidata property.
        let orcid_id = "0000-0002-1825-0097";
        let author = author_with_external_ids(
            orcid_id,
            &[
                ("ResearcherID", "ABC-1234"),
                ("Scopus Author ID", "55555"),
                ("Loop profile", "12345"),
                ("SciProfiles", "sp-67890"),
                ("GitHub", "octocat"),
                ("Ciência ID", "C8F1-A2B3-D4E5"),
            ],
        );
        let adapter = adapter_with_cached_author(orcid_id, author).await;

        let gai = adapter.get_author_data(orcid_id, "P496").await.expect("author data");

        // Self-id: orcid_id under author_property.
        assert_eq!(gai.prop2id().get("P496").map(String::as_str), Some(orcid_id));
        // External IDs:
        assert_eq!(gai.prop2id().get("P1053").map(String::as_str), Some("ABC-1234"));
        assert_eq!(gai.prop2id().get("P1153").map(String::as_str), Some("55555"));
        assert_eq!(gai.prop2id().get("P2798").map(String::as_str), Some("12345"));
        assert_eq!(gai.prop2id().get("P8159").map(String::as_str), Some("sp-67890"));
        assert_eq!(gai.prop2id().get("P2037").map(String::as_str), Some("octocat"));
        assert_eq!(gai.prop2id().get("P7893").map(String::as_str), Some("C8F1-A2B3-D4E5"));
    }

    #[tokio::test]
    async fn get_author_data_handles_researcher_id_alias() {
        // The dispatch matrix accepts both "ResearcherID" and
        // "Researcher ID" (with space) for P1053. Pin both.
        let orcid_id = "0000-0002-1825-0097";
        let author = author_with_external_ids(orcid_id, &[("Researcher ID", "XYZ-9999")]);
        let adapter = adapter_with_cached_author(orcid_id, author).await;
        let gai = adapter.get_author_data(orcid_id, "P496").await.unwrap();
        assert_eq!(gai.prop2id().get("P1053").map(String::as_str), Some("XYZ-9999"));
    }

    #[tokio::test]
    async fn get_author_data_handles_scopus_id_alias() {
        // "Scopus Author ID" and "Scopus ID" both → P1153.
        let orcid_id = "0000-0002-1825-0097";
        let author = author_with_external_ids(orcid_id, &[("Scopus ID", "1111")]);
        let adapter = adapter_with_cached_author(orcid_id, author).await;
        let gai = adapter.get_author_data(orcid_id, "P496").await.unwrap();
        assert_eq!(gai.prop2id().get("P1153").map(String::as_str), Some("1111"));
    }

    #[tokio::test]
    async fn get_author_data_strips_dashes_from_isni() {
        // ISNI is the only ID type that gets a transformation — dashes
        // stripped. P213 expects the bare digit string "0000000123456789",
        // not the upstream "0000-0001-2345-6789".
        let orcid_id = "0000-0002-1825-0097";
        let author =
            author_with_external_ids(orcid_id, &[("ISNI", "0000-0001-2345-6789")]);
        let adapter = adapter_with_cached_author(orcid_id, author).await;
        let gai = adapter.get_author_data(orcid_id, "P496").await.unwrap();
        assert_eq!(
            gai.prop2id().get("P213").map(String::as_str),
            Some("0000000123456789"),
            "ISNI dashes must be stripped before pushing to P213"
        );
    }

    #[tokio::test]
    async fn get_author_data_ignores_unknown_external_id_types() {
        // An unrecognised id-type emits a tracing::warn! but must NOT
        // pollute prop2id — otherwise a typo upstream could create
        // bogus Wikidata statements.
        let orcid_id = "0000-0002-1825-0097";
        let author = author_with_external_ids(
            orcid_id,
            &[("SomeNewIdType", "value-we-do-not-understand")],
        );
        let adapter = adapter_with_cached_author(orcid_id, author).await;
        let gai = adapter.get_author_data(orcid_id, "P496").await.unwrap();
        // Only the orcid_id (P496) should be in prop2id; nothing else.
        assert_eq!(gai.prop2id().len(), 1);
        assert!(gai.prop2id().contains_key("P496"));
    }

    #[tokio::test]
    async fn get_author_data_returns_none_when_author_load_fails() {
        // If get_or_load_author_data returns None (network failure /
        // unknown ORCID), get_author_data must also return None — not
        // a half-populated GenericAuthorInfo.
        let orcid_id = "0000-0002-1825-0097";
        let adapter = Orcid2Wikidata::default();
        adapter
            .author_data
            .lock()
            .await
            .insert(orcid_id.to_string(), None);
        assert!(adapter.get_author_data(orcid_id, "P496").await.is_none());
    }
}
