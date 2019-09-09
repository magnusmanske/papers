use crate::*;
use mediawiki::api::Api;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
struct WikidataStringValue {
    timestamp: SystemTime,
    value: Option<String>, // "Qxxx", or none
}

impl WikidataStringValue {
    pub fn new(value: Option<String>) -> Self {
        Self {
            value: value,
            timestamp: SystemTime::now(),
        }
    }

    pub fn value(&mut self) -> Option<String> {
        self.update_timestamp();
        self.value.to_owned()
    }

    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    fn update_timestamp(&mut self) {
        self.timestamp = SystemTime::now();
    }
}

#[derive(Debug, Clone)]
pub struct WikidataStringCache {
    cache: HashMap<String, HashMap<String, WikidataStringValue>>,
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

    /// Gets an item ID for the property/value
    /// Uses search to find it if it's not in the cache
    pub fn get(&mut self, property: &str, value: &String) -> Option<String> {
        match self.ensure_property(property).get_mut(value) {
            Some(ret) => ret.value(),
            None => self.search(property, value),
        }
    }

    /// Set the value/q tuple for a property
    pub fn set(&mut self, property: &str, value: &String, q: Option<String>) {
        self.ensure_property(property)
            .insert(value.to_string(), WikidataStringValue::new(q));
    }

    /// Convenience wrapper
    pub fn issn2q(&mut self, issn: &String) -> Option<String> {
        self.get("P236", issn)
    }

    fn prune(&mut self) {
        let now = SystemTime::now();
        let allowed = Duration::from_secs(60 * 60); // 1h
        self.cache
            .iter_mut()
            .filter(|(_property, data)| data.len() > 100000) // TODO constant
            .for_each(|(_property, data)| {
                data.retain(|_k, v| {
                    let diff = now.duration_since(v.timestamp()).unwrap();
                    diff < allowed
                })
            });
    }

    /// Creates a new cache for a specific property
    fn ensure_property(&mut self, property: &str) -> &mut HashMap<String, WikidataStringValue> {
        if !self.cache.contains_key(property) {
            self.cache.insert(property.to_string(), HashMap::new());
        }
        self.cache.get_mut(property).unwrap()
    }

    /// Searches for items with a specific property/value
    /// Stores result in cache, and returns it
    /// Stores/returns None if no result found
    /// Stores/returns the first result, if multiple found
    fn search(&mut self, property: &str, value: &String) -> Option<String> {
        self.prune();
        let ret = match self.search_wikibase(
            &format!("haswbstatement:{}={}", property, value),
            &self.mw_api,
        ) {
            Ok(items) => match items.len() {
                0 => None,
                _ => Some(items[0].to_string()), // Picking first one, if several
            },
            Err(_) => None,
        };
        self.ensure_property(property)
            .insert(value.to_string(), WikidataStringValue::new(ret.to_owned()));
        ret
    }
}
