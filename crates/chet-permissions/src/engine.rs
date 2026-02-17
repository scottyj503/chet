//! Permission engine — the main entry point for permission checks.

use crate::hooks::run_hooks;
use crate::matcher::RuleMatcher;
use crate::prompt::PromptHandler;
use crate::types::*;
use std::sync::{Arc, Mutex};

/// The central permission engine that evaluates rules, runs hooks, and prompts users.
pub struct PermissionEngine {
    /// Static rules loaded from config.
    rules: Vec<PermissionRule>,
    /// Session-scoped rules added via "always allow" responses.
    session_rules: Mutex<Vec<PermissionRule>>,
    /// Hook configurations.
    hooks: Vec<HookConfig>,
    /// Optional prompt handler for interactive permission prompts.
    prompt_handler: Option<Arc<dyn PromptHandler>>,
    /// When true, all tool calls are auto-permitted (--ludicrous mode).
    ludicrous: bool,
}

impl PermissionEngine {
    /// Create a new permission engine with the given rules and hooks.
    pub fn new(
        rules: Vec<PermissionRule>,
        hooks: Vec<HookConfig>,
        prompt_handler: Option<Arc<dyn PromptHandler>>,
    ) -> Self {
        Self {
            rules,
            session_rules: Mutex::new(Vec::new()),
            hooks,
            prompt_handler,
            ludicrous: false,
        }
    }

    /// Create an engine that auto-permits everything (--ludicrous mode).
    pub fn ludicrous() -> Self {
        Self {
            rules: Vec::new(),
            session_rules: Mutex::new(Vec::new()),
            hooks: Vec::new(),
            prompt_handler: None,
            ludicrous: true,
        }
    }

    /// Check whether a tool call is permitted, blocked, or needs a prompt.
    ///
    /// Evaluation order:
    /// 1. If ludicrous mode, always Permit.
    /// 2. Check session rules (added via "always allow").
    /// 3. Check static rules from config.
    /// 4. Priority: block > permit > prompt.
    /// 5. Default: read-only tools = Permit, mutating tools = Prompt.
    pub fn check(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        is_read_only: bool,
    ) -> PermissionDecision {
        if self.ludicrous {
            return PermissionDecision::Permit;
        }

        // Check session rules first (they can only add permits)
        let session_rules = self.session_rules.lock().unwrap();
        if let Some(level) = RuleMatcher::evaluate(&session_rules, tool_name, tool_input) {
            if level == PermissionLevel::Permit {
                return PermissionDecision::Permit;
            }
        }
        drop(session_rules);

        // Check static rules
        if let Some(level) = RuleMatcher::evaluate(&self.rules, tool_name, tool_input) {
            return match level {
                PermissionLevel::Permit => PermissionDecision::Permit,
                PermissionLevel::Block => PermissionDecision::Block {
                    reason: format!("Tool '{tool_name}' blocked by permission rule"),
                },
                PermissionLevel::Prompt => PermissionDecision::Prompt {
                    tool: tool_name.to_string(),
                    description: format!("Tool '{tool_name}' requires permission"),
                },
            };
        }

        // Default behavior: read-only = permit, mutating = prompt
        if is_read_only {
            PermissionDecision::Permit
        } else {
            PermissionDecision::Prompt {
                tool: tool_name.to_string(),
                description: format!("Tool '{tool_name}' requires permission"),
            }
        }
    }

    /// Add a session-scoped permit rule (from "always allow" responses).
    /// Dies with the process — not persisted to config.
    pub fn add_session_rule(&self, rule: PermissionRule) {
        let mut session_rules = self.session_rules.lock().unwrap();
        session_rules.push(rule);
    }

    /// Run hooks for the given event.
    pub async fn run_hooks(
        &self,
        event: &HookEvent,
        hook_input: &HookInput,
    ) -> Result<(), String> {
        if self.ludicrous {
            return Ok(());
        }
        run_hooks(&self.hooks, event, hook_input).await
    }

    /// Prompt the user for permission. Returns Block if no handler is set.
    pub async fn prompt(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        description: &str,
    ) -> PromptResponse {
        match &self.prompt_handler {
            Some(handler) => {
                handler
                    .prompt_permission(tool_name, tool_input, description)
                    .await
            }
            None => {
                // No prompt handler = non-interactive mode, safe default is deny
                PromptResponse::Deny
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(tool: &str, args: Option<&str>, level: PermissionLevel) -> PermissionRule {
        PermissionRule {
            tool: tool.to_string(),
            args: args.map(|s| s.to_string()),
            level,
        }
    }

    fn engine(rules: Vec<PermissionRule>) -> PermissionEngine {
        PermissionEngine::new(rules, Vec::new(), None)
    }

    #[test]
    fn test_block_overrides_permit() {
        let e = engine(vec![
            rule("Bash", None, PermissionLevel::Permit),
            rule("Bash", Some("command:rm *"), PermissionLevel::Block),
        ]);
        let decision = e.check("Bash", &json!({"command": "rm -rf /"}), false);
        assert!(matches!(decision, PermissionDecision::Block { .. }));
    }

    #[test]
    fn test_permit_overrides_prompt() {
        let e = engine(vec![
            rule("Bash", None, PermissionLevel::Prompt),
            rule("Bash", Some("command:git *"), PermissionLevel::Permit),
        ]);
        let decision = e.check("Bash", &json!({"command": "git status"}), false);
        assert_eq!(decision, PermissionDecision::Permit);
    }

    #[test]
    fn test_default_read_only_permits() {
        let e = engine(vec![]);
        let decision = e.check("Read", &json!({"file_path": "/tmp/test"}), true);
        assert_eq!(decision, PermissionDecision::Permit);
    }

    #[test]
    fn test_default_mutating_prompts() {
        let e = engine(vec![]);
        let decision = e.check("Bash", &json!({"command": "ls"}), false);
        assert!(matches!(decision, PermissionDecision::Prompt { .. }));
    }

    #[test]
    fn test_session_rule_permits() {
        let e = engine(vec![]);
        // Initially prompts
        assert!(matches!(
            e.check("Bash", &json!({"command": "ls"}), false),
            PermissionDecision::Prompt { .. }
        ));

        // Add session rule
        e.add_session_rule(rule("Bash", None, PermissionLevel::Permit));

        // Now permits
        assert_eq!(
            e.check("Bash", &json!({"command": "ls"}), false),
            PermissionDecision::Permit
        );
    }

    #[test]
    fn test_ludicrous_mode() {
        let e = PermissionEngine::ludicrous();
        assert_eq!(
            e.check("Bash", &json!({"command": "rm -rf /"}), false),
            PermissionDecision::Permit
        );
    }

    #[tokio::test]
    async fn test_no_prompt_handler_returns_deny() {
        let e = engine(vec![]);
        let response = e.prompt("Bash", &json!({}), "test").await;
        assert_eq!(response, PromptResponse::Deny);
    }
}
