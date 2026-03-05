use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wikibase::entity_diff::*;
use wikibase::mediawiki::api::Api;
use wikibase::*;

#[async_trait]
pub trait WikidataInteraction {
    async fn search_wikibase(&self, query: &str, mw_api: Arc<RwLock<Api>>) -> Result<Vec<String>> {
        let params: HashMap<_, _> = vec![
            ("action", "query"),
            ("list", "search"),
            ("srnamespace", "0"),
            ("srsearch", query),
        ]
        .into_iter()
        .map(|(x, y)| (x.to_string(), y.to_string()))
        .collect();
        let res = mw_api.read().await.get_query_api_json(&params).await?;
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
        let mut diff = EntityDiff::new(&Entity::new_empty_item(), item, &params);
        diff.set_edit_summary(Some("(automated edit by SourceMD)".to_string()));
        if diff.is_empty() {
            return None;
        }
        let mut mw_api = mw_api.write().await;
        let new_json = diff.apply_diff(&mut mw_api, &diff).await.ok()?;
        drop(mw_api);
        EntityDiff::get_entity_id(&new_json)
    }
}

#[cfg(test)]
mod tests {}
