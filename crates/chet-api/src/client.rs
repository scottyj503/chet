//! Anthropic Messages API client.

use chet_types::{ApiError, CreateMessageRequest};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

use crate::stream::MessageStream;

/// The Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Client for the Anthropic Messages API.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
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
        })
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

        tracing::debug!("POST {url}");

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ApiError::Timeout
                } else {
                    ApiError::Network(e.to_string())
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(classify_error(status.as_u16(), &body_text));
        }

        Ok(MessageStream::new(response.bytes_stream()))
    }
}

/// Classify an HTTP error response into a typed ApiError.
fn classify_error(status: u16, body: &str) -> ApiError {
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
            retry_after_ms: None,
        },
        529 => ApiError::Overloaded,
        500..=599 => ApiError::Server { status, message },
        _ => ApiError::Server { status, message },
    }
}
