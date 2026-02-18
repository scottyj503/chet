//! Anthropic Messages API provider implementation.

use crate::client::ApiClient;
use crate::retry::RetryConfig;
use chet_types::provider::{EventStream, Provider};
use chet_types::{ApiError, CreateMessageRequest};
use std::future::Future;
use std::pin::Pin;

/// Anthropic Messages API provider.
///
/// Wraps `ApiClient` and implements the `Provider` trait, delegating all
/// calls to the underlying client. Retry logic stays in `ApiClient`.
#[derive(Clone)]
pub struct AnthropicProvider {
    client: ApiClient,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Result<Self, ApiError> {
        Ok(Self {
            client: ApiClient::new(api_key, base_url)?,
        })
    }

    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.client = self.client.with_retry_config(config);
        self
    }
}

impl Provider for AnthropicProvider {
    fn create_message_stream<'a>(
        &'a self,
        request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let stream = self.client.create_message_stream(request).await?;
            Ok(Box::pin(stream) as EventStream)
        })
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_provider_new() {
        let provider = AnthropicProvider::new("test-key", "https://api.example.com");
        assert!(provider.is_ok());
    }

    #[test]
    fn anthropic_provider_name() {
        let provider = AnthropicProvider::new("test-key", "https://api.example.com").unwrap();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn anthropic_provider_with_retry() {
        let provider = AnthropicProvider::new("test-key", "https://api.example.com")
            .unwrap()
            .with_retry_config(RetryConfig {
                max_retries: 5,
                ..RetryConfig::default()
            });
        assert_eq!(provider.name(), "anthropic");
    }
}
