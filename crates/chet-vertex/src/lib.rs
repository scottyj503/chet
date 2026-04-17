//! Google Vertex AI provider for Chet.
//!
//! Implements the `Provider` trait for Claude models hosted on Vertex AI.
//! Uses Google Application Default Credentials (ADC) for authentication
//! and the same SSE wire format as the Anthropic direct API.

mod auth;
mod provider;

pub use provider::VertexProvider;
