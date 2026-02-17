//! Tool trait and related types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::ToolDefinition;

/// Context provided to tools during execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Current working directory for tools that operate on the filesystem.
    pub cwd: PathBuf,
    /// Environment variables available to the tool.
    pub env: HashMap<String, String>,
    /// Whether the tool is running in sandbox mode.
    pub sandboxed: bool,
}

/// Result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// The output content blocks.
    pub content: Vec<ToolOutputContent>,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// A single piece of tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputContent {
    Text { text: String },
    Image { source: crate::ImageSource },
}

impl ToolOutput {
    /// Create a successful text output.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Create an error text output.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: true,
        }
    }
}

/// Trait that all tools must implement.
///
/// Tools are the primary way the AI assistant interacts with the system —
/// reading files, running commands, searching code, etc.
pub trait Tool: Send + Sync {
    /// The unique name of this tool (used in API requests).
    fn name(&self) -> &str;

    /// The tool definition to send to the API (name, description, input schema).
    fn definition(&self) -> ToolDefinition;

    /// Whether this tool only reads data without modifying the system.
    ///
    /// Read-only tools (e.g., Read, Glob, Grep) are auto-permitted by default
    /// when no explicit permission rule matches. Mutating tools require a prompt.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Execute the tool with the given JSON input and context.
    ///
    /// The context is passed by value to avoid lifetime issues with dyn dispatch.
    /// ToolContext is cheap to clone.
    fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, crate::error::ToolError>> + Send + '_>>;
}
