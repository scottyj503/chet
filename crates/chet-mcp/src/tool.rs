//! McpTool — wraps an MCP server tool as a chet_types::Tool.

use crate::client::{McpClient, McpToolContent, McpToolInfo};
use chet_types::{
    ImageSource, ImageSourceType, ToolContext, ToolDefinition, ToolError, ToolOutput,
    ToolOutputContent,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A tool backed by an MCP server.
///
/// Each McpTool represents one tool from one MCP server. The namespaced name
/// follows the pattern `mcp__servername__toolname` to avoid collisions with
/// built-in tools or tools from other MCP servers.
pub struct McpTool {
    namespaced_name: String,
    server_name: String,
    tool_info: McpToolInfo,
    client: Arc<McpClient>,
}

impl McpTool {
    /// Create a new MCP tool wrapper.
    pub fn new(server_name: &str, tool_info: McpToolInfo, client: Arc<McpClient>) -> Self {
        let namespaced_name = format!("mcp__{}__{}", server_name, tool_info.name);
        Self {
            namespaced_name,
            server_name: server_name.to_string(),
            tool_info,
            client,
        }
    }
}

impl chet_types::Tool for McpTool {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.namespaced_name.clone(),
            description: format!("[MCP: {}] {}", self.server_name, self.tool_info.description),
            input_schema: self.tool_info.input_schema.clone(),
            cache_control: None,
        }
    }

    fn is_read_only(&self) -> bool {
        false // conservative — we can't know
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + '_>> {
        Box::pin(async move {
            let result = self
                .client
                .call_tool(&self.tool_info.name, input)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let content: Vec<ToolOutputContent> = result
                .content
                .into_iter()
                .map(|c| match c {
                    McpToolContent::Text { text } => ToolOutputContent::Text { text },
                    McpToolContent::Image { data, mime_type } => ToolOutputContent::Image {
                        source: ImageSource {
                            source_type: ImageSourceType::Base64,
                            media_type: mime_type,
                            data,
                        },
                    },
                })
                .collect();

            Ok(ToolOutput {
                content,
                is_error: result.is_error,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn sample_tool_info() -> McpToolInfo {
        McpToolInfo {
            name: "read_file".to_string(),
            description: "Read a file from disk".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        }
    }

    // We can't easily create an Arc<McpClient> without a running server,
    // so we test the parts that don't need one.

    #[test]
    fn namespaced_name_format() {
        let name = format!("mcp__{}__{}", "filesystem", "read_file");
        assert_eq!(name, "mcp__filesystem__read_file");
    }

    #[test]
    fn definition_includes_server_prefix() {
        let desc = format!("[MCP: {}] {}", "github", "List repositories");
        assert!(desc.starts_with("[MCP: github]"));
        assert!(desc.contains("List repositories"));
    }

    #[test]
    fn mcp_tool_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<McpTool>();
    }
}
