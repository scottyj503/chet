//! MCP client — manages one server connection.
//!
//! Handles the MCP protocol handshake (initialize + initialized notification),
//! tool discovery (tools/list), and tool invocation (tools/call).

use crate::config::McpServerConfig;
use crate::error::McpError;
use crate::transport::StdioTransport;
use serde::Deserialize;

/// MCP protocol version we support.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Information about a tool exposed by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Result of calling a tool on an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolResult {
    pub content: Vec<McpToolContent>,
    pub is_error: bool,
}

/// A content item in a tool result.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpToolContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Client for a single MCP server.
pub struct McpClient {
    name: String,
    transport: StdioTransport,
    tools: Vec<McpToolInfo>,
}

/// Deserialization helpers for MCP protocol messages.
#[derive(Deserialize)]
struct ToolsListResult {
    tools: Vec<ToolEntry>,
}

#[derive(Deserialize)]
struct ToolEntry {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_schema", rename = "inputSchema")]
    input_schema: serde_json::Value,
}

fn default_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Deserialize)]
struct ToolCallResult {
    content: Vec<McpToolContent>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

impl McpClient {
    /// Connect to an MCP server: spawn, handshake, discover tools.
    pub async fn connect(name: String, config: &McpServerConfig) -> Result<Self, McpError> {
        let transport = StdioTransport::spawn(
            &config.command,
            &config.args,
            &config.env,
            config.timeout_ms,
        )?;

        // Send `initialize` request
        let init_params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "chet",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let resp = transport
            .send_request("initialize", Some(init_params))
            .await?;

        if let Some(err) = resp.error {
            return Err(McpError::JsonRpc {
                server: name,
                code: err.code,
                message: err.message,
            });
        }

        // Send `notifications/initialized`
        transport
            .send_notification("notifications/initialized", None)
            .await?;

        // Discover tools via `tools/list`
        let tools_resp = transport.send_request("tools/list", None).await?;

        let tools = if let Some(result) = tools_resp.result {
            let list: ToolsListResult = serde_json::from_value(result).map_err(|e| {
                McpError::Protocol(format!("Failed to parse tools/list response: {e}"))
            })?;
            list.tools
                .into_iter()
                .map(|t| McpToolInfo {
                    name: t.name,
                    description: t.description.unwrap_or_default(),
                    input_schema: t.input_schema,
                })
                .collect()
        } else if let Some(err) = tools_resp.error {
            return Err(McpError::JsonRpc {
                server: name,
                code: err.code,
                message: err.message,
            });
        } else {
            Vec::new()
        };

        tracing::info!("MCP server '{}' connected with {} tools", name, tools.len());

        Ok(Self {
            name,
            transport,
            tools,
        })
    }

    /// Call a tool on this server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self
            .transport
            .send_request("tools/call", Some(params))
            .await?;

        if let Some(err) = resp.error {
            return Err(McpError::JsonRpc {
                server: self.name.clone(),
                code: err.code,
                message: err.message,
            });
        }

        let result = resp.result.ok_or_else(|| {
            McpError::Protocol("tools/call response has neither result nor error".to_string())
        })?;

        let call_result: ToolCallResult = serde_json::from_value(result)
            .map_err(|e| McpError::Protocol(format!("Failed to parse tools/call result: {e}")))?;

        Ok(McpToolResult {
            content: call_result.content,
            is_error: call_result.is_error,
        })
    }

    /// Get the tools exposed by this server.
    pub fn tools(&self) -> &[McpToolInfo] {
        &self.tools
    }

    /// Get the server name.
    pub fn server_name(&self) -> &str {
        &self.name
    }

    /// Shut down the server connection.
    pub async fn shutdown(self) {
        self.transport.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_tool_entry() {
        let json = r#"{
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }
        }"#;
        let entry: ToolEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.name, "read_file");
        assert_eq!(entry.description.as_deref(), Some("Read a file"));
    }

    #[test]
    fn deserialize_tool_entry_without_description() {
        let json = r#"{
            "name": "list",
            "inputSchema": {"type": "object"}
        }"#;
        let entry: ToolEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.name, "list");
        assert!(entry.description.is_none());
    }

    #[test]
    fn deserialize_tool_call_result_text() {
        let json = r#"{
            "content": [{"type": "text", "text": "file contents here"}],
            "isError": false
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 1);
        assert!(!result.is_error);
        match &result.content[0] {
            McpToolContent::Text { text } => assert_eq!(text, "file contents here"),
            _ => panic!("Expected text content"),
        }
    }

    #[test]
    fn deserialize_tool_call_result_error() {
        let json = r#"{
            "content": [{"type": "text", "text": "not found"}],
            "isError": true
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn deserialize_tool_call_result_image() {
        let json = r#"{
            "content": [{"type": "image", "data": "base64data", "mimeType": "image/png"}],
            "isError": false
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        match &result.content[0] {
            McpToolContent::Image { data, mime_type } => {
                assert_eq!(data, "base64data");
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("Expected image content"),
        }
    }

    #[test]
    fn deserialize_tools_list_result() {
        let json = r#"{
            "tools": [
                {"name": "a", "description": "Tool A", "inputSchema": {"type": "object"}},
                {"name": "b", "inputSchema": {"type": "object"}}
            ]
        }"#;
        let result: ToolsListResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.tools.len(), 2);
        assert_eq!(result.tools[0].name, "a");
        assert!(result.tools[1].description.is_none());
    }
}
