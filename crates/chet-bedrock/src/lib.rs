//! AWS Bedrock provider for Chet.
//!
//! Implements the `Provider` trait for Claude models hosted on AWS Bedrock.
//! Uses SigV4 signing for authentication and AWS EventStream binary framing
//! for streaming responses.

pub mod eventstream;
pub mod provider;

pub use provider::BedrockProvider;
