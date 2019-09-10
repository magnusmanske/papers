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
    value: Option<SmallString>, // "Qxxx", or none
}

impl WikidataStringValue {
    pub fn new(value: Option<String>) -> Self {
        Self {
            value: value.map(|v| v.into()),
            timestamp: SystemTime::now(),
        }
    }

    pub fn value(&mut self) -> Option<String> {
        self.update_timestamp();
        self.value.to_owned().map(|v| v.into())
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

    /// Gets an item ID for the property/value
    /// Uses search to find it if it's not in the cache
    pub fn get(&mut self, property: &str, value: &String) -> Option<String> {
        let value: SmallString = value.to_string().into();
        match self.ensure_property(property).get_mut(&value) {
            Some(ret) => {
                ret.update_timestamp();
                ret.value()
            }
            None => self.search(property, &value),
        }
    }

    /// Set the value/q tuple for a property
    pub fn set(&mut self, property: &str, value: &String, q: Option<String>) {
        self.ensure_property(property)
            .insert(value.to_string().into(), WikidataStringValue::new(q));
        self.prune_property(property);
    }

    /// Convenience wrapper
    pub fn issn2q(&mut self, issn: &String) -> Option<String> {
        self.get("P236", issn)
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

    /// Searches for items with a specific property/value
    /// Stores result in cache, and returns it
    /// Stores/returns None if no result found
    /// Stores/returns the first result, if multiple found
    fn search(&mut self, property: &str, value: &SmallString) -> Option<String> {
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
            .insert(value.to_owned(), WikidataStringValue::new(ret.to_owned()));
        self.prune_property(property);
        ret
    }
}
