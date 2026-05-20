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
use wiremock::{Mock, MockServer, ResponseTemplate};

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
