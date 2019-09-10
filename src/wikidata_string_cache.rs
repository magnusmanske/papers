extern crate smallstring;

use crate::*;
use mediawiki::api::Api;
use smallstring::SmallString;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

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
}

impl WikidataInteraction for WikidataStringCache {}

impl WikidataStringCache {
    pub fn new(mw_api: &Api) -> Self {
        Self {
            cache: HashMap::new(),
            mw_api: mw_api.clone(),
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
        let ret: SmallString = key.to_string().to_lowercase().into();
        ret
    }

    fn prune_property(&mut self, property: &str) {
        let data = match self.cache.get_mut(&property.to_string()) {
            Some(data) => data,
            None => return,
        };
        if data.len() < MAX_CACHE_SIZE_PER_PROPERTY {
            return;
        }
        println!("Pruning {}", property);
        let now = SystemTime::now();
        let allowed = Duration::from_secs(60 * 60); // 1h
        data.retain(|_k, v| {
            let diff = now.duration_since(v.timestamp()).unwrap();
            diff < allowed
        });
        println!("Pruned {} to {}", property, data.len());
    }

    /// Creates a new cache for a specific property
    fn ensure_property(
        &mut self,
        property: &str,
    ) -> &mut HashMap<SmallString, WikidataStringValue> {
        if !self.cache.contains_key(property) {
            self.cache.insert(property.to_string(), HashMap::new());
        }
        self.cache.get_mut(property).unwrap()
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
