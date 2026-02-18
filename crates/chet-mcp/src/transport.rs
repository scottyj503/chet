//! Stdio transport for MCP server communication.
//!
//! Spawns a child process and manages async communication over stdin/stdout
//! using newline-delimited JSON-RPC messages.

use crate::error::McpError;
use crate::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

/// Async stdio transport for communicating with an MCP server process.
pub struct StdioTransport {
    next_id: AtomicU64,
    write_tx: mpsc::Sender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    reader_handle: JoinHandle<()>,
    writer_handle: JoinHandle<()>,
    child: Arc<Mutex<Child>>,
    timeout_ms: u64,
}

impl StdioTransport {
    /// Spawn a child process and start background reader/writer tasks.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| McpError::SpawnFailed {
            name: command.to_string(),
            source: e,
        })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Writer task: drains channel and writes to child stdin
        let (write_tx, mut write_rx) = mpsc::channel::<String>(64);
        let writer_handle = tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(msg) = write_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Reader task: reads lines from stdout, parses JSON-RPC, dispatches
        let pending_for_reader = Arc::clone(&pending);
        let reader_handle = tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let resp: JsonRpcResponse = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("Failed to parse MCP response: {e}: {line}");
                        continue;
                    }
                };
                if let Some(id) = resp.id {
                    let mut pending = pending_for_reader.lock().await;
                    if let Some(tx) = pending.remove(&id) {
                        let _ = tx.send(resp);
                    }
                }
                // Notifications from server (no id) are currently ignored
            }
        });

        Ok(Self {
            next_id: AtomicU64::new(1),
            write_tx,
            pending,
            reader_handle,
            writer_handle,
            child: Arc::new(Mutex::new(child)),
            timeout_ms,
        })
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let serialized = serde_json::to_string(&request)?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        self.write_tx
            .send(serialized)
            .await
            .map_err(|_| McpError::Protocol("Writer channel closed".to_string()))?;

        match tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(McpError::Protocol("Response channel dropped".to_string())),
            Err(_) => {
                // Clean up pending entry on timeout
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(McpError::Timeout {
                    name: method.to_string(),
                    timeout_ms: self.timeout_ms,
                })
            }
        }
    }

    /// Send a JSON-RPC notification (fire-and-forget, no response expected).
    pub async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), McpError> {
        let notification = JsonRpcNotification::new(method, params);
        let serialized = serde_json::to_string(&notification)?;

        self.write_tx
            .send(serialized)
            .await
            .map_err(|_| McpError::Protocol("Writer channel closed".to_string()))?;

        Ok(())
    }

    /// Shut down the transport: drop the write channel, wait briefly, then kill.
    pub async fn shutdown(self) {
        // Drop write channel to send EOF to child stdin
        drop(self.write_tx);

        let child = self.child;

        // Give the child 5 seconds to exit gracefully
        let graceful = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut child = child.lock().await;
            let _ = child.wait().await;
        })
        .await;

        if graceful.is_err() {
            // Force kill if it didn't exit
            let mut child = child.lock().await;
            let _ = child.kill().await;
        }

        self.reader_handle.abort();
        self.writer_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_echo_process() {
        // Use `cat` as a simple echo process
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new(), 5000);
        assert!(transport.is_ok());
        let transport = transport.unwrap();
        transport.shutdown().await;
    }

    #[tokio::test]
    async fn spawn_nonexistent_command_fails() {
        let result = StdioTransport::spawn(
            "this_command_does_not_exist_xyz123",
            &[],
            &HashMap::new(),
            5000,
        );
        match result {
            Err(McpError::SpawnFailed { name, .. }) => {
                assert_eq!(name, "this_command_does_not_exist_xyz123");
            }
            Err(other) => panic!("Expected SpawnFailed, got: {other:?}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn request_response_roundtrip_with_mock() {
        // Create a mock MCP server using a bash script that echoes JSON-RPC responses
        let script = r#"while IFS= read -r line; do id=$(echo "$line" | python3 -c "import sys,json; print(json.loads(sys.stdin.read())['id'])"); echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"ok\":true}}"; done"#;
        let transport = StdioTransport::spawn(
            "bash",
            &["-c".to_string(), script.to_string()],
            &HashMap::new(),
            5000,
        );

        if transport.is_err() {
            // Skip test if bash/python3 not available
            return;
        }
        let transport = transport.unwrap();

        let resp = transport
            .send_request("test/method", Some(serde_json::json!({})))
            .await;
        assert!(resp.is_ok());
        let resp = resp.unwrap();
        assert_eq!(resp.result.unwrap()["ok"], true);

        transport.shutdown().await;
    }

    #[tokio::test]
    async fn notification_does_not_block() {
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new(), 5000).unwrap();

        let result = transport
            .send_notification("notifications/initialized", None)
            .await;
        assert!(result.is_ok());

        transport.shutdown().await;
    }

    #[tokio::test]
    async fn timeout_fires_on_unresponsive_server() {
        // `sleep` never writes to stdout, so requests will time out
        let transport =
            StdioTransport::spawn("sleep", &["10".to_string()], &HashMap::new(), 100).unwrap();

        let result = transport
            .send_request("test/method", Some(serde_json::json!({})))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::Timeout { timeout_ms, .. } => {
                assert_eq!(timeout_ms, 100);
            }
            other => panic!("Expected Timeout, got: {other:?}"),
        }

        transport.shutdown().await;
    }
}
