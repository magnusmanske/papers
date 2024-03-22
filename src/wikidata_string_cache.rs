use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use wikibase::mediawiki::api::Api;

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
        let (ret, do_search) = match self
            .cache
            .write()
            .await
            .get_mut(property)
            .unwrap()
            .get_mut(&key)
        {
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
            .unwrap() // Safe
            .insert(key, WikidataStringValue::new(q));
        self.prune_property(property).await;
    }

    /// Convenience wrapper
    pub async fn issn2q(&self, issn: &str) -> Option<String> {
        self.get("P236", issn).await
    }

    fn fix_key(&self, key: &str) -> String {
        let ret: String = key.to_string().trim().to_lowercase();
        ret
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
        let mut times: Vec<SystemTime> = data.iter().map(|(_k, v)| v.timestamp()).collect();
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
    use tokio;
    use wikibase::mediawiki::api::Api;

    async fn api() -> Arc<tokio::sync::RwLock<Api>> {
        // lazy_static! {
        // static ref API: Arc<RwLock<Api>> =
        Arc::new(tokio::sync::RwLock::new(
            Api::new("https://www.wikidata.org/w/api.php")
                .await
                .unwrap(),
        ))
        // }
        // API.clone()
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
        let wsc = WikidataStringCache::new(api().await);
        assert_eq!(wsc.fix_key(&" fOoBAr  ".to_string()), "foobar".to_string());
    }

    #[tokio::test]
    async fn ensure_property() {
        let wsc = WikidataStringCache::new(api().await);
        assert!(!wsc.cache.read().await.contains_key("P123"));
        wsc.ensure_property("P123").await;
        assert!(wsc.cache.read().await.contains_key("P123"));
    }

    #[tokio::test]
    async fn search() {
        let wsc = WikidataStringCache::new(api().await);
        assert_eq!(
            wsc.search("P698", "16116339").await,
            Some("Q46664291".to_string())
        );
        assert_eq!(wsc.search("P698", "not_a_valid_id").await, None);
    }

    #[tokio::test]
    async fn get_set() {
        let wsc = WikidataStringCache::new(api().await);
        assert_eq!(
            wsc.get("P698", &"16116339".to_string()).await,
            Some("Q46664291".to_string())
        );
        assert_eq!(wsc.get("P698", &"not_a_valid_id".to_string()).await, None);
        wsc.set("P698", &"16116339".to_string(), Some("foobar".to_string()))
            .await;
        assert_eq!(
            wsc.get("P698", &"16116339".to_string()).await,
            Some("foobar".to_string())
        );
        wsc.set("P698", &"16116339".to_string(), None).await;
        assert_eq!(wsc.get("P698", &"16116339".to_string()).await, None);
    }

    #[tokio::test]
    async fn issn2q() {
        let wsc = WikidataStringCache::new(api().await);
        assert_eq!(
            wsc.issn2q(&"1351-5101".to_string()).await,
            Some("Q15757256".to_string())
        );
        assert_eq!(wsc.issn2q(&"nope-di-dope".to_string()).await, None);
    }

    #[tokio::test]
    async fn prune() {
        let mut wsc = WikidataStringCache::new(api().await);
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
}
