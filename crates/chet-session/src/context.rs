//! Context window tracking and token estimation.

use chet_types::{ContentBlock, Message, Role, ToolResultContent};

/// Information about current context window usage.
#[derive(Debug, Clone)]
pub struct ContextInfo {
    pub estimated_tokens: u64,
    pub context_window: u64,
    pub user_tokens: u64,
    pub assistant_tokens: u64,
    pub system_tokens: u64,
    pub last_turn_input_tokens: u64,
    pub last_turn_output_tokens: u64,
}

impl ContextInfo {
    /// Usage as a percentage of the context window.
    pub fn usage_percent(&self) -> f64 {
        if self.context_window == 0 {
            return 0.0;
        }
        (self.estimated_tokens as f64 / self.context_window as f64) * 100.0
    }
}

/// Tracks context window usage for a conversation.
pub struct ContextTracker {
    context_window: u64,
}

impl ContextTracker {
    /// Create a tracker for the given model.
    pub fn new(model: &str) -> Self {
        Self {
            context_window: model_context_window(model),
        }
    }

    /// Estimate token usage for the current conversation state.
    pub fn estimate(&self, messages: &[Message], system_prompt: Option<&str>) -> ContextInfo {
        let system_tokens = system_prompt.map(estimate_text_tokens).unwrap_or(0);
        let mut user_tokens = 0u64;
        let mut assistant_tokens = 0u64;
        let mut last_turn_input_tokens = 0u64;
        let mut last_turn_output_tokens = 0u64;

        for msg in messages {
            let msg_tokens = estimate_message_tokens(msg);
            match msg.role {
                Role::User => user_tokens += msg_tokens,
                Role::Assistant => assistant_tokens += msg_tokens,
            }
        }

        // Last turn: find the final assistant message and the user message(s) before it
        if let Some(last_asst_idx) = messages.iter().rposition(|m| m.role == Role::Assistant) {
            last_turn_output_tokens = estimate_message_tokens(&messages[last_asst_idx]);
            // Input for the last turn = the user message(s) right before the last assistant msg
            for i in (0..last_asst_idx).rev() {
                if messages[i].role == Role::User {
                    last_turn_input_tokens += estimate_message_tokens(&messages[i]);
                } else {
                    break;
                }
            }
        }

        let estimated_tokens = system_tokens + user_tokens + assistant_tokens;

        ContextInfo {
            estimated_tokens,
            context_window: self.context_window,
            user_tokens,
            assistant_tokens,
            system_tokens,
            last_turn_input_tokens,
            last_turn_output_tokens,
        }
    }

    /// Format a brief one-line context summary.
    pub fn format_brief(&self, info: &ContextInfo) -> String {
        let est_k = info.estimated_tokens as f64 / 1000.0;
        let win_k = info.context_window as f64 / 1000.0;
        format!(
            "Context: {est_k:.1}k/{win_k:.0}k tokens ({:.0}%)",
            info.usage_percent()
        )
    }

    /// Format a detailed multi-line context breakdown with actionable suggestions.
    pub fn format_detailed(&self, info: &ContextInfo) -> String {
        let mut lines = Vec::new();
        let est_k = info.estimated_tokens as f64 / 1000.0;
        let win_k = info.context_window as f64 / 1000.0;
        let pct = info.usage_percent();
        lines.push(format!(
            "Context window: {est_k:.1}k / {win_k:.0}k tokens ({pct:.1}%)",
        ));
        lines.push(format!("  System:    ~{} tokens", info.system_tokens));
        lines.push(format!("  User:      ~{} tokens", info.user_tokens));
        lines.push(format!("  Assistant: ~{} tokens", info.assistant_tokens));
        if info.last_turn_input_tokens > 0 || info.last_turn_output_tokens > 0 {
            lines.push(format!(
                "  Last turn: ~{} in / ~{} out",
                info.last_turn_input_tokens, info.last_turn_output_tokens
            ));
        }

        // Actionable suggestions
        if pct > 80.0 {
            lines.push(String::new());
            lines.push("Suggestions:".to_string());
            lines.push(
                "  - Run /compact to archive old messages and free context space".to_string(),
            );
        } else if pct > 50.0 {
            lines.push(String::new());
            lines.push("Suggestions:".to_string());
            lines.push(
                "  - Consider /compact if responses seem to lose earlier context".to_string(),
            );
        }
        if info.system_tokens > info.context_window / 5 {
            if !lines.last().is_some_and(|l| l.starts_with("  - ")) {
                lines.push(String::new());
                lines.push("Suggestions:".to_string());
            }
            lines.push(
                "  - System prompt is large; trim memory with /memory reset if stale".to_string(),
            );
        }

        lines.join("\n")
    }
}

/// Estimate tokens for a text string (~3.5 chars/token for prose/code).
pub fn estimate_text_tokens(text: &str) -> u64 {
    // 3.5 chars/token is more accurate than 4 for mixed code/prose.
    // Multiply by 2, divide by 7 to avoid floating point: ceil(len * 2 / 7)
    let len = text.len() as u64;
    (len * 2).div_ceil(7) // equivalent to ceil(len / 3.5)
}

/// Estimate tokens for JSON/structured data (~5 chars/token — structural
/// characters like `{`, `"`, `:` tokenize more efficiently).
fn estimate_json_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(5)
}

/// Estimate tokens for a single content block.
fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text { text } => estimate_text_tokens(text),
        ContentBlock::ToolUse { name, input, .. } => {
            // Tool use has: id (~36 chars / ~10 tokens), name, and JSON input.
            // Use JSON estimation for the input since it's structured data.
            let input_str = input.to_string();
            10 + estimate_text_tokens(name) + estimate_json_tokens(&input_str)
        }
        ContentBlock::ToolResult { content, .. } => {
            // tool_use_id overhead (~10 tokens) + content
            let mut tokens = 10u64;
            for c in content {
                match c {
                    ToolResultContent::Text { text } => tokens += estimate_text_tokens(text),
                    ToolResultContent::Image { .. } => tokens += 1000,
                }
            }
            tokens
        }
        // Thinking blocks are NOT counted — they're output-only and Anthropic
        // does not include them in input token counts for subsequent turns.
        ContentBlock::Thinking { .. } => 0,
        ContentBlock::Image { .. } => 1000,
    }
}

/// Estimate tokens for an entire message.
fn estimate_message_tokens(msg: &Message) -> u64 {
    let mut tokens = 4u64; // message overhead (role, separators)
    for block in &msg.content {
        tokens += estimate_block_tokens(block);
    }
    tokens
}

/// Look up the context window size for a model.
fn model_context_window(model: &str) -> u64 {
    if model.contains("opus") {
        1_000_000
    } else if model.contains("sonnet") || model.contains("haiku") {
        200_000
    } else {
        200_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_types::{ContentBlock, Message, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn empty_messages() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.estimated_tokens, 0);
        assert_eq!(info.context_window, 200_000);
    }

    #[test]
    fn single_message() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![text_msg(Role::User, "Hello world")];
        let info = tracker.estimate(&msgs, None);
        assert!(info.user_tokens > 0);
        assert_eq!(info.assistant_tokens, 0);
        assert_eq!(info.estimated_tokens, info.user_tokens);
    }

    #[test]
    fn with_system_prompt() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![text_msg(Role::User, "Hi")];
        let info = tracker.estimate(&msgs, Some("You are a helpful assistant."));
        assert!(info.system_tokens > 0);
        assert_eq!(info.estimated_tokens, info.system_tokens + info.user_tokens);
    }

    #[test]
    fn with_tool_use_blocks() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "id1".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            }],
        }];
        let info = tracker.estimate(&msgs, None);
        assert!(info.assistant_tokens > 0);
    }

    #[test]
    fn model_detection_opus() {
        let tracker = ContextTracker::new("claude-opus-4-6");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.context_window, 1_000_000);
    }

    #[test]
    fn model_detection_sonnet() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.context_window, 200_000);
    }

    #[test]
    fn model_detection_default() {
        let tracker = ContextTracker::new("some-unknown-model");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.context_window, 200_000);
    }

    #[test]
    fn format_brief_output() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![text_msg(Role::User, "Hello world")];
        let info = tracker.estimate(&msgs, None);
        let brief = tracker.format_brief(&info);
        assert!(brief.contains("Context:"));
        assert!(brief.contains("/200k tokens"));
    }

    #[test]
    fn last_turn_tracking() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![
            text_msg(Role::User, "First question"),
            text_msg(Role::Assistant, "First answer"),
            text_msg(Role::User, "Second question"),
            text_msg(Role::Assistant, "Second answer"),
        ];
        let info = tracker.estimate(&msgs, None);
        assert!(info.last_turn_input_tokens > 0);
        assert!(info.last_turn_output_tokens > 0);
    }

    #[test]
    fn thinking_blocks_not_counted() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "Let me think about this for a very long time...".repeat(100),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "Here's my answer.".to_string(),
                },
            ],
        }];
        let info = tracker.estimate(&msgs, None);
        // Only the text block + message overhead should be counted
        let text_only = estimate_text_tokens("Here's my answer.") + 4;
        assert_eq!(info.assistant_tokens, text_only);
    }

    #[test]
    fn tool_use_json_estimated_efficiently() {
        // JSON input should use chars/5 (estimate_json_tokens), not chars/3.5
        let big_json = serde_json::json!({"file_path": "/home/user/very/long/path/to/file.rs"});
        let input_str = big_json.to_string();
        let json_est = estimate_json_tokens(&input_str);
        let text_est = estimate_text_tokens(&input_str);
        // JSON estimation should be lower than text estimation
        assert!(
            json_est < text_est,
            "json={json_est} text={text_est} for {input_str}"
        );
    }

    #[test]
    fn estimate_text_tokens_accuracy() {
        // "Hello world" = 11 chars, ~3.5 chars/token ≈ 3-4 tokens
        let tokens = estimate_text_tokens("Hello world");
        assert!((3..=4).contains(&tokens), "got {tokens}");
    }

    #[test]
    fn estimate_json_tokens_lower_than_text() {
        let json_str = r#"{"file_path": "/tmp/test.rs", "offset": 1, "limit": 100}"#;
        let json_est = estimate_json_tokens(json_str);
        let text_est = estimate_text_tokens(json_str);
        assert!(json_est < text_est, "json={json_est} text={text_est}");
    }

    #[test]
    fn format_detailed_suggests_compact_at_high_usage() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        // Build info manually to simulate >80% usage
        let info = ContextInfo {
            estimated_tokens: 170_000,
            context_window: 200_000,
            user_tokens: 80_000,
            assistant_tokens: 80_000,
            system_tokens: 10_000,
            last_turn_input_tokens: 0,
            last_turn_output_tokens: 0,
        };
        let output = tracker.format_detailed(&info);
        assert!(output.contains("/compact"));
        assert!(output.contains("Suggestions:"));
    }

    #[test]
    fn format_detailed_no_suggestions_at_low_usage() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        let info = ContextInfo {
            estimated_tokens: 5_000,
            context_window: 200_000,
            user_tokens: 2_000,
            assistant_tokens: 2_000,
            system_tokens: 1_000,
            last_turn_input_tokens: 0,
            last_turn_output_tokens: 0,
        };
        let output = tracker.format_detailed(&info);
        assert!(!output.contains("Suggestions:"));
    }

    #[test]
    fn format_detailed_warns_large_system_prompt() {
        let tracker = ContextTracker::new("claude-sonnet-4-5-20250929");
        // system_tokens > context_window / 5 = 40k
        let info = ContextInfo {
            estimated_tokens: 60_000,
            context_window: 200_000,
            user_tokens: 10_000,
            assistant_tokens: 10_000,
            system_tokens: 41_000,
            last_turn_input_tokens: 0,
            last_turn_output_tokens: 0,
        };
        let output = tracker.format_detailed(&info);
        assert!(output.contains("/memory reset"));
    }
}
