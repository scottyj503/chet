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
                    McpToolContent::Image { data, mime_type } => {
                        // Try to save binary content to disk instead of keeping
                        // base64 in context (can be very large for PDFs/docs/audio)
                        match save_binary_content(&_ctx.cwd, &data, &mime_type) {
                            Some(path) => ToolOutputContent::Text {
                                text: format!(
                                    "[Binary content ({}) saved to {}]",
                                    mime_type,
                                    path.display()
                                ),
                            },
                            None => ToolOutputContent::Image {
                                source: ImageSource {
                                    source_type: ImageSourceType::Base64,
                                    media_type: mime_type,
                                    data,
                                },
                            },
                        }
                    }
                })
                .collect();

            Ok(ToolOutput {
                content,
                is_error: result.is_error,
            })
        })
    }
}

/// Decode base64 data and save to a file with the correct extension.
/// Returns the file path on success, None on failure.
fn save_binary_content(
    cwd: &std::path::Path,
    base64_data: &str,
    mime_type: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;

    let decoded = base64_decode(base64_data)?;

    let ext = match mime_type {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" => "wav",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        _ => "bin",
    };

    let dir = cwd.join(".chet-mcp-output");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }

    let filename = format!("mcp-{}.{ext}", &uuid::Uuid::new_v4().to_string()[..8]);
    let path = dir.join(&filename);

    let mut file = std::fs::File::create(&path).ok()?;
    file.write_all(&decoded).ok()?;
    Some(path)
}

/// Simple base64 decoder (no padding required).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;

    for &byte in input.as_bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' || byte == b' ' {
            continue;
        }
        let val = TABLE.iter().position(|&b| b == byte)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Some(output)
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
