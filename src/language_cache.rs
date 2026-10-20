//! ISO-language-code → Wikidata-Q-item resolver.
//!
//! Looks up the Wikidata Q-item for a language identified by its ISO 639
//! code by issuing a single SPARQL query against the Wikidata Query
//! Service's main graph the first time it's needed, and caching the result
//! for the rest of the process lifetime.
//!
//! Degrades to an empty cache (so `get` returns `None`) on any failure path,
//! rather than panicking. See `audits/STATUS.md` P2-6.

use std::collections::HashMap;

use tokio::sync::OnceCell;
use wikimisc::sparql_client::{SparqlClient, SparqlError};
use wikimisc::sparql_value::SparqlValue;

use crate::wdqs::Wdqs;

/// SPARQL that materialises the language-code → Q-item map. Each row is
/// (ISO code, Wikidata Q-item). One language can have multiple codes
/// (P219 / P220) so we collect them all in a single pass.
const LANGUAGE_SPARQL: &str =
    "SELECT DISTINCT ?l ?q { ?q wdt:P31/wdt:P279* wd:Q20162172; (wdt:P219|wdt:P220) ?l }";

/// Lazily-populated map from language identifier to Wikidata Q-item.
///
/// Construct one per process via [`LanguageCache::wikidata`] or
/// [`LanguageCache::new`] (explicit client, for tests).
pub struct LanguageCache {
    map: OnceCell<HashMap<String, String>>,
    client: SparqlClient,
}

impl LanguageCache {
    /// Build a cache querying through `client`. Lazy: no HTTP is issued
    /// until the first `get` call.
    pub fn new(client: SparqlClient) -> Self {
        Self {
            map: OnceCell::new(),
            client,
        }
    }

    /// Build a cache querying Wikidata's main graph.
    pub fn wikidata() -> Self {
        Self::new(Wdqs::default().main().clone())
    }

    /// Look up the Q-item for a language identifier. Returns `None` if
    /// the language is unknown or if the underlying SPARQL load fails
    /// (the failure is logged and the cache stays empty so the next
    /// call doesn't repeatedly retry).
    pub async fn get(&self, language: &str) -> Option<String> {
        self.map
            .get_or_init(|| self.load())
            .await
            .get(language)
            .cloned()
    }

    async fn load(&self) -> HashMap<String, String> {
        match self.try_load().await {
            Ok(map) => {
                tracing::info!(entries = map.len(), "LanguageCache loaded");
                map
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    endpoint = %self.client.endpoint().url(),
                    "LanguageCache: SPARQL load failed; degrading to empty cache",
                );
                HashMap::new()
            }
        }
    }

    async fn try_load(&self) -> Result<HashMap<String, String>, SparqlError> {
        let result = self.client.query(LANGUAGE_SPARQL).await?;
        Ok(result
            .bindings()
            .iter()
            .filter_map(|row| match (row.get("l")?, row.get("q")?) {
                (SparqlValue::Literal(l), SparqlValue::Entity(q)) => Some((l.clone(), q.clone())),
                _ => None,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wikimisc::sparql_endpoint::SparqlEndpoint;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cache_for(server: &MockServer) -> LanguageCache {
        let wdqs = Wdqs::new(
            SparqlEndpoint::new(format!("{}/sparql", server.uri())),
            SparqlEndpoint::new("http://127.0.0.1:1/unused"),
        );
        LanguageCache::new(wdqs.main().clone())
    }

    #[tokio::test]
    async fn get_resolves_language_codes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "head": {"vars": ["l", "q"]},
                "results": {"bindings": [
                    {"l": {"type": "literal", "value": "en"},
                     "q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q1860"}},
                    {"l": {"type": "literal", "value": "eng"},
                     "q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q1860"}}
                ]}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cache = cache_for(&server);
        assert_eq!(cache.get("en").await.as_deref(), Some("Q1860"));
        assert_eq!(cache.get("eng").await.as_deref(), Some("Q1860"));
        assert!(cache.get("xx").await.is_none());
    }

    #[tokio::test]
    async fn get_returns_none_when_sparql_fails_without_panic() {
        // Degrade to an empty cache and return None.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let cache = cache_for(&server);
        assert!(
            cache.get("en").await.is_none(),
            "expected None on SPARQL failure"
        );
        // Second call also returns None: the cache holds the empty map,
        // we don't re-attempt the failed SPARQL on every lookup.
        assert!(cache.get("de").await.is_none());
    }
}
