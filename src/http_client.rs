//! Shared HTTP client + retry helper for outbound provider calls.
//!
//! Provides a single process-wide [`reqwest::Client`] (with timeouts and
//! a `User-Agent`), a second one with a timeout long enough for SPARQL
//! queries, and [`retry_policy`], the [`HttpRetryPolicy`] that retries
//! transient failures (timeouts, connect errors, HTTP 5xx, 408, 429)
//! with exponential backoff + jitter.
//!
//! `fetch_json` is the drop-in replacement for the previous pattern
//! `reqwest::get(&url).await.ok()?.json().await.ok()?`.
//!
//! The [`JsonFetcher`] trait + [`HttpJsonFetcher`] production impl are
//! the dependency-injection seam used by adapters that hit JSON HTTP
//! endpoints directly. Test code can construct an in-memory
//! [`MockJsonFetcher`] (`cfg(test)`) to capture the URLs an adapter
//! would have hit and to feed it canned responses without touching a
//! real network. See audit P2-10.

use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use wikimisc::http::{client_builder, HttpError, HttpRetryPolicy};
use wikimisc::retry::{Retry, Retryable};

/// What papers' requests say they are. Wikimedia's user-agent policy asks
/// for contact information.
pub const USER_AGENT: &str = wikimisc::user_agent!();

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// WDQS gives a query up to 60 s; this leaves room for sending the result.
const SPARQL_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 8;

/// Retries after the first attempt, and the waits between them.
const RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(250);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(5);

fn build_client(timeout: Duration) -> Client {
    client_builder(USER_AGENT)
        .timeout(timeout)
        .pool_idle_timeout(DEFAULT_POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(DEFAULT_POOL_MAX_IDLE_PER_HOST)
        .build()
        .expect("failed to build shared HTTP client")
}

/// Returns the process-wide HTTP client.
pub fn http_client() -> &'static Client {
    static HTTP: OnceLock<Client> = OnceLock::new();
    HTTP.get_or_init(|| build_client(DEFAULT_TIMEOUT))
}

/// Returns the process-wide HTTP client for SPARQL queries.
pub fn sparql_http_client() -> &'static Client {
    static HTTP: OnceLock<Client> = OnceLock::new();
    HTTP.get_or_init(|| build_client(SPARQL_TIMEOUT))
}

/// The retry policy for transient HTTP failures: 3 retries, waiting
/// 250 ms, 500 ms, 1 s (each plus up to half again as jitter, never more
/// than 5 s). A `Retry-After` is honoured up to that 5 s cap; a longer
/// one is cut down to it rather than ending the retries.
pub fn retry_policy() -> HttpRetryPolicy {
    HttpRetryPolicy::new(RETRIES)
        .with_initial_backoff(RETRY_BASE_DELAY)
        .with_max_wait(RETRY_MAX_DELAY)
        .with_jitter()
        .clamp_retry_after()
        .on_retry(|retry: &Retry<'_, HttpError>| {
            tracing::warn!(
                error = %retry.error,
                attempt = retry.retry,
                "HTTP transient failure, retrying"
            );
        })
}

/// GET `url` and decode the response body as JSON, retrying transient
/// failures with the default [`retry_policy`].
///
/// Returns `None` on transport failure, non-2xx response (after
/// retries), or JSON parse failure. Logs failures via `tracing`.
pub async fn fetch_json(url: &str) -> Option<serde_json::Value> {
    fetch_json_with(url, &retry_policy()).await
}

pub async fn fetch_json_with(url: &str, policy: &HttpRetryPolicy) -> Option<serde_json::Value> {
    let resp = match policy.send(http_client().get(url)).await {
        Ok(r) => r,
        Err(e) if e.is_retryable() && policy.retries() > 0 => {
            tracing::error!(error = %e, url, "HTTP gave up on retriable failure");
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e, url, "HTTP request failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(
            status = resp.status().as_u16(),
            url,
            "non-success HTTP response"
        );
        return None;
    }
    match resp.json::<serde_json::Value>().await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, url, "JSON decode failed");
            None
        }
    }
}

/// Dependency-injection seam for adapters that fetch JSON over HTTP.
///
/// Production callers use [`HttpJsonFetcher`]; tests use
/// [`MockJsonFetcher`] (or any other impl) to record the URLs an
/// adapter would hit and to feed it canned responses.
///
/// Semantics match [`fetch_json`]: `None` means "no usable JSON
/// response" (transport failure, non-2xx after retries, or parse
/// failure) — callers do not distinguish the underlying cause.
///
/// The `Debug` supertrait lets adapter structs that hold an
/// `Arc<dyn JsonFetcher>` keep their existing `#[derive(Debug)]`.
#[async_trait]
pub trait JsonFetcher: std::fmt::Debug + Send + Sync {
    async fn fetch_json(&self, url: &str) -> Option<serde_json::Value>;
}

/// Production [`JsonFetcher`] backed by the shared [`reqwest::Client`]
/// and an [`HttpRetryPolicy`].
///
/// Stateless modulo the retry policy; cheap to clone (the underlying
/// HTTP client is a process-wide `OnceLock<Client>`).
#[derive(Debug, Clone)]
pub struct HttpJsonFetcher {
    retry: HttpRetryPolicy,
}

impl Default for HttpJsonFetcher {
    fn default() -> Self {
        Self::with_retry(retry_policy())
    }
}

impl HttpJsonFetcher {
    /// Override the retry policy. Useful for tests that want a fast
    /// fail-path; production should stick with [`retry_policy`].
    pub fn with_retry(retry: HttpRetryPolicy) -> Self {
        Self { retry }
    }
}

#[async_trait]
impl JsonFetcher for HttpJsonFetcher {
    async fn fetch_json(&self, url: &str) -> Option<serde_json::Value> {
        fetch_json_with(url, &self.retry).await
    }
}

/// In-memory [`JsonFetcher`] for unit tests. Records every URL it was
/// asked to fetch and returns canned responses (or `None` for unknown
/// URLs / configured failures). Cheap, deterministic, and side-effect
/// free; prefer this over a real wiremock server for adapter unit
/// tests since you only care about the URL and the JSON shape.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct MockJsonFetcher {
    responses: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    fail_urls: std::sync::Mutex<std::collections::HashSet<String>>,
    captured_urls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl MockJsonFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `url` resolve to `response`.
    pub fn add_response(&self, url: impl Into<String>, response: serde_json::Value) {
        self.responses.lock().unwrap().insert(url.into(), response);
    }

    /// Make `url` resolve to `None` (simulates a 5xx-after-retries,
    /// timeout, or parse failure — all of which look the same to a
    /// caller of `fetch_json`).
    pub fn add_failure(&self, url: impl Into<String>) {
        self.fail_urls.lock().unwrap().insert(url.into());
    }

    /// URLs that the adapter has hit, in call order.
    pub fn captured_urls(&self) -> Vec<String> {
        self.captured_urls.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl JsonFetcher for MockJsonFetcher {
    async fn fetch_json(&self, url: &str) -> Option<serde_json::Value> {
        self.captured_urls.lock().unwrap().push(url.to_string());
        if self.fail_urls.lock().unwrap().contains(url) {
            return None;
        }
        self.responses.lock().unwrap().get(url).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fast_retry() -> HttpRetryPolicy {
        retry_policy()
            .with_initial_backoff(Duration::from_millis(1))
            .with_max_wait(Duration::from_millis(5))
    }

    #[tokio::test]
    async fn fetch_json_succeeds_on_first_try() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[tokio::test]
    async fn retries_on_503_then_succeeds() {
        let server = MockServer::start().await;
        // First 2 requests: 503; next: 200.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[tokio::test]
    async fn retries_on_429_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_on_503() {
        let server = MockServer::start().await;
        // 3 retries in fast_retry; expect exactly 4 calls before giveup.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(4)
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn does_not_retry_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn does_not_retry_on_400() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn returns_none_on_malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn retry_after_beyond_the_cap_still_retries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .mount(&server)
            .await;

        let url = format!("{}/x", server.uri());
        let json = fetch_json_with(&url, &fast_retry()).await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[test]
    fn retry_policy_retries() {
        assert_eq!(retry_policy().retries(), RETRIES);
    }

    #[test]
    fn http_client_is_shared_singleton() {
        let a = http_client();
        let b = http_client();
        assert!(std::ptr::eq(a, b));
    }

    // === JsonFetcher / HttpJsonFetcher / MockJsonFetcher ==================

    #[tokio::test]
    async fn http_json_fetcher_returns_some_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let fetcher = HttpJsonFetcher::with_retry(fast_retry());
        let json = fetcher.fetch_json(&url).await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[tokio::test]
    async fn http_json_fetcher_returns_none_on_unreachable() {
        // A port we don't listen on. Connect-error → None after retries.
        let fetcher = HttpJsonFetcher::with_retry(fast_retry());
        let json = fetcher.fetch_json("http://127.0.0.1:1/never").await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn mock_json_fetcher_returns_canned_response() {
        let f = MockJsonFetcher::new();
        f.add_response("https://example.test/x", serde_json::json!({"ok": 1}));
        let json = f.fetch_json("https://example.test/x").await;
        assert_eq!(json, Some(serde_json::json!({"ok": 1})));
    }

    #[tokio::test]
    async fn mock_json_fetcher_returns_none_for_unknown_url() {
        let f = MockJsonFetcher::new();
        f.add_response("https://example.test/known", serde_json::json!({"ok": 1}));
        let json = f.fetch_json("https://example.test/unknown").await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn mock_json_fetcher_returns_none_for_configured_failure() {
        let f = MockJsonFetcher::new();
        // A failure entry takes precedence over a response on the same URL.
        f.add_response("https://example.test/x", serde_json::json!({"ok": 1}));
        f.add_failure("https://example.test/x");
        let json = f.fetch_json("https://example.test/x").await;
        assert_eq!(json, None);
    }

    #[tokio::test]
    async fn mock_json_fetcher_captures_urls_in_order() {
        let f = MockJsonFetcher::new();
        f.add_response("https://example.test/a", serde_json::json!({}));
        f.add_response("https://example.test/b", serde_json::json!({}));
        let _ = f.fetch_json("https://example.test/a").await;
        let _ = f.fetch_json("https://example.test/b").await;
        let _ = f.fetch_json("https://example.test/c").await; // unknown → None
        assert_eq!(
            f.captured_urls(),
            vec![
                "https://example.test/a".to_string(),
                "https://example.test/b".to_string(),
                "https://example.test/c".to_string(),
            ]
        );
    }
}
