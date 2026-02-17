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

    /// Format a detailed multi-line context breakdown.
    pub fn format_detailed(&self, info: &ContextInfo) -> String {
        let mut lines = Vec::new();
        let est_k = info.estimated_tokens as f64 / 1000.0;
        let win_k = info.context_window as f64 / 1000.0;
        lines.push(format!(
            "Context window: {est_k:.1}k / {win_k:.0}k tokens ({:.1}%)",
            info.usage_percent()
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
        lines.join("\n")
    }
}

/// Estimate tokens for a text string (chars / 4 heuristic).
pub fn estimate_text_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Estimate tokens for a single content block.
fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text { text } => estimate_text_tokens(text),
        ContentBlock::ToolUse { name, input, .. } => {
            let input_str = input.to_string();
            estimate_text_tokens(name) + estimate_text_tokens(&input_str)
        }
        ContentBlock::ToolResult { content, .. } => {
            let mut tokens = 0u64;
            for c in content {
                match c {
                    ToolResultContent::Text { text } => tokens += estimate_text_tokens(text),
                    ToolResultContent::Image { .. } => tokens += 1000, // rough image estimate
                }
            }
            tokens
        }
        ContentBlock::Thinking { thinking, .. } => estimate_text_tokens(thinking),
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
    let model_lower = model.to_lowercase();
    if model_lower.contains("claude") {
        200_000
    } else {
        128_000 // sensible default
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
    fn model_detection_claude() {
        let tracker = ContextTracker::new("claude-opus-4-6");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.context_window, 200_000);
    }

    #[test]
    fn model_detection_default() {
        let tracker = ContextTracker::new("some-unknown-model");
        let info = tracker.estimate(&[], None);
        assert_eq!(info.context_window, 128_000);
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
}
