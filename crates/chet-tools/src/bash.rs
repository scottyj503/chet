//! Bash tool — executes shell commands.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::process::Command;

/// Maximum output length before truncation (in the returned tool result).
const MAX_OUTPUT_BYTES: usize = 30_000;

/// Maximum total output bytes before killing the process.
/// Prevents runaway processes from filling memory (5 GB).
const MAX_PROCESS_OUTPUT_BYTES: usize = 5 * 1024 * 1024 * 1024;

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Tool for executing bash commands with timeout and output truncation.
pub struct BashTool {
    /// Persistent working directory across calls.
    cwd: Mutex<Option<PathBuf>>,
}

#[derive(Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            cwd: Mutex::new(None),
        }
    }
}

impl BashTool {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Bash".to_string(),
            description: "Execute a bash command. The working directory persists between calls. \
                          Output is truncated at 30K characters. Commands time out after 2 minutes \
                          by default."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (max 600000)"
                    }
                }
            }),
            cache_control: None,
        }
    }

    fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>,
    > {
        Box::pin(async move {
            let input: BashInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "Bash".into(),
                    message: e.to_string(),
                })?;

            let timeout_ms = input.timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(600_000);

            // Get the persistent cwd or fall back to context cwd
            let cwd = {
                let mut lock = self.cwd.lock().unwrap();
                let candidate = lock.clone().unwrap_or_else(|| ctx.cwd.clone());
                if candidate.is_dir() {
                    candidate
                } else {
                    *lock = None;
                    eprintln!(
                        "Warning: CWD {} no longer exists, falling back to {}",
                        candidate.display(),
                        ctx.cwd.display()
                    );
                    ctx.cwd.clone()
                }
            };

            let mut child = Command::new("bash")
                .arg("-c")
                .arg(&input.command)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn command: {e}")))?;

            let result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                read_child_output(&mut child, MAX_PROCESS_OUTPUT_BYTES),
            )
            .await;

            let output = match result {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    let _ = child.kill().await;
                    return Err(ToolError::ExecutionFailed(e));
                }
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(ToolError::Timeout { timeout_ms });
                }
            };

            // Try to detect `cd` commands and update persistent cwd
            // This is a heuristic — we check if the command starts with `cd`
            // and then resolve the new directory
            if let Some(dir) = extract_cd_target(&input.command) {
                let new_cwd = if dir.starts_with('/') {
                    PathBuf::from(dir)
                } else {
                    cwd.join(dir)
                };
                if new_cwd.is_dir() {
                    *self.cwd.lock().unwrap() = Some(new_cwd);
                }
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);

            let mut result_text = String::new();
            if !stdout.is_empty() {
                result_text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result_text.is_empty() {
                    result_text.push('\n');
                }
                result_text.push_str(&stderr);
            }

            if output.killed_for_output_limit {
                result_text.push_str(&format!(
                    "\n\n(process killed: output exceeded {} byte limit)",
                    MAX_PROCESS_OUTPUT_BYTES
                ));
            }

            // Truncate if needed
            if result_text.len() > MAX_OUTPUT_BYTES {
                chet_types::truncate_string(&mut result_text, MAX_OUTPUT_BYTES);
                result_text.push_str("\n\n(output truncated)");
            }

            if exit_code != 0 && result_text.is_empty() {
                result_text = format!("Command exited with code {exit_code}");
            }

            if result_text.is_empty() {
                result_text = "(no output)".to_string();
            }

            Ok(ToolOutput {
                content: vec![chet_types::ToolOutputContent::Text { text: result_text }],
                is_error: exit_code != 0,
            })
        })
    }
}

/// Output captured from a child process.
struct ChildOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: std::process::ExitStatus,
    killed_for_output_limit: bool,
}

/// Read stdout and stderr from a child process, killing it if total output exceeds `max_bytes`.
async fn read_child_output(
    child: &mut tokio::process::Child,
    max_bytes: usize,
) -> Result<ChildOutput, String> {
    let stdout_reader = child.stdout.take().ok_or("No stdout pipe")?;
    let stderr_reader = child.stderr.take().ok_or("No stderr pipe")?;

    let killed;

    // Read stdout and stderr concurrently with bounded reads.
    // Each task reads up to max_bytes to prevent unbounded memory usage.
    let max_per_stream = max_bytes;
    let stdout_handle =
        tokio::spawn(async move { bounded_read(stdout_reader, max_per_stream).await });
    let stderr_handle =
        tokio::spawn(async move { bounded_read(stderr_reader, max_per_stream).await });

    let (stdout_result, stderr_result) = tokio::join!(stdout_handle, stderr_handle);
    let stdout = stdout_result.unwrap_or(Ok(Vec::new())).unwrap_or_default();
    let stderr = stderr_result.unwrap_or(Ok(Vec::new())).unwrap_or_default();

    let total_bytes = stdout.len() + stderr.len();
    if total_bytes > max_bytes {
        let _ = child.kill().await;
        killed = true;
    } else {
        killed = false;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Failed to wait: {e}"))?;

    Ok(ChildOutput {
        stdout,
        stderr,
        status,
        killed_for_output_limit: killed,
    })
}

/// Read from an async reader up to `max_bytes`, then discard the rest.
async fn bounded_read<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < max_bytes {
                    let take = n.min(max_bytes - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                }
                // If over limit, keep reading to drain the pipe but don't store
            }
            Err(e) => return Err(format!("Read error: {e}")),
        }
    }
    Ok(buf)
}

/// Extract the target directory from a simple `cd` command.
fn extract_cd_target(command: &str) -> Option<&str> {
    let trimmed = command.trim();
    if trimmed == "cd" {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("cd ") {
        let target = rest.trim();
        // Only handle simple cases, not `cd foo && something`
        if !target.contains('&') && !target.contains(';') && !target.contains('|') {
            return Some(target);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::collections::HashMap;

    #[cfg(unix)]
    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            env: HashMap::new(),
            sandboxed: false,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool::new();
        let output = tool
            .execute(serde_json::json!({"command": "echo hello"}), test_ctx())
            .await
            .unwrap();

        assert!(!output.is_error);
        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert_eq!(text.trim(), "hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool::new();
        let output = tool
            .execute(serde_json::json!({"command": "exit 1"}), test_ctx())
            .await
            .unwrap();

        assert!(output.is_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bash_timeout() {
        let tool = BashTool::new();
        let result = tool
            .execute(
                serde_json::json!({"command": "sleep 10", "timeout": 100}),
                test_ctx(),
            )
            .await;

        assert!(matches!(result, Err(ToolError::Timeout { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_bash_cwd_deleted_fallback() {
        let tool = BashTool::new();
        let tmp = tempfile::tempdir().unwrap();
        let doomed = tmp.path().join("doomed");
        std::fs::create_dir(&doomed).unwrap();

        // Set the tool's persistent CWD to the doomed dir
        *tool.cwd.lock().unwrap() = Some(doomed.clone());

        // Delete the directory
        std::fs::remove_dir(&doomed).unwrap();

        // Next command should fall back to ctx.cwd
        let output = tool
            .execute(serde_json::json!({"command": "pwd"}), test_ctx())
            .await
            .unwrap();

        assert!(!output.is_error);
        // Persistent CWD should have been reset
        assert!(tool.cwd.lock().unwrap().is_none());
    }

    #[test]
    fn test_extract_cd_target() {
        assert_eq!(extract_cd_target("cd /tmp"), Some("/tmp"));
        assert_eq!(extract_cd_target("cd foo/bar"), Some("foo/bar"));
        assert_eq!(extract_cd_target("cd foo && ls"), None);
        assert_eq!(extract_cd_target("echo hi"), None);
    }
}
