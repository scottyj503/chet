//! SubagentTool — spawns a child agent to handle a delegated task.

use crate::Agent;
use crate::worktree;
use chet_permissions::PermissionEngine;
use chet_tools::ToolRegistry;
use chet_types::{
    ContentBlock, Message, Role, ToolContext, ToolDefinition, ToolError, ToolOutput,
    provider::Provider,
};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A tool that spawns a child agent to handle a delegated task independently.
///
/// The child agent gets a fresh set of built-in tools (no SubagentTool, preventing
/// infinite recursion) and shares the parent's permission engine so session rules
/// propagate both ways. The child runs silently and its final assistant text becomes
/// the tool result.
pub struct SubagentTool {
    provider: Arc<dyn Provider>,
    permissions: Arc<PermissionEngine>,
    model: String,
    max_tokens: u32,
    cwd: PathBuf,
}

impl SubagentTool {
    pub fn new(
        provider: Arc<dyn Provider>,
        permissions: Arc<PermissionEngine>,
        model: String,
        max_tokens: u32,
        cwd: PathBuf,
    ) -> Self {
        Self {
            provider,
            permissions,
            model,
            max_tokens,
            cwd,
        }
    }
}

/// System prompt for subagent children.
fn subagent_system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "You are a subagent of Chet, an AI coding assistant. You have been spawned to \
         handle a specific task. Complete the task using the available tools and provide \
         a clear, concise response with your findings or results.\n\n\
         Current working directory: {}",
        cwd.display()
    )
}

/// Extract the text content from the last assistant message.
fn extract_assistant_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|m| {
            if m.role == Role::Assistant {
                let text: String = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() { None } else { Some(text) }
            } else {
                None
            }
        })
        .unwrap_or_default()
}

impl chet_types::Tool for SubagentTool {
    fn name(&self) -> &str {
        "Subagent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Subagent".to_string(),
            description: "Spawn a child agent to handle a delegated task independently. \
                          The child agent has access to all built-in tools (Read, Write, \
                          Edit, Bash, Glob, Grep) and runs silently. Use this for complex \
                          sub-tasks like searching many files, running test suites, or \
                          making independent changes in parallel."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["prompt"],
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task for the child agent to perform"
                    },
                    "description": {
                        "type": "string",
                        "description": "A short description of the task (for logging/display)"
                    },
                    "isolation": {
                        "type": "string",
                        "enum": ["none", "worktree"],
                        "description": "Isolation mode. 'worktree' runs the child in an isolated git worktree. Default: 'none'."
                    }
                }
            }),
            cache_control: None,
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + '_>> {
        Box::pin(async move {
            let prompt = input
                .get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput {
                    tool: "Subagent".to_string(),
                    message: "Missing required parameter: prompt".to_string(),
                })?
                .to_string();

            let isolation = input
                .get("isolation")
                .and_then(|v| v.as_str())
                .unwrap_or("none");

            // Set up worktree isolation if requested
            let mut managed_worktree = None;
            let effective_cwd = if isolation == "worktree" {
                match worktree::create_worktree(
                    &self.cwd,
                    None,
                    Some(Arc::clone(&self.permissions)),
                )
                .await
                {
                    Ok(wt) => {
                        let cwd = wt.path().to_path_buf();
                        managed_worktree = Some(wt);
                        cwd
                    }
                    Err(e) => {
                        return Ok(ToolOutput::error(format!("Failed to create worktree: {e}")));
                    }
                }
            } else {
                self.cwd.clone()
            };

            // Create child agent with builtins only (no SubagentTool → no recursion)
            let registry = ToolRegistry::with_builtins();
            let mut child = Agent::new(
                Arc::clone(&self.provider),
                registry,
                Arc::clone(&self.permissions),
                self.model.clone(),
                self.max_tokens,
                effective_cwd.clone(),
            );
            child.set_system_prompt(subagent_system_prompt(&effective_cwd));

            let mut messages = vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: prompt }],
            }];

            // Run silently — no-op event callback
            let cancel = CancellationToken::new();
            let result = match child.run(&mut messages, cancel, |_| {}).await {
                Ok(_usage) => {
                    let text = extract_assistant_text(&messages);
                    if text.is_empty() {
                        Ok(ToolOutput::error(
                            "Subagent completed but produced no text output".to_string(),
                        ))
                    } else {
                        Ok(ToolOutput::text(text))
                    }
                }
                Err(e) => Ok(ToolOutput::error(format!("Subagent error: {e}"))),
            };

            // Clean up worktree if we created one
            if let Some(mut wt) = managed_worktree {
                if let Err(e) = wt.cleanup().await {
                    tracing::warn!("Failed to remove subagent worktree: {e}");
                }
            }

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_api::AnthropicProvider;

    fn make_tool() -> SubagentTool {
        let provider: Arc<dyn Provider> =
            Arc::new(AnthropicProvider::new("test-key", "https://api.example.com").unwrap());
        let permissions = Arc::new(PermissionEngine::ludicrous());
        SubagentTool::new(
            provider,
            permissions,
            "claude-sonnet-4-20250514".to_string(),
            4096,
            PathBuf::from("/tmp"),
        )
    }

    #[test]
    fn definition_has_correct_name() {
        use chet_types::Tool;
        let tool = make_tool();
        assert_eq!(tool.name(), "Subagent");
        let def = tool.definition();
        assert_eq!(def.name, "Subagent");
        assert!(def.description.contains("child agent"));
    }

    #[test]
    fn definition_schema_requires_prompt() {
        use chet_types::Tool;
        let tool = make_tool();
        let def = tool.definition();
        let required = def.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("prompt")));
        assert!(def.input_schema["properties"]["prompt"].is_object());
    }

    #[test]
    fn is_not_read_only() {
        use chet_types::Tool;
        let tool = make_tool();
        assert!(!tool.is_read_only());
    }

    #[test]
    fn definition_schema_has_isolation_property() {
        use chet_types::Tool;
        let tool = make_tool();
        let def = tool.definition();
        let isolation = &def.input_schema["properties"]["isolation"];
        assert!(isolation.is_object());
        assert_eq!(isolation["type"], "string");
        let enum_vals = isolation["enum"].as_array().unwrap();
        assert!(enum_vals.iter().any(|v| v.as_str() == Some("none")));
        assert!(enum_vals.iter().any(|v| v.as_str() == Some("worktree")));
    }

    #[test]
    fn extract_assistant_text_from_messages() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "the result".to_string(),
                }],
            },
        ];
        assert_eq!(extract_assistant_text(&messages), "the result");
    }

    #[test]
    fn extract_assistant_text_empty_when_no_assistant() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        }];
        assert_eq!(extract_assistant_text(&messages), "");
    }

    #[test]
    fn extract_assistant_text_skips_tool_use_blocks() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::Text {
                        text: "final answer".to_string(),
                    },
                ],
            },
        ];
        assert_eq!(extract_assistant_text(&messages), "final answer");
    }
}
