//! Shared HTTP client + retry helper for outbound provider calls.
//!
//! Provides a single process-wide [`reqwest::Client`] (with timeouts and
//! a `User-Agent`) and a [`send_with_retry`] helper that retries
//! transient failures (timeouts, connect errors, HTTP 5xx, HTTP 429)
//! with exponential backoff + jitter.
//!
//! `fetch_json` is the drop-in replacement for the previous pattern
//! `reqwest::get(&url).await.ok()?.json().await.ok()?`.

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use rand::RngExt;
use reqwest::{Client, Response, StatusCode};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 8;

fn build_client() -> Client {
    Client::builder()
        .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
        .timeout(DEFAULT_TIMEOUT)
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .pool_idle_timeout(DEFAULT_POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(DEFAULT_POOL_MAX_IDLE_PER_HOST)
        .build()
        .expect("failed to build shared HTTP client")
}

/// Returns the process-wide HTTP client.
pub fn http_client() -> &'static Client {
    static HTTP: OnceLock<Client> = OnceLock::new();
    HTTP.get_or_init(build_client)
}

/// Retry policy for transient HTTP failures.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(5),
        }
    }
}

fn is_retriable_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
}

fn is_retriable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

/// Sends an HTTP request, retrying transient failures.
///
/// `make_req` is invoked once per attempt and must return a fresh
/// `Future` each call (so the request can be re-sent).
///
/// Retries:
/// - on `reqwest::Error::is_timeout()` or `is_connect()`
/// - on HTTP `5xx` and HTTP `429`
///
/// Does not retry on other 4xx — those are deterministic.
pub async fn send_with_retry<F, Fut>(
    config: &RetryConfig,
    mut make_req: F,
) -> reqwest::Result<Response>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = reqwest::Result<Response>>,
{
    let mut delay = config.base_delay;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let outcome = make_req().await;
        let retriable = match &outcome {
            Ok(resp) => is_retriable_status(resp.status()),
            Err(e) => is_retriable_error(e),
        };
        if !retriable || attempt >= config.max_attempts {
            if attempt > 1 {
                match &outcome {
                    Ok(resp) if is_retriable_status(resp.status()) => {
                        tracing::error!(
                            status = resp.status().as_u16(),
                            attempt,
                            "HTTP gave up on retriable status"
                        );
                    },
                    Err(e) if is_retriable_error(e) => {
                        tracing::error!(error = %e, attempt, "HTTP gave up on retriable error");
                    },
                    _ => {},
                }
            }
            return outcome;
        }
        match &outcome {
            Ok(resp) => tracing::warn!(
                status = resp.status().as_u16(),
                attempt,
                "HTTP transient status, retrying"
            ),
            Err(e) => tracing::warn!(error = %e, attempt, "HTTP transient error, retrying"),
        }
        let cap_ms = delay.as_millis() as u64;
        let jitter_ms = if cap_ms == 0 { 0 } else { rand::rng().random_range(0..=cap_ms) };
        tokio::time::sleep(delay + Duration::from_millis(jitter_ms / 2)).await;
        delay = std::cmp::min(delay.saturating_mul(2), config.max_delay);
    }
}

/// GET `url` and decode the response body as JSON, retrying transient
/// failures with the default [`RetryConfig`].
///
/// Returns `None` on transport failure, non-2xx response (after
/// retries), or JSON parse failure. Logs failures via `tracing`.
pub async fn fetch_json(url: &str) -> Option<serde_json::Value> {
    fetch_json_with(url, &RetryConfig::default()).await
}

pub async fn fetch_json_with(url: &str, config: &RetryConfig) -> Option<serde_json::Value> {
    let client = http_client();
    let resp = match send_with_retry(config, || client.get(url).send()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, url, "HTTP request failed");
            return None;
        },
    };
    if !resp.status().is_success() {
        tracing::warn!(status = resp.status().as_u16(), url, "non-success HTTP response");
        return None;
    }
    match resp.json::<serde_json::Value>().await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, url, "JSON decode failed");
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fast_retry() -> RetryConfig {
        RetryConfig {
            max_attempts: 4,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        }
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
        // max_attempts = 4 in fast_retry; expect exactly 4 calls before giveup.
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

    #[test]
    fn retry_config_default_is_sane() {
        let c = RetryConfig::default();
        assert!(c.max_attempts >= 2);
        assert!(c.base_delay <= c.max_delay);
    }

    #[test]
    fn http_client_is_shared_singleton() {
        let a = http_client();
        let b = http_client();
        assert!(std::ptr::eq(a, b));
    }
}
