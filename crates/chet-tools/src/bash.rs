//! Bash tool — executes shell commands.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::process::Command;

/// Maximum output length before truncation.
const MAX_OUTPUT_BYTES: usize = 30_000;

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
        }
    }

    fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>> {
        Box::pin(async move {
        let input: BashInput = serde_json::from_value(input).map_err(|e| {
            ToolError::InvalidInput {
                tool: "Bash".into(),
                message: e.to_string(),
            }
        })?;

        let timeout_ms = input
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(600_000);

        // Get the persistent cwd or fall back to context cwd
        let cwd = {
            let lock = self.cwd.lock().unwrap();
            lock.clone().unwrap_or_else(|| ctx.cwd.clone())
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            Command::new("bash")
                .arg("-c")
                .arg(&input.command)
                .current_dir(&cwd)
                .output(),
        )
        .await;

        let output = match result {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "Failed to spawn command: {e}"
                )));
            }
            Err(_) => {
                return Err(ToolError::Timeout {
                    timeout_ms,
                });
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

        // Truncate if needed
        if result_text.len() > MAX_OUTPUT_BYTES {
            result_text.truncate(MAX_OUTPUT_BYTES);
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
    use std::collections::HashMap;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            env: HashMap::new(),
            sandboxed: false,
        }
    }

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool::new();
        let output = tool
            .execute(
                serde_json::json!({"command": "echo hello"}),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert_eq!(text.trim(), "hello");
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool::new();
        let output = tool
            .execute(
                serde_json::json!({"command": "exit 1"}),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(output.is_error);
    }

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

    #[test]
    fn test_extract_cd_target() {
        assert_eq!(extract_cd_target("cd /tmp"), Some("/tmp"));
        assert_eq!(extract_cd_target("cd foo/bar"), Some("foo/bar"));
        assert_eq!(extract_cd_target("cd foo && ls"), None);
        assert_eq!(extract_cd_target("echo hi"), None);
    }
}
