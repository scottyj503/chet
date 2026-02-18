//! Error hierarchy for Chet.

use thiserror::Error;

/// Top-level error type for all Chet operations.
#[derive(Debug, Error)]
pub enum ChetError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Permission denied: {0}")]
    Permission(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

/// Errors from the Anthropic Messages API.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Authentication failed: {message}")]
    Auth { message: String },

    #[error("Bad request: {message}")]
    BadRequest { message: String },

    #[error("Rate limited (retry after {retry_after_ms:?}ms)")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("Server overloaded")]
    Overloaded,

    #[error("Server error: {status} {message}")]
    Server { status: u16, message: String },

    #[error("Network error: {0}")]
    Network(String),

    #[error("Stream parse error: {0}")]
    StreamParse(String),

    #[error("Request timeout")]
    Timeout,
}

/// Errors from tool execution.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Unknown tool: {name}")]
    UnknownTool { name: String },

    #[error("Invalid input for tool '{tool}': {message}")]
    InvalidInput { tool: String, message: String },

    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Tool timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("Tool '{tool}' blocked by permission rule")]
    Blocked { tool: String },
}

/// Errors from configuration loading.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file parse error at {path}: {message}")]
    Parse { path: String, message: String },

    #[error("Missing required configuration: {key}")]
    MissingKey { key: String },

    #[error("Invalid configuration value for '{key}': {message}")]
    InvalidValue { key: String, message: String },
}
