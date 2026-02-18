//! Provider trait for LLM API providers.

use crate::{ApiError, CreateMessageRequest, StreamEvent};
use futures_core::Stream;
use std::future::Future;
use std::pin::Pin;

/// A boxed async stream of events from an LLM provider.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ApiError>> + Send>>;

/// Trait for LLM API providers (Anthropic, OpenAI, etc.).
///
/// Providers translate between canonical Chet message types and their
/// native API format. Dyn-compatible so Agent works with `Arc<dyn Provider>`.
pub trait Provider: Send + Sync {
    /// Send a streaming message request, returning a stream of canonical events.
    fn create_message_stream<'a>(
        &'a self,
        request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>>;

    /// Provider name for logging/display (e.g., "anthropic").
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn provider_is_dyn_compatible() {
        // Compile-time check: Provider can be used as a trait object.
        fn _accept(_p: &dyn Provider) {}
    }

    #[test]
    fn arc_provider_is_send_sync() {
        // Compile-time assert: Arc<dyn Provider> is Send + Sync.
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<Arc<dyn Provider>>();
    }
}
