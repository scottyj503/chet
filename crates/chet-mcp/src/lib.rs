//! MCP (Model Context Protocol) client implementation for Chet.
//!
//! Supports stdio-based MCP servers that communicate via newline-delimited
//! JSON-RPC 2.0 messages. Each configured server is spawned as a child process,
//! initialized with a handshake, and its tools are discovered and registered.

pub mod client;
pub mod config;
pub mod error;
pub mod jsonrpc;
pub mod manager;
pub mod tool;
mod transport;

pub use client::{McpClient, McpToolInfo, McpToolResult};
pub use config::{McpConfig, McpServerConfig};
pub use error::McpError;
pub use manager::McpManager;
pub use tool::McpTool;
