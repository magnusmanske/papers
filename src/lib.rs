extern crate crossref;
extern crate reqwest;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate lazy_static;

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;
use wikibase::entity_diff::*;
use wikibase::mediawiki::api::Api;
use wikibase::*;

pub const PROP_PMID: &str = "P698";
pub const PROP_PMCID: &str = "P932";
pub const PROP_DOI: &str = "P356";
pub const PROP_ARXIV: &str = "P818";

#[async_trait]
pub trait WikidataInteraction {
    async fn search_wikibase(
        &self,
        query: &str,
        mw_api: Arc<RwLock<Api>>,
    ) -> Result<Vec<String>, String> {
        let params: HashMap<_, _> = vec![
            ("action", "query"),
            ("list", "search"),
            ("srnamespace", "0"),
            ("srsearch", query),
        ]
        .into_iter()
        .map(|(x, y)| (x.to_string(), y.to_string()))
        .collect();
        let res = mw_api
            .read()
            .await
            .get_query_api_json(&params)
            .await
            .map_err(|e| format!("{}", e))?;
        match res["query"]["search"].as_array() {
            Some(items) => Ok(items
                .iter()
                .filter_map(|item| item["title"].as_str().map(|s| s.to_string()))
                .collect()),
            None => Ok(vec![]),
        }
    }

    async fn create_item(&self, item: &Entity, mw_api: Arc<RwLock<Api>>) -> Option<String> {
        let params = EntityDiffParams::all();
        let diff = EntityDiff::new(&Entity::new_empty_item(), item, &params);
        if diff.is_empty() {
            return None;
        }
        let mut mw_api = mw_api.write().await;
        let new_json = diff.apply_diff(&mut mw_api, &diff).await.ok()?;
        drop(mw_api);
        EntityDiff::get_entity_id(&new_json)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericWorkType {
    Property(String),
    Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericWorkIdentifier {
    pub work_type: GenericWorkType,
    pub id: String,
}

impl GenericWorkIdentifier {
    pub fn new_prop(prop: &str, id: &str) -> Self {
        Self {
            work_type: GenericWorkType::Property(prop.to_string()),
            id: id.to_string(),
        }
    }

    pub fn is_legit(&self) -> bool {
        !self.id.is_empty() && self.id != "0"
    }
}

pub mod crossref2wikidata;
pub mod generic_author_info;
pub mod orcid2wikidata;
pub mod pmc2wikidata;
pub mod pubmed2wikidata;
pub mod scientific_publication_adapter;
pub mod semanticscholar2wikidata;
pub mod sourcemd_bot;
pub mod sourcemd_command;
pub mod sourcemd_config;
pub mod wikidata_papers;
pub mod wikidata_string_cache;

#[cfg(test)]
mod tests {
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    fn search_wikibase(
    fn create_item(&self, item: &Entity, mw_api: &mut Api) -> Option<String> {
    pub fn new_prop(prop: &str, id: &str) -> Self {
    pub fn is_legit(&self) -> bool {
    */
}
