extern crate smallstring;

use crate::*;
use mediawiki::api::Api;
use smallstring::SmallString;
use std::collections::HashMap;
use std::time::SystemTime;

const MAX_CACHE_SIZE_PER_PROPERTY: usize = 100000;

#[derive(Debug, Clone)]
struct WikidataStringValue {
    timestamp: SystemTime,
    key: Option<SmallString>, // "Qxxx", or none
}

impl WikidataStringValue {
    pub fn new(key: Option<String>) -> Self {
        Self {
            key: key.map(|v| v.into()),
            timestamp: SystemTime::now(),
        }
    }

    pub fn key(&mut self) -> Option<String> {
        self.update_timestamp();
        self.key.to_owned().map(|v| v.into())
    }

    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    fn update_timestamp(&mut self) {
        self.timestamp = SystemTime::now();
    }
}

type WikidataStringHash = HashMap<SmallString, WikidataStringValue>;

#[derive(Debug, Clone)]
pub struct WikidataStringCache {
    cache: HashMap<String, WikidataStringHash>,
    mw_api: Api,
    max_cache_size_per_property: usize,
}

impl WikidataInteraction for WikidataStringCache {}

impl WikidataStringCache {
    pub fn new(mw_api: &Api) -> Self {
        Self {
            cache: HashMap::new(),
            mw_api: mw_api.clone(),
            max_cache_size_per_property: MAX_CACHE_SIZE_PER_PROPERTY,
        }
    }

    /// Gets an item ID for the property/key
    /// Uses search to find it if it's not in the cache
    pub fn get(&mut self, property: &str, key: &String) -> Option<String> {
        let key = self.fix_key(key);
        match self.ensure_property(property).get_mut(&key) {
            Some(ret) => {
                ret.update_timestamp();
                ret.key()
            }
            None => self.search(property, &key),
        }
    }

    /// Set the key/q tuple for a property
    pub fn set(&mut self, property: &str, key: &String, q: Option<String>) {
        let key = self.fix_key(key);
        self.ensure_property(property)
            .insert(key, WikidataStringValue::new(q));
        self.prune_property(property);
    }

    /// Convenience wrapper
    pub fn issn2q(&mut self, issn: &String) -> Option<String> {
        self.get("P236", issn)
    }

    fn fix_key(&self, key: &String) -> SmallString {
        let ret: SmallString = key.to_string().trim().to_lowercase().into();
        ret
    }

    fn prune_property(&mut self, property: &str) {
        let data = match self.cache.get_mut(&property.to_string()) {
            Some(data) => data,
            None => return,
        };
        if data.len() < self.max_cache_size_per_property {
            return;
        }
        println!("Pruning {}", property);
        let mut times: Vec<SystemTime> = data.iter().map(|(_k, v)| v.timestamp()).collect();
        times.sort();
        // Remove older half of cache
        let half_time = times[times.len() / 2];
        data.retain(|_k, v| v.timestamp() >= half_time);
        println!("Pruned {} to {}", property, data.len());
    }

    /// Creates a new cache for a specific property
    fn ensure_property(
        &mut self,
        property: &str,
    ) -> &mut HashMap<SmallString, WikidataStringValue> {
        self.cache
            .entry(property.to_string())
            .or_insert(HashMap::new())
    }

    /// Searches for items with a specific property/key
    /// Stores result in cache, and returns it
    /// Stores/returns None if no result found
    /// Stores/returns the first result, if multiple found
    fn search(&mut self, property: &str, key: &SmallString) -> Option<String> {
        let ret = match self.search_wikibase(
            &format!("haswbstatement:{}={}", property, key),
            &self.mw_api,
        ) {
            Ok(items) => match items.len() {
                0 => None,
                _ => Some(items[0].to_string()), // Picking first one, if several
            },
            Err(_) => None,
        };
        self.ensure_property(property)
            .insert(key.to_owned(), WikidataStringValue::new(ret.to_owned()));
        self.prune_property(property);
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediawiki::api::Api;
    use std::thread;
    use std::time::Duration;

    fn api() -> Api {
        Api::new("https://www.wikidata.org/w/api.php").unwrap()
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
        let wsc = WikidataStringCache::new(&api());
        assert_eq!(
            wsc.fix_key(&" fOoBAr  ".to_string()),
            "foobar".to_string().into()
        );
    }

    #[test]
    fn ensure_property() {
        let mut wsc = WikidataStringCache::new(&api());
        assert!(!wsc.cache.contains_key("P123"));
        wsc.ensure_property("P123");
        assert!(wsc.cache.contains_key("P123"));
    }

    #[test]
    fn search() {
        let mut wsc = WikidataStringCache::new(&api());
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
        let mut wsc = WikidataStringCache::new(&api());
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
        let mut wsc = WikidataStringCache::new(&api());
        assert_eq!(
            wsc.issn2q(&"1351-5101".to_string()),
            Some("Q15757256".to_string())
        );
        assert_eq!(wsc.issn2q(&"nope-di-dope".to_string()), None);
    }

    #[test]
    fn prune() {
        let mut wsc = WikidataStringCache::new(&api());
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
        assert_eq!(wsc.ensure_property("P123").len(), 5);
    }
}
