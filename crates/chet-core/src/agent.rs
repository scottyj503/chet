//! The core agent loop that orchestrates conversation with tool use.

use crate::util::{finalize_tool_result, fire_stop_failure_hook};
use chet_permissions::{
    HookEvent, HookInput, PermissionDecision, PermissionEngine, PermissionLevel, PermissionRule,
    PromptResponse,
};
use chet_tools::ToolRegistry;
use chet_types::{
    CacheControl, ContentBlock, ContentDelta, CreateMessageRequest, Effort, Message, Role,
    StopReason, StreamEvent, SystemContent, ThinkingConfig, ToolContext, ToolOutput,
    ToolResultContent, Usage,
    provider::{EventStream, Provider},
};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
    /// The operation was cancelled (e.g. Ctrl+C).
    Cancelled,
    /// An error occurred.
    Error(String),
}

/// Result of collecting the assistant's streaming response.
struct CollectResult {
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    tool_uses: Vec<(String, String, serde_json::Value)>,
}

/// Result of checking permissions for tool uses.
struct PermissionCheckResult {
    tool_results: Vec<Option<ContentBlock>>,
    permitted_tools: Vec<(usize, String, String, serde_json::Value, bool)>,
}

/// The main agent that manages conversation with the LLM and tool execution.
pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    permissions: Arc<PermissionEngine>,
    model: String,
    max_tokens: u32,
    system_prompt: Option<String>,
    thinking_budget: Option<u32>,
    effort: Option<Effort>,
    cwd: PathBuf,
    read_only_mode: bool,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: ToolRegistry,
        permissions: Arc<PermissionEngine>,
        model: String,
        max_tokens: u32,
        cwd: PathBuf,
    ) -> Self {
        Self {
            provider,
            registry,
            permissions,
            model,
            max_tokens,
            system_prompt: None,
            thinking_budget: None,
            effort: None,
            cwd,
            read_only_mode: false,
        }
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = Some(prompt);
    }

    pub fn set_thinking_budget(&mut self, budget: u32) {
        self.thinking_budget = Some(budget);
    }

    pub fn set_effort(&mut self, effort: Option<Effort>) {
        self.effort = effort;
    }

    pub fn effort(&self) -> Option<Effort> {
        self.effort
    }

    pub fn set_read_only_mode(&mut self, enabled: bool) {
        self.read_only_mode = enabled;
    }

    /// Update the agent's working directory (e.g., after exiting a worktree).
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Run the agent loop: send messages, handle tool calls, repeat until done.
    ///
    /// The callback receives AgentEvents as they occur (for streaming UI).
    /// The `cancel` token can be used to abort the loop (e.g. on Ctrl+C).
    pub async fn run<F>(
        &self,
        messages: &mut Vec<Message>,
        cancel: CancellationToken,
        mut on_event: F,
    ) -> Result<Usage, chet_types::ChetError>
    where
        F: FnMut(AgentEvent),
    {
        let mut total_usage = Usage::default();

        for _loop_iter in 0..MAX_TOOL_LOOPS {
            let mut request = self.build_request(messages);
            let stream_result = self.provider.create_message_stream(&request).await;
            *messages = std::mem::take(&mut request.messages);
            let stream = stream_result.map_err(chet_types::ChetError::Api)?;

            let result = self
                .collect_response(stream, &cancel, &mut on_event, &mut total_usage)
                .await?;

            if !result.content_blocks.is_empty() {
                messages.push(Message {
                    role: Role::Assistant,
                    content: result.content_blocks,
                });
            }

            if result.tool_uses.is_empty() || result.stop_reason == Some(StopReason::EndTurn) {
                on_event(AgentEvent::Done);
                on_event(AgentEvent::Usage(total_usage.clone()));
                return Ok(total_usage);
            }

            let ctx = ToolContext {
                cwd: self.cwd.clone(),
                env: std::env::vars().collect(),
                sandboxed: false,
            };

            let check = self
                .check_tool_permissions(&result.tool_uses, &mut on_event)
                .await;
            let mut tool_results = check.tool_results;

            // Execute permitted tools: read-only in parallel, mutating sequentially
            let (read_only, mutating): (Vec<_>, Vec<_>) =
                check.permitted_tools.into_iter().partition(|t| t.4);

            if !read_only.is_empty() {
                let futures: Vec<_> = read_only
                    .iter()
                    .map(|(_, _, name, input, _)| {
                        self.registry.execute(name, input.clone(), ctx.clone())
                    })
                    .collect();

                let cancel_ref = &cancel;
                let results = tokio::select! {
                    _ = cancel_ref.cancelled() => {
                        on_event(AgentEvent::Cancelled);
                        if let Some(last) = messages.last() {
                            if last.role == Role::Assistant { messages.pop(); }
                        }
                        return Err(chet_types::ChetError::Cancelled);
                    }
                    results = futures_util::future::join_all(futures) => results
                };

                for (result, (idx, tool_id, tool_name, tool_input, _)) in
                    results.into_iter().zip(read_only)
                {
                    let output = match result {
                        Ok(output) => output,
                        Err(e) => ToolOutput::error(e.to_string()),
                    };
                    tool_results[idx] = Some(
                        finalize_tool_result(
                            &self.permissions,
                            &self.cwd,
                            &tool_id,
                            &tool_name,
                            &tool_input,
                            output,
                            &mut on_event,
                        )
                        .await,
                    );
                }
            }

            for (idx, tool_id, tool_name, tool_input, _) in mutating {
                let tool_result = tokio::select! {
                    _ = cancel.cancelled() => {
                        on_event(AgentEvent::Cancelled);
                        if let Some(last) = messages.last() {
                            if last.role == Role::Assistant { messages.pop(); }
                        }
                        return Err(chet_types::ChetError::Cancelled);
                    }
                    result = self.registry.execute(&tool_name, tool_input.clone(), ctx.clone()) => result
                };

                let output = match tool_result {
                    Ok(output) => output,
                    Err(e) => ToolOutput::error(e.to_string()),
                };
                tool_results[idx] = Some(
                    finalize_tool_result(
                        &self.permissions,
                        &self.cwd,
                        &tool_id,
                        &tool_name,
                        &tool_input,
                        output,
                        &mut on_event,
                    )
                    .await,
                );
            }

            let tool_results: Vec<ContentBlock> = tool_results.into_iter().flatten().collect();
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

    /// Build the API request, moving messages out for O(1) transfer.
    /// Caller must restore messages from `request.messages` after streaming.
    fn build_request(&self, messages: &mut Vec<Message>) -> CreateMessageRequest {
        let system = self.system_prompt.as_ref().map(|text| {
            vec![SystemContent {
                content_type: "text",
                text: text.clone(),
                cache_control: Some(CacheControl::ephemeral()),
            }]
        });

        let tools = {
            let mut defs = if self.read_only_mode {
                self.registry.read_only_definitions()
            } else {
                self.registry.definitions()
            };
            defs.retain(|d| !self.permissions.is_tool_blocked(&d.name));
            if let Some(last) = defs.last_mut() {
                last.cache_control = Some(CacheControl::ephemeral());
            }
            if defs.is_empty() { None } else { Some(defs) }
        };

        let effective_budget = self
            .thinking_budget
            .or_else(|| self.effort.map(|e| e.budget_tokens()));
        let (thinking, temperature) = if let Some(budget) = effective_budget {
            (
                Some(ThinkingConfig {
                    thinking_type: "enabled".to_string(),
                    budget_tokens: Some(budget),
                }),
                Some(1.0),
            )
        } else {
            (None, None)
        };

        CreateMessageRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: std::mem::take(messages),
            system,
            tools,
            stop_sequences: None,
            temperature,
            thinking,
            stream: true,
        }
    }

    /// Collect the full assistant response from the SSE stream.
    async fn collect_response<F>(
        &self,
        mut stream: EventStream,
        cancel: &CancellationToken,
        on_event: &mut F,
        total_usage: &mut Usage,
    ) -> Result<CollectResult, chet_types::ChetError>
    where
        F: FnMut(AgentEvent),
    {
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_json = String::new();
        let mut current_thinking = String::new();
        let mut current_signature = String::new();
        let mut in_thinking_block = false;
        let mut stop_reason = None;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    on_event(AgentEvent::Cancelled);
                    return Err(chet_types::ChetError::Cancelled);
                }
                event = stream.next() => {
                    match event {
                        Some(Ok(StreamEvent::MessageStart { message })) => {
                            total_usage.add(&message.usage);
                        }
                        Some(Ok(StreamEvent::ContentBlockStart {
                            content_block: ContentBlock::Text { .. },
                            ..
                        })) => {
                            current_text.clear();
                        }
                        Some(Ok(StreamEvent::ContentBlockStart {
                            content_block: ContentBlock::Thinking { .. },
                            ..
                        })) => {
                            current_thinking.clear();
                            current_signature.clear();
                            in_thinking_block = true;
                        }
                        Some(Ok(StreamEvent::ContentBlockStart {
                            content_block: ContentBlock::ToolUse { id, name, .. },
                            ..
                        })) => {
                            current_tool_id = id;
                            current_tool_name = name;
                            current_tool_json.clear();
                            on_event(AgentEvent::ToolStart {
                                name: current_tool_name.clone(),
                                input: String::new(),
                            });
                        }
                        Some(Ok(StreamEvent::ContentBlockDelta { delta, .. })) => match delta {
                            ContentDelta::TextDelta { text } => {
                                on_event(AgentEvent::TextDelta(text.clone()));
                                current_text.push_str(&text);
                            }
                            ContentDelta::InputJsonDelta { partial_json } => {
                                current_tool_json.push_str(&partial_json);
                            }
                            ContentDelta::ThinkingDelta { thinking } => {
                                on_event(AgentEvent::ThinkingDelta(thinking.clone()));
                                current_thinking.push_str(&thinking);
                            }
                            ContentDelta::SignatureDelta { signature } => {
                                current_signature.push_str(&signature);
                            }
                        },
                        Some(Ok(StreamEvent::ContentBlockStop { .. })) => {
                            if in_thinking_block {
                                if !current_thinking.is_empty() {
                                    content_blocks.push(ContentBlock::Thinking {
                                        thinking: current_thinking.clone(),
                                        signature: if current_signature.is_empty() {
                                            None
                                        } else {
                                            Some(current_signature.clone())
                                        },
                                    });
                                }
                                current_thinking.clear();
                                current_signature.clear();
                                in_thinking_block = false;
                            } else if !current_text.is_empty() {
                                content_blocks.push(ContentBlock::Text {
                                    text: current_text.clone(),
                                });
                                current_text.clear();
                            } else if !current_tool_id.is_empty() {
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
                        Some(Ok(StreamEvent::MessageDelta { delta, usage })) => {
                            stop_reason = delta.stop_reason;
                            if let Some(u) = usage {
                                total_usage.add(&u);
                            }
                        }
                        Some(Ok(StreamEvent::Error { error })) => {
                            let msg = format!("{}: {}", error.error_type, error.message);
                            on_event(AgentEvent::Error(msg.clone()));
                            fire_stop_failure_hook(&self.permissions, &msg).await;
                            return Err(chet_types::ChetError::Api(chet_types::ApiError::Server {
                                status: 0,
                                message: error.message,
                            }));
                        }
                        Some(Ok(_)) => {} // Ping, MessageStop
                        Some(Err(e)) => {
                            on_event(AgentEvent::Error(e.to_string()));
                            fire_stop_failure_hook(&self.permissions, &e.to_string()).await;
                            return Err(chet_types::ChetError::Api(e));
                        }
                        None => break, // Stream ended
                    }
                }
            }
        }

        let tool_uses: Vec<_> = content_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();

        Ok(CollectResult {
            content_blocks,
            stop_reason,
            tool_uses,
        })
    }

    /// Check permissions and run before_tool hooks for each tool use.
    async fn check_tool_permissions<F>(
        &self,
        tool_uses: &[(String, String, serde_json::Value)],
        on_event: &mut F,
    ) -> PermissionCheckResult
    where
        F: FnMut(AgentEvent),
    {
        let mut tool_results: Vec<Option<ContentBlock>> = vec![None; tool_uses.len()];
        let mut permitted_tools: Vec<(usize, String, String, serde_json::Value, bool)> = Vec::new();

        for (i, (tool_id, tool_name, tool_input)) in tool_uses.iter().enumerate() {
            let is_read_only = self.registry.is_read_only(tool_name).unwrap_or(false);

            if self.read_only_mode && !is_read_only {
                on_event(AgentEvent::ToolBlocked {
                    name: tool_name.clone(),
                    reason: "plan mode (read-only)".to_string(),
                });
                tool_results[i] = Some(ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content: vec![ToolResultContent::Text {
                        text: "Blocked: plan mode only allows read-only tools".to_string(),
                    }],
                    is_error: Some(true),
                });
                continue;
            }

            let decision = self.permissions.check(tool_name, tool_input, is_read_only);

            let permitted = match decision {
                PermissionDecision::Permit => true,
                PermissionDecision::Block { reason } => {
                    on_event(AgentEvent::ToolBlocked {
                        name: tool_name.clone(),
                        reason: reason.clone(),
                    });
                    tool_results[i] = Some(ContentBlock::ToolResult {
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
                            tool_results[i] = Some(ContentBlock::ToolResult {
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

            let hook_input = HookInput {
                event: HookEvent::BeforeTool,
                tool_name: Some(tool_name.clone()),
                tool_input: Some(tool_input.clone()),
                tool_output: None,
                is_error: None,
                worktree_path: None,
                worktree_source: None,
                messages_removed: None,
                messages_remaining: None,
                config_path: None,
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
                tool_results[i] = Some(ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content: vec![ToolResultContent::Text {
                        text: format!("Blocked by hook: {reason}"),
                    }],
                    is_error: Some(true),
                });
                continue;
            }

            permitted_tools.push((
                i,
                tool_id.clone(),
                tool_name.clone(),
                tool_input.clone(),
                is_read_only,
            ));
        }

        PermissionCheckResult {
            tool_results,
            permitted_tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::truncate_for_display;
    use chet_api::AnthropicProvider;

    fn make_provider() -> Arc<dyn Provider> {
        Arc::new(AnthropicProvider::new("test-key", "https://api.example.com").unwrap())
    }

    #[test]
    fn agent_event_cancelled_debug() {
        let event = AgentEvent::Cancelled;
        assert_eq!(format!("{event:?}"), "Cancelled");
    }

    #[test]
    fn chet_error_cancelled_display() {
        let err = chet_types::ChetError::Cancelled;
        assert_eq!(err.to_string(), "Operation cancelled");
    }

    #[test]
    fn cancellation_token_starts_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancellation_token_can_cancel() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn read_only_mode_defaults_false() {
        let provider = make_provider();
        let registry = ToolRegistry::new();
        let permissions = Arc::new(PermissionEngine::ludicrous());
        let agent = Agent::new(
            provider,
            registry,
            permissions,
            "test".into(),
            1024,
            PathBuf::from("/tmp"),
        );
        assert!(!agent.read_only_mode);
    }

    #[test]
    fn set_read_only_mode_toggles() {
        let provider = make_provider();
        let registry = ToolRegistry::new();
        let permissions = Arc::new(PermissionEngine::ludicrous());
        let mut agent = Agent::new(
            provider,
            registry,
            permissions,
            "test".into(),
            1024,
            PathBuf::from("/tmp"),
        );
        agent.set_read_only_mode(true);
        assert!(agent.read_only_mode);
        agent.set_read_only_mode(false);
        assert!(!agent.read_only_mode);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_for_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate_for_display("hello world", 5);
        assert_eq!(result, "hello...");
    }
}
