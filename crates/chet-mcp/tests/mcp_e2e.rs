//! End-to-end MCP integration test.
//!
//! Spawns an inline Python script as a minimal MCP server, then exercises the
//! full `McpClient` pipeline: connect (initialize + initialized + tools/list),
//! call a tool, and shut down.
//!
//! Run with: `cargo test -p chet-mcp --test mcp_e2e -- --ignored`

use chet_mcp::client::McpToolContent;
use chet_mcp::{McpClient, McpServerConfig};
use std::collections::HashMap;

/// Inline Python script that implements a minimal MCP server.
///
/// Handles three JSON-RPC methods:
/// - `initialize` → returns server info + capabilities
/// - `tools/list` → returns one tool: "echo" that echoes its input
/// - `tools/call` → for "echo", returns the arguments as text; otherwise error
const MCP_SERVER_SCRIPT: &str = r#"
import sys, json

def respond(id, result=None, error=None):
    resp = {"jsonrpc": "2.0", "id": id}
    if result is not None:
        resp["result"] = result
    if error is not None:
        resp["error"] = error
    print(json.dumps(resp), flush=True)

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    # Notifications have no id — ignore them
    if "id" not in msg:
        continue

    req_id = msg["id"]
    method = msg.get("method", "")

    if method == "initialize":
        respond(req_id, result={
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "test-mcp-server", "version": "0.1.0"}
        })
    elif method == "tools/list":
        respond(req_id, result={
            "tools": [{
                "name": "echo",
                "description": "Echoes the input message back",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "message": {"type": "string", "description": "Message to echo"}
                    },
                    "required": ["message"]
                }
            }]
        })
    elif method == "tools/call":
        params = msg.get("params", {})
        tool_name = params.get("name", "")
        arguments = params.get("arguments", {})
        if tool_name == "echo":
            respond(req_id, result={
                "content": [{"type": "text", "text": arguments.get("message", "")}],
                "isError": False
            })
        else:
            respond(req_id, result={
                "content": [{"type": "text", "text": f"Unknown tool: {tool_name}"}],
                "isError": True
            })
    else:
        respond(req_id, error={"code": -32601, "message": f"Method not found: {method}"})
"#;

fn server_config() -> McpServerConfig {
    McpServerConfig {
        command: "python3".to_string(),
        args: vec!["-c".to_string(), MCP_SERVER_SCRIPT.to_string()],
        env: HashMap::new(),
        timeout_ms: 5000,
    }
}

/// Full handshake: connect discovers the "echo" tool.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn test_mcp_connect_and_discover_tools() {
    let config = server_config();
    let client = McpClient::connect("test-server".to_string(), &config)
        .await
        .expect("connect should succeed");

    let tools = client.tools();
    assert_eq!(tools.len(), 1, "server exposes one tool");
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echoes the input message back");
    assert!(tools[0].input_schema["properties"]["message"].is_object());

    client.shutdown().await;
}

/// Full pipeline: connect → call_tool → verify result → shutdown.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn test_mcp_call_tool() {
    let config = server_config();
    let client = McpClient::connect("test-server".to_string(), &config)
        .await
        .expect("connect should succeed");

    let result = client
        .call_tool("echo", serde_json::json!({"message": "hello world"}))
        .await
        .expect("call_tool should succeed");

    assert!(!result.is_error, "tool call should not be an error");
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        McpToolContent::Text { text } => {
            assert_eq!(text, "hello world");
        }
        other => panic!("Expected text content, got: {other:?}"),
    }

    client.shutdown().await;
}

/// Calling an unknown tool returns is_error=true.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn test_mcp_call_unknown_tool() {
    let config = server_config();
    let client = McpClient::connect("test-server".to_string(), &config)
        .await
        .expect("connect should succeed");

    let result = client
        .call_tool("nonexistent", serde_json::json!({}))
        .await
        .expect("call_tool should return a result (not transport error)");

    assert!(result.is_error, "unknown tool should be an error");
    match &result.content[0] {
        McpToolContent::Text { text } => {
            assert!(
                text.contains("nonexistent"),
                "error should mention tool name: {text}"
            );
        }
        other => panic!("Expected text content, got: {other:?}"),
    }

    client.shutdown().await;
}
