//! Core types for the permission system.

use serde::{Deserialize, Serialize};

/// Permission level for a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    Permit,
    Block,
    Prompt,
}

impl PermissionLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionLevel::Permit => "permit",
            PermissionLevel::Block => "block",
            PermissionLevel::Prompt => "prompt",
        }
    }
}

/// A single permission rule matching a tool + optional argument pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Tool name or glob pattern (e.g., "Bash", "*").
    pub tool: String,
    /// Optional argument matcher in `field_name:glob_pattern` format
    /// (e.g., "command:git *", "file_path:/etc/*").
    #[serde(default)]
    pub args: Option<String>,
    /// The permission level to apply when this rule matches.
    pub level: PermissionLevel,
}

/// The result of evaluating permission rules for a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool call is permitted — execute immediately.
    Permit,
    /// Tool call is blocked — do not execute.
    Block { reason: String },
    /// Tool call requires user confirmation.
    Prompt { tool: String, description: String },
}

/// User's response to a permission prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptResponse {
    /// Allow this specific invocation only.
    AllowOnce,
    /// Allow this tool for the rest of the session.
    AlwaysAllow,
    /// Deny this tool call.
    Deny,
}

/// Events that can trigger hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    BeforeTool,
    AfterTool,
    BeforeInput,
    OnExit,
    OnSessionStart,
    OnSessionEnd,
    WorktreeCreate,
    WorktreeRemove,
}

/// Configuration for a single hook script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Which event triggers this hook.
    pub event: HookEvent,
    /// Shell command to execute.
    pub command: String,
    /// Timeout in milliseconds (default: 10000).
    #[serde(default = "default_hook_timeout")]
    pub timeout_ms: u64,
}

fn default_hook_timeout() -> u64 {
    10_000
}

/// JSON payload sent to hooks on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInput {
    /// The event that triggered the hook.
    pub event: HookEvent,
    /// Tool name (for tool-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input JSON (for tool-related events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// Tool output (for after_tool events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<String>,
    /// Whether the tool execution was an error (for after_tool events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Path to the worktree directory (for worktree events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Path to the source repository (for worktree events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_serde_worktree_create() {
        let event = HookEvent::WorktreeCreate;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"worktree_create\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::WorktreeCreate);
    }

    #[test]
    fn hook_event_serde_worktree_remove() {
        let event = HookEvent::WorktreeRemove;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"worktree_remove\"");
        let back: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, HookEvent::WorktreeRemove);
    }

    #[test]
    fn hook_input_worktree_fields_serialize() {
        let input = HookInput {
            event: HookEvent::WorktreeCreate,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            is_error: None,
            worktree_path: Some("/tmp/chet-worktree-abc".to_string()),
            worktree_source: Some("/home/user/repo".to_string()),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["event"], "worktree_create");
        assert_eq!(json["worktree_path"], "/tmp/chet-worktree-abc");
        assert_eq!(json["worktree_source"], "/home/user/repo");
        assert!(json.get("tool_name").is_none());
    }

    #[test]
    fn hook_input_worktree_fields_skip_when_none() {
        let input = HookInput {
            event: HookEvent::BeforeTool,
            tool_name: Some("Read".to_string()),
            tool_input: None,
            tool_output: None,
            is_error: None,
            worktree_path: None,
            worktree_source: None,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert!(json.get("worktree_path").is_none());
        assert!(json.get("worktree_source").is_none());
    }

    #[test]
    fn hook_input_roundtrip() {
        let input = HookInput {
            event: HookEvent::WorktreeRemove,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            is_error: None,
            worktree_path: Some("/tmp/wt".to_string()),
            worktree_source: Some("/repo".to_string()),
        };
        let json = serde_json::to_string(&input).unwrap();
        let back: HookInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, HookEvent::WorktreeRemove);
        assert_eq!(back.worktree_path.as_deref(), Some("/tmp/wt"));
        assert_eq!(back.worktree_source.as_deref(), Some("/repo"));
    }
}
