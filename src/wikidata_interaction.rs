use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::RwLock;
use wikibase::{entity_diff::*, mediawiki::api::Api, *};

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

    /// Creates a new Wikidata entity from `item`'s diff against an empty
    /// item. Returns `Ok(None)` if the diff is empty (nothing to write);
    /// `Err(_)` if the underlying API call fails so callers can mark the
    /// command failed instead of silently dropping the write.
    async fn create_item(
        &self,
        item: &Entity,
        mw_api: Arc<RwLock<Api>>,
    ) -> Result<Option<String>> {
        let params = EntityDiffParams::all();
        let mut diff = EntityDiff::new(&Entity::new_empty_item(), item, &params);
        diff.set_edit_summary(Some("(automated edit by SourceMD)".to_string()));
        if diff.is_empty() {
            return Ok(None);
        }
        let mut mw_api = mw_api.write().await;
        let new_json = diff
            .apply_diff(&mut mw_api, &diff)
            .await
            .map_err(|e| anyhow!("create_item: apply_diff failed: {e}"))?;
        drop(mw_api);
        Ok(EntityDiff::get_entity_id(&new_json))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        matchers::{method, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;
    use crate::test_helpers::start_mediawiki_mock_server as start_mock_server;

    const SEARCH_Q46664291: &str = include_str!("../test_data/search_found_q46664291.json");
    const SEARCH_EMPTY: &str = include_str!("../test_data/search_empty.json");

    /// Bare trait implementor for unit-testing the default trait methods.
    struct DummyInteractor;
    impl WikidataInteraction for DummyInteractor {}

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

    async fn mock_api(mock_server: &MockServer) -> Arc<RwLock<Api>> {
        Arc::new(RwLock::new(Api::new(&mock_server.uri()).await.unwrap()))
    }

    #[tokio::test]
    async fn search_wikibase_returns_titles_when_search_has_hits() {
        let mock_server = start_mock_server().await;
        add_search_mock(&mock_server, "haswbstatement:P698=16116339", SEARCH_Q46664291).await;
        let api = mock_api(&mock_server).await;
        let d = DummyInteractor;
        let result = d.search_wikibase("haswbstatement:P698=16116339", api).await.unwrap();
        assert_eq!(result, vec!["Q46664291".to_string()]);
    }

    #[tokio::test]
    async fn search_wikibase_returns_empty_vec_when_no_hits() {
        let mock_server = start_mock_server().await;
        add_search_mock(&mock_server, "haswbstatement:P698=missing", SEARCH_EMPTY).await;
        let api = mock_api(&mock_server).await;
        let d = DummyInteractor;
        let result = d.search_wikibase("haswbstatement:P698=missing", api).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn create_item_returns_ok_none_for_empty_diff() {
        // Two identical empty items produce an empty diff, so create_item should
        // bail out early without performing any HTTP write — Ok(None), not Err.
        let mock_server = start_mock_server().await;
        let api = mock_api(&mock_server).await;
        let d = DummyInteractor;
        let empty = Entity::new_empty_item();
        let r = d.create_item(&empty, api).await;
        assert!(matches!(r, Ok(None)), "expected Ok(None), got {:?}", r.as_ref().err());
    }
}
