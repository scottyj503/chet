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
}
