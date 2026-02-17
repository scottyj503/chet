//! The core agent loop that orchestrates conversation with tool use.

use chet_api::ApiClient;
use chet_permissions::{
    HookEvent, HookInput, PermissionDecision, PermissionEngine, PermissionLevel, PermissionRule,
    PromptResponse,
};
use chet_tools::ToolRegistry;
use chet_types::{
    ContentBlock, ContentDelta, CreateMessageRequest, Message, Role, StopReason, StreamEvent,
    ToolContext, ToolOutput, ToolResultContent, Usage,
};
use futures_util::StreamExt;
use std::path::PathBuf;

/// Maximum number of consecutive tool-use loops before stopping.
const MAX_TOOL_LOOPS: usize = 50;

/// Events emitted by the agent during execution.
#[derive(Debug)]
pub enum AgentEvent {
    /// A text delta from the assistant's response.
    TextDelta(String),
    /// A thinking delta (extended thinking).
    ThinkingDelta(String),
    /// A tool is about to be executed.
    ToolStart { name: String, input: String },
    /// A tool has finished executing.
    ToolEnd {
        name: String,
        output: String,
        is_error: bool,
    },
    /// Usage information from the API.
    Usage(Usage),
    /// The agent has finished (no more tool calls).
    Done,
    /// A tool call was blocked by the permission system.
    ToolBlocked { name: String, reason: String },
    /// An error occurred.
    Error(String),
}

/// The main agent that manages conversation with the LLM and tool execution.
pub struct Agent {
    client: ApiClient,
    registry: ToolRegistry,
    permissions: PermissionEngine,
    model: String,
    max_tokens: u32,
    system_prompt: Option<String>,
    cwd: PathBuf,
}

impl Agent {
    pub fn new(
        client: ApiClient,
        registry: ToolRegistry,
        permissions: PermissionEngine,
        model: String,
        max_tokens: u32,
        cwd: PathBuf,
    ) -> Self {
        Self {
            client,
            registry,
            permissions,
            model,
            max_tokens,
            system_prompt: None,
            cwd,
        }
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = Some(prompt);
    }

    /// Run the agent loop: send messages, handle tool calls, repeat until done.
    ///
    /// The callback receives AgentEvents as they occur (for streaming UI).
    /// Returns the final list of messages (including assistant + tool results).
    pub async fn run<F>(
        &self,
        messages: &mut Vec<Message>,
        mut on_event: F,
    ) -> Result<Usage, chet_types::ChetError>
    where
        F: FnMut(AgentEvent),
    {
        let mut total_usage = Usage::default();

        for _loop_iter in 0..MAX_TOOL_LOOPS {
            let request = CreateMessageRequest {
                model: self.model.clone(),
                max_tokens: self.max_tokens,
                messages: messages.clone(),
                system: self.system_prompt.clone(),
                tools: Some(self.registry.definitions()),
                stop_sequences: None,
                temperature: None,
                stream: true,
            };

            let mut stream = self
                .client
                .create_message_stream(&request)
                .await
                .map_err(chet_types::ChetError::Api)?;

            // Collect the full assistant response
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut current_text = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_json = String::new();
            let mut stop_reason = None;

            while let Some(event) = stream.next().await {
                match event {
                    Ok(StreamEvent::MessageStart { message }) => {
                        total_usage.add(&message.usage);
                    }
                    Ok(StreamEvent::ContentBlockStart {
                        content_block: ContentBlock::Text { .. },
                        ..
                    }) => {
                        current_text.clear();
                    }
                    Ok(StreamEvent::ContentBlockStart {
                        content_block: ContentBlock::ToolUse { id, name, .. },
                        ..
                    }) => {
                        current_tool_id = id;
                        current_tool_name = name;
                        current_tool_json.clear();
                        on_event(AgentEvent::ToolStart {
                            name: current_tool_name.clone(),
                            input: String::new(),
                        });
                    }
                    Ok(StreamEvent::ContentBlockDelta { delta, .. }) => match delta {
                        ContentDelta::TextDelta { text } => {
                            on_event(AgentEvent::TextDelta(text.clone()));
                            current_text.push_str(&text);
                        }
                        ContentDelta::InputJsonDelta { partial_json } => {
                            current_tool_json.push_str(&partial_json);
                        }
                        ContentDelta::ThinkingDelta { thinking } => {
                            on_event(AgentEvent::ThinkingDelta(thinking));
                        }
                        _ => {}
                    },
                    Ok(StreamEvent::ContentBlockStop { .. }) => {
                        if !current_text.is_empty() {
                            content_blocks.push(ContentBlock::Text {
                                text: current_text.clone(),
                            });
                            current_text.clear();
                        }
                        if !current_tool_id.is_empty() {
                            let input_value: serde_json::Value =
                                serde_json::from_str(&current_tool_json).unwrap_or_default();
                            content_blocks.push(ContentBlock::ToolUse {
                                id: current_tool_id.clone(),
                                name: current_tool_name.clone(),
                                input: input_value,
                            });
                            current_tool_id.clear();
                            current_tool_name.clear();
                            current_tool_json.clear();
                        }
                    }
                    Ok(StreamEvent::MessageDelta { delta, usage }) => {
                        stop_reason = delta.stop_reason;
                        if let Some(u) = usage {
                            total_usage.add(&u);
                        }
                    }
                    Ok(StreamEvent::Error { error }) => {
                        on_event(AgentEvent::Error(format!(
                            "{}: {}",
                            error.error_type, error.message
                        )));
                        return Err(chet_types::ChetError::Api(chet_types::ApiError::Server {
                            status: 0,
                            message: error.message,
                        }));
                    }
                    Ok(_) => {} // Ping, MessageStop
                    Err(e) => {
                        on_event(AgentEvent::Error(e.to_string()));
                        return Err(chet_types::ChetError::Api(e));
                    }
                }
            }

            // Append the assistant message
            if !content_blocks.is_empty() {
                messages.push(Message {
                    role: Role::Assistant,
                    content: content_blocks.clone(),
                });
            }

            // If no tool use, we're done
            let tool_uses: Vec<_> = content_blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            if tool_uses.is_empty() || stop_reason == Some(StopReason::EndTurn) {
                on_event(AgentEvent::Done);
                on_event(AgentEvent::Usage(total_usage.clone()));
                return Ok(total_usage);
            }

            // Execute tools and collect results
            let ctx = ToolContext {
                cwd: self.cwd.clone(),
                env: std::env::vars().collect(),
                sandboxed: false,
            };

            let mut tool_results = Vec::new();
            for (tool_id, tool_name, tool_input) in &tool_uses {
                let is_read_only = self.registry.is_read_only(tool_name).unwrap_or(false);

                // 1. Check permissions
                let decision = self.permissions.check(tool_name, tool_input, is_read_only);

                let permitted = match decision {
                    PermissionDecision::Permit => true,
                    PermissionDecision::Block { reason } => {
                        on_event(AgentEvent::ToolBlocked {
                            name: tool_name.clone(),
                            reason: reason.clone(),
                        });
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tool_id.clone(),
                            content: vec![ToolResultContent::Text {
                                text: format!("Permission denied: {reason}"),
                            }],
                            is_error: Some(true),
                        });
                        false
                    }
                    PermissionDecision::Prompt { description, .. } => {
                        let response = self
                            .permissions
                            .prompt(tool_name, tool_input, &description)
                            .await;
                        match response {
                            PromptResponse::AllowOnce => true,
                            PromptResponse::AlwaysAllow => {
                                self.permissions.add_session_rule(PermissionRule {
                                    tool: tool_name.clone(),
                                    args: None,
                                    level: PermissionLevel::Permit,
                                });
                                true
                            }
                            PromptResponse::Deny => {
                                on_event(AgentEvent::ToolBlocked {
                                    name: tool_name.clone(),
                                    reason: "Denied by user".to_string(),
                                });
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: tool_id.clone(),
                                    content: vec![ToolResultContent::Text {
                                        text: "Permission denied by user".to_string(),
                                    }],
                                    is_error: Some(true),
                                });
                                false
                            }
                        }
                    }
                };

                if !permitted {
                    continue;
                }

                // 2. Run before_tool hooks
                let hook_input = HookInput {
                    event: HookEvent::BeforeTool,
                    tool_name: Some(tool_name.clone()),
                    tool_input: Some(tool_input.clone()),
                    tool_output: None,
                    is_error: None,
                };
                if let Err(reason) = self
                    .permissions
                    .run_hooks(&HookEvent::BeforeTool, &hook_input)
                    .await
                {
                    on_event(AgentEvent::ToolBlocked {
                        name: tool_name.clone(),
                        reason: reason.clone(),
                    });
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: tool_id.clone(),
                        content: vec![ToolResultContent::Text {
                            text: format!("Blocked by hook: {reason}"),
                        }],
                        is_error: Some(true),
                    });
                    continue;
                }

                // 3. Execute the tool
                let result = self
                    .registry
                    .execute(tool_name, tool_input.clone(), ctx.clone())
                    .await;

                let output = match result {
                    Ok(output) => output,
                    Err(e) => ToolOutput::error(e.to_string()),
                };

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
                    name: tool_name.clone(),
                    output: truncate_for_display(&output_text, 200),
                    is_error: output.is_error,
                });

                // 4. Run after_tool hooks (log-only, don't undo)
                let after_hook_input = HookInput {
                    event: HookEvent::AfterTool,
                    tool_name: Some(tool_name.clone()),
                    tool_input: Some(tool_input.clone()),
                    tool_output: Some(truncate_for_display(&output_text, 1000)),
                    is_error: Some(output.is_error),
                };
                if let Err(msg) = self
                    .permissions
                    .run_hooks(&HookEvent::AfterTool, &after_hook_input)
                    .await
                {
                    tracing::warn!("after_tool hook error: {msg}");
                }

                let content = output
                    .content
                    .into_iter()
                    .map(|c| match c {
                        chet_types::ToolOutputContent::Text { text } => {
                            ToolResultContent::Text { text }
                        }
                        chet_types::ToolOutputContent::Image { source } => {
                            ToolResultContent::Image { source }
                        }
                    })
                    .collect();

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content,
                    is_error: if output.is_error { Some(true) } else { None },
                });
            }

            // Append tool results as a user message
            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });
        }

        on_event(AgentEvent::Error(
            "Maximum tool-use loops reached".to_string(),
        ));
        Ok(total_usage)
    }
}

fn truncate_for_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
