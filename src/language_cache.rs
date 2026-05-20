//! ISO-language-code → Wikidata-Q-item resolver.
//!
//! Looks up the Wikidata Q-item for a language identified by its ISO 639
//! code by issuing a single SPARQL query against the Wikidata Query
//! Service the first time it's needed, and caching the result for the
//! rest of the process lifetime.
//!
//! Previously this lived as a default method on
//! `ScientificPublicationAdapter` and:
//! - hard-coded the Wikidata API URL (DIP violation),
//! - `expect()`-panicked on network / parse failure (operational risk),
//! - couldn't be tested independently of an adapter instance.
//!
//! This module encapsulates the cache, makes the API URL overridable
//! via the `PAPERS_WIKIDATA_API` env var, and degrades to an empty
//! cache (so `get` returns `None`) on any failure path.
//!
//! See `audits/STATUS.md` P2-6.

use std::collections::HashMap;

use tokio::sync::OnceCell;
use wikibase::mediawiki::api::Api;

/// Default API URL used when no override is set.
pub const DEFAULT_WIKIDATA_API_URL: &str = "https://www.wikidata.org/w/api.php";

/// Environment variable that, when set, overrides the API URL — used by
/// tests (e.g. with a `wiremock` server) and lets operators redirect to
/// a mirror in production.
pub const API_URL_ENV: &str = "PAPERS_WIKIDATA_API";

/// SPARQL that materialises the language-code → Q-item map. Each row is
/// (ISO code, Wikidata Q-item). One language can have multiple codes
/// (P219 / P220) so we collect them all in a single pass.
const LANGUAGE_SPARQL: &str =
    "SELECT DISTINCT ?l ?q { ?q wdt:P31/wdt:P279* wd:Q20162172; (wdt:P219|wdt:P220) ?l }";

/// Lazily-populated map from language identifier to Wikidata Q-item.
///
/// Construct one per process via [`LanguageCache::wikidata`] (uses the
/// env-overridable default) or [`LanguageCache::new`] (explicit URL,
/// for tests).
pub struct LanguageCache {
    map: OnceCell<HashMap<String, String>>,
    api_url: String,
}

impl LanguageCache {
    /// Build a cache pointing at `api_url`. Lazy: no HTTP is issued until
    /// the first `get` call.
    pub fn new(api_url: impl Into<String>) -> Self {
        Self { map: OnceCell::new(), api_url: api_url.into() }
    }

    /// Build a cache pointing at production Wikidata, or whatever the
    /// `PAPERS_WIKIDATA_API` env var says.
    pub fn wikidata() -> Self {
        let url = std::env::var(API_URL_ENV)
            .unwrap_or_else(|_| DEFAULT_WIKIDATA_API_URL.to_string());
        Self::new(url)
    }

    /// Look up the Q-item for a language identifier. Returns `None` if
    /// the language is unknown or if the underlying SPARQL load fails
    /// (the failure is logged and the cache stays empty so the next
    /// call doesn't repeatedly retry — the previous behaviour was to
    /// panic on the first failure).
    pub async fn get(&self, language: &str) -> Option<String> {
        self.map.get_or_init(|| self.load()).await.get(language).cloned()
    }

    async fn load(&self) -> HashMap<String, String> {
        match Self::try_load(&self.api_url).await {
            Ok(map) => {
                tracing::info!(entries = map.len(), "LanguageCache loaded");
                map
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    api_url = %self.api_url,
                    "LanguageCache: SPARQL load failed; degrading to empty cache",
                );
                HashMap::new()
            },
        }
    }

    async fn try_load(api_url: &str) -> Result<HashMap<String, String>, anyhow::Error> {
        let mw_api = Api::new(api_url).await?;
        let json = mw_api.sparql_query(LANGUAGE_SPARQL).await?;
        let bindings = json["results"]["bindings"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("LanguageCache: SPARQL response missing bindings"))?;
        Ok(bindings
            .iter()
            .filter_map(|j| {
                let l = j["l"]["value"].as_str()?;
                let q = mw_api.extract_entity_from_uri(j["q"]["value"].as_str()?).ok()?;
                Some((l.to_string(), q.to_string()))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");

    #[tokio::test]
    async fn get_returns_none_when_sparql_fails_without_panic() {
        // The pre-refactor code panicked on SPARQL failure
        // (`.expect("generate_l2q: fail1")`). This test pins the new
        // contract: degrade to an empty cache and return None.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("meta", "siteinfo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(SITEINFO),
            )
            .mount(&server)
            .await;
        // All other GETs return 500 — should not panic.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let cache = LanguageCache::new(server.uri());
        assert!(cache.get("en").await.is_none(), "expected None on SPARQL failure");
        // Second call also returns None: the cache holds the empty map,
        // we don't re-attempt the failed SPARQL on every lookup.
        assert!(cache.get("de").await.is_none());
    }

    // Env-var tests run serially (both touch a global) — combined into
    // one test to avoid cross-test races.
    #[test]
    fn wikidata_constructor_honours_env_var_and_default() {
        let prev = std::env::var(API_URL_ENV).ok();

        // 1. With env var set, that URL wins.
        std::env::set_var(API_URL_ENV, "http://example.test/api.php");
        let c = LanguageCache::wikidata();
        assert_eq!(c.api_url, "http://example.test/api.php");

        // 2. With env var unset, default URL is used.
        std::env::remove_var(API_URL_ENV);
        let c = LanguageCache::wikidata();
        assert_eq!(c.api_url, DEFAULT_WIKIDATA_API_URL);

        // Restore.
        match prev {
            Some(v) => std::env::set_var(API_URL_ENV, v),
            None => std::env::remove_var(API_URL_ENV),
        }
    }
}
