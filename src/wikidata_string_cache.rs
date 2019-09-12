use crate::*;
use mediawiki::api::Api;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

const MAX_CACHE_SIZE_PER_PROPERTY: usize = 10000;

#[derive(Debug, Clone)]
struct WikidataStringValue {
    timestamp: SystemTime,
    key: Option<String>, // "Qxxx", or none
}

impl WikidataStringValue {
    pub fn new(key: Option<String>) -> Self {
        Self {
            key: key,
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
    cache: Arc<RwLock<HashMap<String, WikidataStringHash>>>,
    mw_api: Arc<RwLock<Api>>,
    max_cache_size_per_property: usize,
}

impl WikidataInteraction for WikidataStringCache {}

impl WikidataStringCache {
    pub fn new(mw_api: Arc<RwLock<Api>>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            mw_api: mw_api,
            max_cache_size_per_property: MAX_CACHE_SIZE_PER_PROPERTY,
        }
    }

    /// Gets an item ID for the property/key
    /// Uses search to find it if it's not in the cache
    pub fn get(&self, property: &str, key: &String) -> Option<String> {
        let key = self.fix_key(key);
        self.ensure_property(property);
        let mut do_search = false;
        let ret = match self
            .cache
            .write()
            .unwrap()
            .get_mut(property)
            .unwrap() // Safe
            .get_mut(&key)
        {
            Some(ret) => {
                ret.update_timestamp();
                ret.key()
            }
            None => {
                do_search = true;
                None
            }
        };
        if do_search {
            self.search(property, &key)
        } else {
            ret
        }
    }

    /// Set the key/q tuple for a property
    pub fn set(&self, property: &str, key: &String, q: Option<String>) {
        let key = self.fix_key(key);
        self.ensure_property(property);
        self.cache
            .write()
            .unwrap()
            .get_mut(property)
            .unwrap() // Safe
            .insert(key, WikidataStringValue::new(q));
        self.prune_property(property);
    }

    /// Convenience wrapper
    pub fn issn2q(&self, issn: &String) -> Option<String> {
        self.get("P236", issn)
    }

    fn fix_key(&self, key: &String) -> String {
        let ret: String = key.to_string().trim().to_lowercase().into();
        ret
    }

    fn proterty_needs_pruning(&self, property: &str) -> bool {
        if !self.has_property(property) {
            return false;
        }
        let cache = self.cache.read().unwrap();
        if cache.get(&property.to_string()).unwrap().len() < self.max_cache_size_per_property {
            return false;
        }
        true
    }

    fn prune_property(&self, property: &str) {
        if !self.proterty_needs_pruning(property) {
            return;
        }
        let mut cache = self.cache.write().unwrap();
        let data = match cache.get_mut(&property.to_string()) {
            Some(data) => data,
            None => return,
        };
        println!("Pruning {}", property);
        let mut times: Vec<SystemTime> = data.iter().map(|(_k, v)| v.timestamp()).collect();
        times.sort();
        // Remove older half of cache
        let half_time = times[times.len() / 2];
        data.retain(|_k, v| v.timestamp() >= half_time);
        println!("Pruned {} to {}", property, data.len());
    }

    /// Checks if a property has a key-value hash in the cache
    fn has_property(&self, property: &str) -> bool {
        self.cache
            .read()
            .unwrap()
            .get(&property.to_string())
            .is_some()
    }

    /// Creates a new cache for a specific property
    fn ensure_property(&self, property: &str) {
        if !self.has_property(property) {
            self.cache
                .write()
                .unwrap()
                .entry(property.to_string())
                .or_insert(HashMap::new());
        }
    }

    /// Searches for items with a specific property/key
    /// Stores result in cache, and returns it
    /// Stores/returns None if no result found
    /// Stores/returns the first result, if multiple found
    fn search(&self, property: &str, key: &String) -> Option<String> {
        let ret = match self.search_wikibase(
            &format!("haswbstatement:{}={}", property, key),
            self.mw_api.clone(),
        ) {
            Ok(items) => items.get(0).map(|s| s.to_string()), // Picking first one, if several
            Err(_) => None,
        };
        self.set(property, key, ret.to_owned());
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediawiki::api::Api;
    use std::thread;
    use std::time::Duration;

    fn api() -> Arc<RwLock<Api>> {
        lazy_static! {
            static ref API: Arc<RwLock<Api>> = Arc::new(RwLock::new(
                Api::new("https://www.wikidata.org/w/api.php").unwrap(),
            ));
        }
        API.clone()
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

    #[test]
    fn fix_key() {
        let wsc = WikidataStringCache::new(api());
        assert_eq!(wsc.fix_key(&" fOoBAr  ".to_string()), "foobar".to_string());
    }

    #[test]
    fn ensure_property() {
        let wsc = WikidataStringCache::new(api());
        assert!(!wsc.cache.read().unwrap().contains_key("P123"));
        wsc.ensure_property("P123");
        assert!(wsc.cache.read().unwrap().contains_key("P123"));
    }

    #[test]
    fn search() {
        let wsc = WikidataStringCache::new(api());
        assert_eq!(
            wsc.search("P698", &"16116339".to_string().into()),
            Some("Q46664291".to_string())
        );
        assert_eq!(
            wsc.search("P698", &"not_a_valid_id".to_string().into()),
            None
        );
    }

    #[test]
    fn get_set() {
        let wsc = WikidataStringCache::new(api());
        assert_eq!(
            wsc.get("P698", &"16116339".to_string()),
            Some("Q46664291".to_string())
        );
        assert_eq!(wsc.get("P698", &"not_a_valid_id".to_string()), None);
        wsc.set("P698", &"16116339".to_string(), Some("foobar".to_string()));
        assert_eq!(
            wsc.get("P698", &"16116339".to_string()),
            Some("foobar".to_string())
        );
        wsc.set("P698", &"16116339".to_string(), None);
        assert_eq!(wsc.get("P698", &"16116339".to_string()), None);
    }

    #[test]
    fn issn2q() {
        let wsc = WikidataStringCache::new(api());
        assert_eq!(
            wsc.issn2q(&"1351-5101".to_string()),
            Some("Q15757256".to_string())
        );
        assert_eq!(wsc.issn2q(&"nope-di-dope".to_string()), None);
    }

    #[test]
    fn prune() {
        let mut wsc = WikidataStringCache::new(api());
        for num in 1..10 {
            wsc.set(
                "P123",
                &format!("Key #{}", num),
                Some(format!("Value #{}", num)),
            );
            thread::sleep(Duration::from_millis(10));
        }
        wsc.max_cache_size_per_property = 5;
        wsc.prune_property("P123");
        assert_eq!(wsc.cache.read().unwrap().get("P123").unwrap().len(), 5);
    }
}
