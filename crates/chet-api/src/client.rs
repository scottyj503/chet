//! Anthropic Messages API client.

use std::time::Duration;

use chet_types::{ApiError, CreateMessageRequest};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

use crate::retry::{RetryConfig, calculate_delay, is_retryable};
use crate::stream::MessageStream;

/// The Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Client for the Anthropic Messages API.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    retry_config: RetryConfig,
}

impl ApiClient {
    /// Create a new API client.
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| ApiError::Network(e.to_string()))?;

        Ok(Self {
            http,
            api_key: api_key.into(),
            base_url: base_url.into(),
            retry_config: RetryConfig::default(),
        })
    }

    /// Set the retry configuration for transient errors (429, 529, 5xx, network).
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Send a streaming Messages API request and return a stream of events.
    pub async fn create_message_stream(
        &self,
        request: &CreateMessageRequest,
    ) -> Result<MessageStream, ApiError> {
        let url = format!("{}/v1/messages", self.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).map_err(|_| ApiError::Auth {
                message: "Invalid API key format".into(),
            })?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );

        let body = serde_json::to_string(request).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to serialize request: {e}"),
        })?;

        for attempt in 0..=self.retry_config.max_retries {
            tracing::debug!(
                "POST {url} (attempt {}/{})",
                attempt + 1,
                self.retry_config.max_retries + 1
            );

            let result = self
                .http
                .post(&url)
                .headers(headers.clone())
                .body(body.clone())
                .send()
                .await;

            match result {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(MessageStream::new(response.bytes_stream()));
                    }

                    let retry_after = parse_retry_after(response.headers());
                    let body_text = response.text().await.unwrap_or_default();
                    let err = classify_error(status.as_u16(), &body_text, retry_after);

                    if !is_retryable(&err) || attempt == self.retry_config.max_retries {
                        return Err(err);
                    }

                    let delay = calculate_delay(&self.retry_config, attempt, retry_after);
                    tracing::warn!(
                        "Retryable API error (attempt {}/{}): {err}. Retrying in {delay}ms...",
                        attempt + 1,
                        self.retry_config.max_retries,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => {
                    let err = if e.is_timeout() {
                        ApiError::Timeout
                    } else {
                        ApiError::Network(e.to_string())
                    };

                    if attempt == self.retry_config.max_retries {
                        return Err(err);
                    }

                    let delay = calculate_delay(&self.retry_config, attempt, None);
                    tracing::warn!(
                        "Retryable network error (attempt {}/{}): {err}. Retrying in {delay}ms...",
                        attempt + 1,
                        self.retry_config.max_retries,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }

        // Unreachable: the loop always returns on the last attempt
        unreachable!("retry loop should have returned")
    }
}

/// Parse the `retry-after` header value as seconds and convert to milliseconds.
fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1000.0) as u64)
}

/// Classify an HTTP error response into a typed ApiError.
fn classify_error(status: u16, body: &str, retry_after: Option<u64>) -> ApiError {
    // Try to parse as JSON error response
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        error: Option<ErrorDetail>,
    }
    #[derive(serde::Deserialize)]
    struct ErrorDetail {
        message: Option<String>,
    }

    let message = serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|b| b.error)
        .and_then(|e| e.message)
        .unwrap_or_else(|| body.to_string());

    match status {
        401 => ApiError::Auth { message },
        400 => ApiError::BadRequest { message },
        429 => ApiError::RateLimited {
            retry_after_ms: retry_after,
        },
        529 => ApiError::Overloaded,
        500..=599 => ApiError::Server { status, message },
        _ => ApiError::Server { status, message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retry_after_integer() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("5"));
        assert_eq!(parse_retry_after(&headers), Some(5000));
    }

    #[test]
    fn parse_retry_after_float() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("1.5"));
        assert_eq!(parse_retry_after(&headers), Some(1500));
    }

    #[test]
    fn parse_retry_after_missing() {
        let headers = HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn parse_retry_after_invalid() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("not-a-number"));
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn classify_error_429_with_retry_after() {
        let err = classify_error(429, "{}", Some(3000));
        match err {
            ApiError::RateLimited { retry_after_ms } => {
                assert_eq!(retry_after_ms, Some(3000));
            }
            _ => panic!("Expected RateLimited, got {err:?}"),
        }
    }

    #[test]
    fn classify_error_529() {
        let err = classify_error(529, "{}", None);
        assert!(matches!(err, ApiError::Overloaded));
    }

    #[test]
    fn classify_error_500() {
        let err = classify_error(500, r#"{"error":{"message":"boom"}}"#, None);
        match err {
            ApiError::Server { status, message } => {
                assert_eq!(status, 500);
                assert_eq!(message, "boom");
            }
            _ => panic!("Expected Server, got {err:?}"),
        }
    }

    #[test]
    fn classify_error_401() {
        let err = classify_error(401, r#"{"error":{"message":"invalid key"}}"#, None);
        assert!(matches!(err, ApiError::Auth { .. }));
    }
}
