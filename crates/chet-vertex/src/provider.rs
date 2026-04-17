//! VertexProvider — implements the Provider trait for Google Vertex AI.

use crate::auth::GoogleAuth;
use chet_types::sse::{SseParser, parse_stream_event};
use chet_types::{
    ApiError, CreateMessageRequest, StreamEvent,
    provider::{EventStream, Provider},
};
use futures_util::StreamExt;
use std::future::Future;
use std::pin::Pin;

/// Google Vertex AI provider for Claude models.
pub struct VertexProvider {
    project_id: String,
    region: String,
    auth: GoogleAuth,
    client: reqwest::Client,
}

impl VertexProvider {
    /// Create a new VertexProvider.
    pub fn new(project_id: &str, region: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            region: region.to_string(),
            auth: GoogleAuth::new(),
            client: reqwest::Client::new(),
        }
    }

    /// Build the Vertex AI streaming endpoint URL.
    fn endpoint_url(&self, model: &str) -> String {
        format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:streamRawPredict",
            region = self.region,
            project = self.project_id,
            model = model,
        )
    }
}

impl Provider for VertexProvider {
    fn name(&self) -> &str {
        "vertex"
    }

    fn create_message_stream<'a>(
        &'a self,
        request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let url = self.endpoint_url(&request.model);

            // Build the request body — same as Anthropic but with anthropic_version
            let mut body_value =
                serde_json::to_value(request).map_err(|e| ApiError::BadRequest {
                    message: format!("Failed to serialize request: {e}"),
                })?;
            body_value["anthropic_version"] = serde_json::json!("vertex-2023-10-16");

            let body = serde_json::to_string(&body_value).map_err(|e| ApiError::BadRequest {
                message: format!("Failed to serialize request: {e}"),
            })?;

            // Get access token
            let token = self
                .auth
                .access_token()
                .await
                .map_err(|e| ApiError::Auth { message: e })?;

            let response = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
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

            let status = response.status().as_u16();
            if status != 200 {
                let body = response.text().await.unwrap_or_default();
                return Err(match status {
                    401 | 403 => ApiError::Auth {
                        message: format!("Vertex AI auth error ({status}): {body}"),
                    },
                    429 => ApiError::RateLimited {
                        retry_after_ms: None,
                    },
                    529 => ApiError::Overloaded,
                    s if s >= 500 => ApiError::Server {
                        status: s,
                        message: body,
                    },
                    _ => ApiError::BadRequest { message: body },
                });
            }

            // Vertex uses the same SSE wire format as Anthropic direct API
            let byte_stream = response.bytes_stream();
            let event_stream = byte_stream
                .scan(SseParser::new(), |parser, chunk| {
                    let events: Vec<Result<StreamEvent, ApiError>> = match chunk {
                        Ok(bytes) => {
                            let text = String::from_utf8_lossy(&bytes);
                            parser
                                .feed(&text)
                                .into_iter()
                                .filter_map(|sse| {
                                    match parse_stream_event(&sse.event_type, &sse.data) {
                                        Ok(Some(event)) => Some(Ok(event)),
                                        Ok(None) => None,
                                        Err(e) => Some(Err(e)),
                                    }
                                })
                                .collect()
                        }
                        Err(e) => vec![Err(ApiError::Network(e.to_string()))],
                    };
                    std::future::ready(Some(futures_util::stream::iter(events)))
                })
                .flatten();

            Ok(Box::pin(event_stream) as EventStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_format() {
        let provider = VertexProvider::new("my-project-123", "us-east5");
        let url = provider.endpoint_url("claude-sonnet-4-5-20250929");
        assert_eq!(
            url,
            "https://us-east5-aiplatform.googleapis.com/v1/projects/my-project-123/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-5-20250929:streamRawPredict"
        );
    }

    #[test]
    fn provider_name() {
        let provider = VertexProvider::new("proj", "us-east5");
        assert_eq!(provider.name(), "vertex");
    }

    #[test]
    fn provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VertexProvider>();
    }
}
