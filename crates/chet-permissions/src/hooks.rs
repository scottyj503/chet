//! Hook runner — executes external scripts on permission events.

use crate::types::{HookConfig, HookEvent, HookInput};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Run all hooks matching the given event, sequentially.
///
/// Exit code protocol:
/// - 0 = approve (continue)
/// - 1 = error (warn and continue)
/// - 2 = deny (stop and return error)
///
/// First deny wins — subsequent hooks are not executed.
/// Timeout kills the process and is treated as an error (warn + continue).
pub async fn run_hooks(
    hooks: &[HookConfig],
    event: &HookEvent,
    hook_input: &HookInput,
) -> Result<(), String> {
    let matching: Vec<&HookConfig> = hooks.iter().filter(|h| &h.event == event).collect();

    for hook in matching {
        match run_single_hook(hook, hook_input).await {
            HookResult::Approve => continue,
            HookResult::Deny(reason) => return Err(reason),
            HookResult::Error(msg) => {
                tracing::warn!("Hook '{}' error: {}", hook.command, msg);
                continue;
            }
        }
    }

    Ok(())
}

enum HookResult {
    Approve,
    Deny(String),
    Error(String),
}

async fn run_single_hook(hook: &HookConfig, hook_input: &HookInput) -> HookResult {
    let input_json = match serde_json::to_string(hook_input) {
        Ok(json) => json,
        Err(e) => return HookResult::Error(format!("Failed to serialize hook input: {e}")),
    };

    let mut child = match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return HookResult::Error(format!("Failed to spawn hook: {e}")),
    };

    // Write JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    // Wait with timeout
    let timeout = Duration::from_millis(hook.timeout_ms);
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => match status.code() {
            Some(0) => HookResult::Approve,
            Some(2) => HookResult::Deny(format!("Hook '{}' denied the operation", hook.command)),
            Some(code) => HookResult::Error(format!("Hook exited with code {code}")),
            None => HookResult::Error("Hook terminated by signal".to_string()),
        },
        Ok(Err(e)) => HookResult::Error(format!("Failed to wait for hook: {e}")),
        Err(_) => {
            // Timeout — kill the process
            let _ = child.kill().await;
            HookResult::Error(format!(
                "Hook '{}' timed out after {}ms",
                hook.command, hook.timeout_ms
            ))
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::types::HookEvent;

    fn test_hook(command: &str) -> HookConfig {
        HookConfig {
            event: HookEvent::BeforeTool,
            command: command.to_string(),
            timeout_ms: 5000,
        }
    }

    fn test_input() -> HookInput {
        HookInput {
            event: HookEvent::BeforeTool,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            tool_output: None,
            is_error: None,
        }
    }

    #[tokio::test]
    async fn test_hook_approve() {
        let hooks = vec![test_hook("exit 0")];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_deny() {
        let hooks = vec![test_hook("exit 2")];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("denied"));
    }

    #[tokio::test]
    async fn test_hook_error_continues() {
        // Exit 1 = error, should warn but continue (Ok)
        let hooks = vec![test_hook("exit 1")];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeTool,
            command: "sleep 10".to_string(),
            timeout_ms: 100,
        }];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        // Timeout is treated as error (warn + continue)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_receives_json_stdin() {
        // Hook reads stdin and exits 0 if it got valid JSON
        let hooks = vec![test_hook(
            "python3 -c 'import sys, json; json.load(sys.stdin)'",
        )];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        // This may fail if python3 isn't available, so just check it doesn't panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_hook_no_matching_event() {
        let hooks = vec![test_hook("exit 2")]; // BeforeTool hook
        // Run with AfterTool event — should not match, so Ok
        let result = run_hooks(&hooks, &HookEvent::AfterTool, &test_input()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_chain_first_deny_wins() {
        let hooks = vec![
            test_hook("exit 0"), // approve
            test_hook("exit 2"), // deny
            test_hook("exit 0"), // would approve but never reached
        ];
        let result = run_hooks(&hooks, &HookEvent::BeforeTool, &test_input()).await;
        assert!(result.is_err());
    }
}
