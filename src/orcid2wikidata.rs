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
        o.author_cache_mut().insert("0000-0001-2345-6789".to_string(), "Q42".to_string());
        assert_eq!(o.author_cache().get("0000-0001-2345-6789"), Some(&"Q42".to_string()));
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
        work.author_ids.push("0000-0001-2345-6789".to_string());
        o.work_cache.insert("10.0/test".to_string(), work);

        let got = o.get_cached_publication_from_id("10.0/test").expect("should be cached");
        assert_eq!(got.author_ids, vec!["0000-0001-2345-6789".to_string()]);
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
}
