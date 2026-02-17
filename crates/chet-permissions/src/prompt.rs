//! Prompt handler trait for interactive permission prompts.

use crate::types::PromptResponse;
use std::future::Future;
use std::pin::Pin;

/// Trait for handling interactive permission prompts.
///
/// Uses `Pin<Box<dyn Future>>` for dyn-compatibility, matching the Tool trait pattern.
pub trait PromptHandler: Send + Sync {
    /// Prompt the user for a permission decision.
    ///
    /// Displays the tool name, input summary, and description, then waits for the user's response.
    fn prompt_permission(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        description: &str,
    ) -> Pin<Box<dyn Future<Output = PromptResponse> + Send + '_>>;
}
