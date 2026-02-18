//! Error types for MCP operations.

use thiserror::Error;

/// Errors from MCP server communication.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("Failed to spawn MCP server '{name}': {source}")]
    SpawnFailed {
        name: String,
        source: std::io::Error,
    },

    #[error("MCP server '{name}' is not running")]
    ServerNotRunning { name: String },

    #[error("JSON-RPC error from '{server}' (code {code}): {message}")]
    JsonRpc {
        server: String,
        code: i64,
        message: String,
    },

    #[error("MCP protocol error: {0}")]
    Protocol(String),

    #[error("MCP server '{name}' timed out after {timeout_ms}ms")]
    Timeout { name: String, timeout_ms: u64 },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
