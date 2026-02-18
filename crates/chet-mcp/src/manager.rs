//! MCP manager — orchestrates multiple MCP server connections.

use crate::client::{McpClient, McpToolInfo};
use crate::config::McpConfig;
use std::sync::Arc;

/// Manages connections to multiple MCP servers.
pub struct McpManager {
    clients: Vec<Arc<McpClient>>,
}

impl McpManager {
    /// Start all configured MCP servers.
    ///
    /// Servers that fail to start are logged and skipped — the session continues
    /// with whatever servers are available.
    pub async fn start(config: &McpConfig) -> Self {
        let mut clients = Vec::new();

        for (name, server_config) in &config.servers {
            match McpClient::connect(name.clone(), server_config).await {
                Ok(client) => {
                    tracing::info!(
                        "MCP server '{}' started ({} tools)",
                        name,
                        client.tools().len()
                    );
                    clients.push(Arc::new(client));
                }
                Err(e) => {
                    tracing::warn!("Failed to start MCP server '{}': {}", name, e);
                    eprintln!("Warning: MCP server '{name}' failed to start: {e}");
                }
            }
        }

        Self { clients }
    }

    /// Get all tools from all connected servers, paired with their client.
    pub fn tools(&self) -> Vec<(Arc<McpClient>, McpToolInfo)> {
        let mut all_tools = Vec::new();
        for client in &self.clients {
            for tool in client.tools() {
                all_tools.push((Arc::clone(client), tool.clone()));
            }
        }
        all_tools
    }

    /// Number of connected servers.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Get a summary of connected servers and their tool counts.
    pub fn server_summary(&self) -> Vec<(&str, usize)> {
        self.clients
            .iter()
            .map(|c| (c.server_name(), c.tools().len()))
            .collect()
    }

    /// Shut down all connected servers.
    pub async fn shutdown(self) {
        for client in self.clients {
            // Try to unwrap the Arc; if other references exist, skip
            if let Ok(client) = Arc::try_unwrap(client) {
                client.shutdown().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_config_starts_no_servers() {
        let config = McpConfig::default();
        let manager = McpManager::start(&config).await;
        assert_eq!(manager.client_count(), 0);
        assert!(manager.tools().is_empty());
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_server_is_skipped() {
        let mut config = McpConfig::default();
        config.servers.insert(
            "bad".to_string(),
            crate::config::McpServerConfig {
                command: "nonexistent_command_xyz123".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
                timeout_ms: 1000,
            },
        );
        let manager = McpManager::start(&config).await;
        assert_eq!(manager.client_count(), 0);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn server_summary_empty() {
        let config = McpConfig::default();
        let manager = McpManager::start(&config).await;
        assert!(manager.server_summary().is_empty());
        manager.shutdown().await;
    }
}
