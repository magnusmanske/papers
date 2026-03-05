use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use wikibase::mediawiki::api::Api;

use crate::wikidata_interaction::WikidataInteraction;

const MAX_CACHE_SIZE_PER_PROPERTY: usize = 10000;

#[derive(Debug, Clone)]
struct WikidataStringValue {
    timestamp: SystemTime,
    key: Option<String>, // "Qxxx", or none
}

impl WikidataStringValue {
    pub fn new(key: Option<String>) -> Self {
        Self {
            key,
            timestamp: SystemTime::now(),
        }
    }

    pub fn key(&mut self) -> Option<String> {
        self.update_timestamp();
        self.key.to_owned()
    }

    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    fn update_timestamp(&mut self) {
        self.timestamp = SystemTime::now();
    }
}

type WikidataStringHash = HashMap<String, WikidataStringValue>;

#[derive(Debug, Clone)]
pub struct WikidataStringCache {
    cache: Arc<tokio::sync::RwLock<HashMap<String, WikidataStringHash>>>,
    mw_api: Arc<tokio::sync::RwLock<Api>>,
    max_cache_size_per_property: usize,
}

impl WikidataInteraction for WikidataStringCache {}

impl WikidataStringCache {
    pub fn new(mw_api: Arc<tokio::sync::RwLock<Api>>) -> Self {
        Self {
            cache: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            mw_api,
            max_cache_size_per_property: MAX_CACHE_SIZE_PER_PROPERTY,
        }
    }

    /// Gets an item ID for the property/key
    /// Uses search to find it if it's not in the cache
    pub async fn get(&self, property: &str, key: &str) -> Option<String> {
        let key = self.fix_key(key);
        self.ensure_property(property).await;
        let (ret, do_search) = match self.cache.write().await.get_mut(property)?.get_mut(&key) {
            Some(ret) => {
                ret.update_timestamp();
                (ret.key(), false)
            }
            None => (None, true),
        };
        if do_search {
            self.search(property, &key).await
        } else {
            ret
        }
    }

    /// Set the key/q tuple for a property
    pub async fn set(&self, property: &str, key: &str, q: Option<String>) {
        let key = self.fix_key(key);
        self.ensure_property(property).await;
        self.cache
            .write()
            .await
            .get_mut(property)
            .expect("wikidata_string_cache::set: property not found")
            .insert(key, WikidataStringValue::new(q));
        self.prune_property(property).await;
    }

    /// Convenience wrapper
    pub async fn issn2q(&self, issn: &str) -> Option<String> {
        self.get("P236", issn).await
    }

    fn fix_key(&self, key: &str) -> String {
        key.trim().to_lowercase()
    }

    async fn property_needs_pruning(&self, property: &str) -> bool {
        match self.cache.read().await.get(&property.to_string()) {
            Some(hash) => hash.len() >= self.max_cache_size_per_property,
            None => false,
        }
    }

    async fn prune_property(&self, property: &str) {
        if !self.property_needs_pruning(property).await {
            return;
        }
        let mut cache = self.cache.write().await;
        let data = match cache.get_mut(&property.to_string()) {
            Some(data) => data,
            None => return,
        };

        // Do prune
        println!("Pruning {}", property);
        let mut times: Vec<SystemTime> = data.values().map(|v| v.timestamp()).collect();
        if times.is_empty() {
            return;
        }
        // TODO: CPU work — sort+retain run while holding async write lock;
        // refactor to release the lock before sorting if this becomes a bottleneck
        times.sort();
        // Remove older half of cache
        let half_time = times[times.len() / 2];
        data.retain(|_k, v| v.timestamp() >= half_time);
        println!("Pruned {} to {}", property, data.len());
    }

    /// Checks if a property has a key-value hash in the cache
    async fn has_property(&self, property: &str) -> bool {
        self.cache.read().await.get(&property.to_string()).is_some()
    }

    /// Creates a new cache for a specific property
    async fn ensure_property(&self, property: &str) {
        if !self.has_property(property).await {
            self.cache
                .write()
                .await
                .insert(property.to_string(), HashMap::new());
        }
    }

    /// Searches for items with a specific property/key
    /// Stores result in cache, and returns it
    /// Stores/returns None if no result found
    /// Stores/returns the first result, if multiple found
    async fn search(&self, property: &str, key: &str) -> Option<String> {
        let ret = match self
            .search_wikibase(
                &format!("haswbstatement:{}={}", property, key),
                self.mw_api.clone(),
            )
            .await
        {
            Ok(items) => items.first().map(|s| s.to_string()), // Picking first one, if several
            Err(_) => None,
        };
        self.set(property, key, ret.to_owned()).await;
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    use wikibase::mediawiki::api::Api;
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");
    const SEARCH_Q46664291: &str = include_str!("../test_data/search_found_q46664291.json");
    const SEARCH_Q15757256: &str = include_str!("../test_data/search_found_q15757256.json");
    const SEARCH_EMPTY: &str = include_str!("../test_data/search_empty.json");

    /// Starts a wiremock server with the MediaWiki siteinfo response pre-registered.
    /// The returned server must be kept alive for the duration of the test.
    async fn start_mock_server() -> MockServer {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("meta", "siteinfo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(SITEINFO),
            )
            .mount(&mock_server)
            .await;
        mock_server
    }

    /// Registers a search mock for a specific `srsearch` value.
    async fn add_search_mock(mock_server: &MockServer, srsearch: &str, body: &'static str) {
        Mock::given(method("GET"))
            .and(query_param("srsearch", srsearch))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(body),
            )
            .mount(mock_server)
            .await;
    }

    /// Creates an `Api` connected to the mock server.
    async fn mock_api(mock_server: &MockServer) -> Arc<tokio::sync::RwLock<Api>> {
        Arc::new(tokio::sync::RwLock::new(
            Api::new(&mock_server.uri()).await.unwrap(),
        ))
    }

    #[test]
    fn wsv_new() {
        assert_eq!(
            WikidataStringValue::new(Some("foobar".to_string())).key(),
            Some("foobar".to_string())
        );
    }

    #[test]
    fn wsv_timestamp() {
        let mut wsv = WikidataStringValue::new(Some("foobar".to_string()));
        let ts1 = wsv.timestamp();
        thread::sleep(Duration::from_millis(100));
        let _tmp = wsv.key();
        let ts2 = wsv.timestamp();
        assert_ne!(ts1, ts2);
    }

    #[tokio::test]
    async fn fix_key() {
        let mock_server = start_mock_server().await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert_eq!(wsc.fix_key(" fOoBAr  "), "foobar".to_string());
    }

    #[tokio::test]
    async fn ensure_property() {
        let mock_server = start_mock_server().await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert!(!wsc.cache.read().await.contains_key("P123"));
        wsc.ensure_property("P123").await;
        assert!(wsc.cache.read().await.contains_key("P123"));
    }

    #[tokio::test]
    async fn search() {
        let mock_server = start_mock_server().await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=16116339",
            SEARCH_Q46664291,
        )
        .await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=not_a_valid_id",
            SEARCH_EMPTY,
        )
        .await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert_eq!(
            wsc.search("P698", "16116339").await,
            Some("Q46664291".to_string())
        );
        assert_eq!(wsc.search("P698", "not_a_valid_id").await, None);
    }

    #[tokio::test]
    async fn get_set() {
        let mock_server = start_mock_server().await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=16116339",
            SEARCH_Q46664291,
        )
        .await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=not_a_valid_id",
            SEARCH_EMPTY,
        )
        .await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert_eq!(
            wsc.get("P698", "16116339").await,
            Some("Q46664291".to_string())
        );
        assert_eq!(wsc.get("P698", "not_a_valid_id").await, None);
        wsc.set("P698", "16116339", Some("foobar".to_string()))
            .await;
        assert_eq!(
            wsc.get("P698", "16116339").await,
            Some("foobar".to_string())
        );
        wsc.set("P698", "16116339", None).await;
        assert_eq!(wsc.get("P698", "16116339").await, None);
    }

    #[tokio::test]
    async fn issn2q() {
        let mock_server = start_mock_server().await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P236=1351-5101",
            SEARCH_Q15757256,
        )
        .await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P236=nope-di-dope",
            SEARCH_EMPTY,
        )
        .await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert_eq!(wsc.issn2q("1351-5101").await, Some("Q15757256".to_string()));
        assert_eq!(wsc.issn2q("nope-di-dope").await, None);
    }

    #[tokio::test]
    async fn prune() {
        let mock_server = start_mock_server().await;
        let mut wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        for num in 1..10 {
            wsc.set(
                "P123",
                &format!("Key #{}", num),
                Some(format!("Value #{}", num)),
            )
            .await;
            thread::sleep(Duration::from_millis(10));
        }
        wsc.max_cache_size_per_property = 5;
        wsc.prune_property("P123").await;
        assert_eq!(wsc.cache.read().await.get("P123").unwrap().len(), 5);
    }

    #[tokio::test]
    async fn property_needs_pruning() {
        let mock_server = start_mock_server().await;
        let mut wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert!(!wsc.property_needs_pruning("P123").await);
        for num in 1..10 {
            wsc.set(
                "P123",
                &format!("Key #{}", num),
                Some(format!("Value #{}", num)),
            )
            .await;
        }
        wsc.max_cache_size_per_property = 5;
        assert!(wsc.property_needs_pruning("P123").await);
    }

    #[tokio::test]
    async fn prune_empty_property() {
        let mock_server = start_mock_server().await;
        let mut wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        wsc.max_cache_size_per_property = 0; // Force pruning to trigger
        wsc.ensure_property("P999").await;
        // This should not panic even though the property has zero entries
        wsc.prune_property("P999").await;
        assert_eq!(wsc.cache.read().await.get("P999").unwrap().len(), 0);
    }

    #[tokio::test]
    async fn has_property() {
        let mock_server = start_mock_server().await;
        let wsc = WikidataStringCache::new(mock_api(&mock_server).await);
        assert!(!wsc.has_property("P123").await);
        wsc.ensure_property("P123").await;
        assert!(wsc.has_property("P123").await);
    }

    #[tokio::test]
    async fn search_wikibase() {
        let mock_server = start_mock_server().await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=16116339",
            SEARCH_Q46664291,
        )
        .await;
        add_search_mock(
            &mock_server,
            "haswbstatement:P698=not_a_valid_id",
            SEARCH_EMPTY,
        )
        .await;
        let api = mock_api(&mock_server).await;
        let wsc = WikidataStringCache::new(api.clone());
        assert_eq!(
            wsc.search_wikibase("haswbstatement:P698=16116339", api.clone())
                .await
                .unwrap(),
            vec!["Q46664291".to_string()]
        );
        assert_eq!(
            wsc.search_wikibase("haswbstatement:P698=not_a_valid_id", api.clone())
                .await
                .unwrap()
                .len(),
            0
        );
    }
}
