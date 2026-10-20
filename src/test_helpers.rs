//! Shared test-only helpers. Compiled only under `#[cfg(test)]`.
//!
//! Pre-fix, four test modules (`sourcemd_config`, `wikidata_string_cache`,
//! `sourcemd_bot`, `wikidata_interaction`) each carried a verbatim copy of
//! `start_mock_server` — a wiremock server that responds to the
//! MediaWiki `meta=siteinfo` query with a pre-recorded fixture. The
//! audit (test-audit §1.3, P3 polish) flagged this as the duplicate
//! most likely to drift between callers.

#![cfg(test)]

use wiremock::matchers::{method, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Pre-recorded `meta=siteinfo` response from `wikidata.org/w/api.php`.
/// Lets `mediawiki::Api::new(&mock.uri())` succeed without touching the
/// network.
pub(crate) const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");

/// Starts a wiremock server with the MediaWiki `siteinfo` response
/// pre-registered. The returned server must be kept alive for the
/// duration of the test — when it's dropped, the bound port closes.
pub(crate) async fn start_mediawiki_mock_server() -> MockServer {
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

/// The decoded form field `key` of a POST request.
fn form_field(request: &Request, key: &str) -> Option<String> {
    // A form body is encoded like a URL's query string.
    let form = format!("http://form/?{}", String::from_utf8_lossy(&request.body));
    reqwest::Url::parse(&form)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// Registers a search (`list=search`) mock for a specific `srsearch` value.
/// Searches are POSTed as forms.
pub(crate) async fn add_search_mock(mock_server: &MockServer, srsearch: &str, body: &'static str) {
    let srsearch = srsearch.to_string();
    Mock::given(method("POST"))
        .and(move |request: &Request| form_field(request, "srsearch").as_deref() == Some(&srsearch))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json; charset=utf-8")
                .set_body_string(body),
        )
        .mount(mock_server)
        .await;
}
