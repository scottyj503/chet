//! Session-specific error types.

use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur during session operations.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Session not found: {id}")]
    NotFound { id: Uuid },

    #[error("Ambiguous session prefix '{prefix}': matches {count} sessions")]
    AmbiguousPrefix { prefix: String, count: usize },

    #[error("No sessions match prefix '{prefix}'")]
    PrefixNotFound { prefix: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Nothing to compact: conversation is too short")]
    NothingToCompact,
}
