//! Configuration types for MCP servers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_timeout() -> u64 {
    30000
}

/// Top-level MCP configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to run (e.g., "npx", "python").
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Timeout for requests in milliseconds (default: 30000).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_server() {
        let toml_str = r#"
[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]
"#;
        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.servers.len(), 1);
        let fs = &config.servers["filesystem"];
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args.len(), 3);
        assert_eq!(fs.timeout_ms, 30000); // default
    }

    #[test]
    fn parse_multiple_servers() {
        let toml_str = r#"
[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]

[servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
timeout_ms = 60000
"#;
        let config: McpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers["github"].timeout_ms, 60000);
    }

    #[test]
    fn parse_env_vars() {
        let toml_str = r#"
[servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_xxxx" }
"#;
        let config: McpConfig = toml::from_str(toml_str).unwrap();
        let gh = &config.servers["github"];
        assert_eq!(gh.env["GITHUB_TOKEN"], "ghp_xxxx");
    }

    #[test]
    fn default_config_is_empty() {
        let config = McpConfig::default();
        assert!(config.servers.is_empty());
    }
}
