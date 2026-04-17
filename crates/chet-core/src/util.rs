//! Internal utility functions for the agent loop.

use crate::AgentEvent;
use chet_permissions::{HookEvent, HookInput, PermissionEngine};
use chet_types::{ContentBlock, ToolOutput, ToolResultContent};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Truncate a string for display, adding "..." if truncated.
pub(crate) fn truncate_for_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", chet_types::truncate_str(s, max_len))
    }
}

/// Persist a large tool result to a temp file under the CWD.
/// Returns the path on success, None on failure.
pub(crate) fn persist_tool_result(
    cwd: &Path,
    tool_name: &str,
    tool_id: &str,
    text: &str,
) -> Option<PathBuf> {
    let dir = cwd.join(".chet-tool-output");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Failed to create tool output dir: {e}");
        return None;
    }
    // Short ID to avoid long filenames
    let short_id = if tool_id.len() > 8 {
        &tool_id[..8]
    } else {
        tool_id
    };
    let filename = format!("{tool_name}-{short_id}.txt");
    let path = dir.join(&filename);
    match std::fs::write(&path, text) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!("Failed to persist tool result: {e}");
            None
        }
    }
}

/// Fire StopFailure hook on API errors (best-effort, log-only).
pub(crate) async fn fire_stop_failure_hook(permissions: &Arc<PermissionEngine>, error_msg: &str) {
    let hook_input = HookInput {
        event: HookEvent::StopFailure,
        tool_name: None,
        tool_input: None,
        tool_output: Some(error_msg.to_string()),
        is_error: Some(true),
        worktree_path: None,
        worktree_source: None,
        messages_removed: None,
        messages_remaining: None,
        config_path: None,
    };
    if let Err(msg) = permissions
        .run_hooks(&HookEvent::StopFailure, &hook_input)
        .await
    {
        tracing::warn!("stop_failure hook error: {msg}");
    }
}

/// Emit ToolEnd event, run after_tool hooks, and build the ToolResult ContentBlock.
pub(crate) async fn finalize_tool_result(
    permissions: &Arc<PermissionEngine>,
    cwd: &Path,
    tool_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    output: ToolOutput,
    on_event: &mut impl FnMut(AgentEvent),
) -> ContentBlock {
    let output_text = output
        .content
        .iter()
        .filter_map(|c| match c {
            chet_types::ToolOutputContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    on_event(AgentEvent::ToolEnd {
        name: tool_name.to_string(),
        output: truncate_for_display(&output_text, 200),
        is_error: output.is_error,
    });

    let after_hook_input = HookInput {
        event: HookEvent::AfterTool,
        tool_name: Some(tool_name.to_string()),
        tool_input: Some(tool_input.clone()),
        tool_output: Some(truncate_for_display(&output_text, 1000)),
        is_error: Some(output.is_error),
        worktree_path: None,
        worktree_source: None,
        messages_removed: None,
        messages_remaining: None,
        config_path: None,
    };
    if let Err(msg) = permissions
        .run_hooks(&HookEvent::AfterTool, &after_hook_input)
        .await
    {
        tracing::warn!("after_tool hook error: {msg}");
    }

    const MAX_INLINE_RESULT_CHARS: usize = 50_000;

    let content = output
        .content
        .into_iter()
        .map(|c| match c {
            chet_types::ToolOutputContent::Text { text }
                if text.len() > MAX_INLINE_RESULT_CHARS =>
            {
                let path = persist_tool_result(cwd, tool_name, tool_id, &text);
                let truncated = chet_types::truncate_str(&text, MAX_INLINE_RESULT_CHARS);
                let note = match path {
                    Some(p) => format!(
                        "{truncated}\n\n(output truncated from {} chars — full result at {})",
                        text.len(),
                        p.display()
                    ),
                    None => format!(
                        "{truncated}\n\n(output truncated from {} chars)",
                        text.len()
                    ),
                };
                ToolResultContent::Text { text: note }
            }
            chet_types::ToolOutputContent::Text { text } => ToolResultContent::Text { text },
            chet_types::ToolOutputContent::Image { source } => ToolResultContent::Image { source },
        })
        .collect();

    ContentBlock::ToolResult {
        tool_use_id: tool_id.to_string(),
        content,
        is_error: if output.is_error { Some(true) } else { None },
    }
}
