//! Conversation compaction — extract key facts and archive the full history.

use chet_types::{ContentBlock, Message, Role};

/// Minimum number of messages before compaction is allowed.
const MIN_MESSAGES_FOR_COMPACTION: usize = 12;

/// Number of recent turn-pairs to preserve after compaction.
const KEEP_RECENT_TURNS: usize = 5;

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Full conversation history as markdown (for archiving).
    pub archive_markdown: String,
    /// New messages list: summary + recent messages.
    pub new_messages: Vec<Message>,
    /// How many messages were removed from the conversation.
    pub messages_removed: usize,
}

/// Compact a conversation by extracting key facts and preserving recent messages.
///
/// An optional `label` (e.g. the session label) is prepended to the summary so
/// it survives compaction.
///
/// Returns `None` if the conversation is too short to compact.
pub fn compact(messages: &[Message], label: Option<&str>) -> Option<CompactionResult> {
    if messages.len() < MIN_MESSAGES_FOR_COMPACTION {
        return None;
    }

    // Find the split point: keep the last KEEP_RECENT_TURNS turn-pairs
    let split = find_split_point(messages);
    if split == 0 {
        return None;
    }

    let old_messages = &messages[..split];
    let recent_messages = &messages[split..];

    // Build the full archive
    let archive_markdown = messages_to_markdown(messages);

    // Extract key facts from the old messages
    let facts = extract_key_facts(old_messages);
    let summary_msg = build_summary_message(&facts, label);

    let mut new_messages = vec![summary_msg];
    new_messages.extend(recent_messages.iter().map(strip_heavy_payloads));

    Some(CompactionResult {
        archive_markdown,
        new_messages,
        messages_removed: old_messages.len(),
    })
}

/// Find the index to split at — keep the last N turn-pairs.
fn find_split_point(messages: &[Message]) -> usize {
    // Walk backward counting user messages (each is roughly a "turn")
    let mut user_count = 0;
    let mut split_idx = messages.len();

    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == Role::User {
            user_count += 1;
            if user_count >= KEEP_RECENT_TURNS {
                split_idx = i;
                break;
            }
        }
    }

    // If we couldn't find enough turns to keep, don't compact
    if split_idx == 0 || split_idx >= messages.len() {
        return 0;
    }

    split_idx
}

/// Extract key facts from messages for the compaction summary.
fn extract_key_facts(messages: &[Message]) -> Vec<String> {
    let mut facts = Vec::new();
    let mut files_read = Vec::new();
    let mut files_written = Vec::new();
    let mut tools_used = Vec::new();
    let mut errors = Vec::new();

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { name, input, .. } => {
                    if !tools_used.contains(name) {
                        tools_used.push(name.clone());
                    }

                    // Extract file paths from common tools
                    if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                        match name.as_str() {
                            "Read" | "Glob" | "Grep" => {
                                if !files_read.contains(&path.to_string()) {
                                    files_read.push(path.to_string());
                                }
                            }
                            "Write" | "Edit" => {
                                if !files_written.contains(&path.to_string()) {
                                    files_written.push(path.to_string());
                                }
                            }
                            _ => {}
                        }
                    }

                    // Extract command from Bash tool
                    if name == "Bash" {
                        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                            let short = if cmd.len() > 60 {
                                format!("{}...", chet_types::truncate_str(cmd, 57))
                            } else {
                                cmd.to_string()
                            };
                            facts.push(format!("Ran command: `{short}`"));
                        }
                    }
                }
                ContentBlock::ToolResult {
                    is_error: Some(true),
                    content,
                    ..
                } => {
                    for c in content {
                        if let chet_types::ToolResultContent::Text { text } = c {
                            let short = if text.len() > 100 {
                                format!("{}...", chet_types::truncate_str(text, 97))
                            } else {
                                text.clone()
                            };
                            errors.push(short);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if !tools_used.is_empty() {
        facts.push(format!("Tools used: {}", tools_used.join(", ")));
    }
    if !files_read.is_empty() {
        facts.push(format!("Files read: {}", files_read.join(", ")));
    }
    if !files_written.is_empty() {
        facts.push(format!("Files modified: {}", files_written.join(", ")));
    }
    for err in &errors {
        facts.push(format!("Error encountered: {err}"));
    }

    facts
}

/// Maximum chars for a tool result text block in preserved (recent) messages.
/// Longer results are truncated with a marker. This prevents large file reads,
/// grep outputs, and bash outputs from bloating context after compaction.
const MAX_TOOL_RESULT_CHARS: usize = 4000;

/// Strip heavy payloads from a message to reduce context bloat after compaction.
/// - Truncates large ToolResult text blocks
/// - Removes Thinking blocks (output-only, not counted in input context)
fn strip_heavy_payloads(msg: &Message) -> Message {
    let content = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let stripped_content = content
                    .iter()
                    .map(|c| match c {
                        chet_types::ToolResultContent::Text { text }
                            if text.len() > MAX_TOOL_RESULT_CHARS =>
                        {
                            chet_types::ToolResultContent::Text {
                                text: format!(
                                    "{}\n... (truncated from {} chars during compaction)",
                                    chet_types::truncate_str(text, MAX_TOOL_RESULT_CHARS),
                                    text.len()
                                ),
                            }
                        }
                        other => other.clone(),
                    })
                    .collect();
                Some(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: stripped_content,
                    is_error: *is_error,
                })
            }
            // Drop thinking blocks — they're not part of input context
            ContentBlock::Thinking { .. } => None,
            other => Some(other.clone()),
        })
        .collect();

    Message {
        role: msg.role,
        content,
    }
}

/// Convert the full conversation to a markdown archive.
fn messages_to_markdown(messages: &[Message]) -> String {
    let mut md = String::from("# Conversation Archive\n\n");

    for msg in messages {
        let role_label = match msg.role {
            Role::User => "**User**",
            Role::Assistant => "**Assistant**",
        };
        md.push_str(&format!("## {role_label}\n\n"));

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    md.push_str(text);
                    md.push_str("\n\n");
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    md.push_str(&format!("**Tool call: {name}**\n"));
                    md.push_str(&format!("```json\n{}\n```\n\n", input));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error == Some(true) {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    md.push_str(&format!("**{label}:**\n"));
                    for c in content {
                        if let chet_types::ToolResultContent::Text { text } = c {
                            // Truncate very long tool results in the archive
                            if text.len() > 2000 {
                                md.push_str(&format!(
                                    "```\n{}...\n(truncated)\n```\n\n",
                                    chet_types::truncate_str(text, 2000)
                                ));
                            } else {
                                md.push_str(&format!("```\n{text}\n```\n\n"));
                            }
                        }
                    }
                }
                ContentBlock::Thinking { thinking, .. } => {
                    md.push_str(&format!(
                        "*Thinking: {}*\n\n",
                        chet_types::truncate_str(thinking, 200)
                    ));
                }
                ContentBlock::Image { .. } => {
                    md.push_str("*[Image]*\n\n");
                }
            }
        }
    }

    md
}

/// Build a summary user message from extracted facts.
fn build_summary_message(facts: &[String], label: Option<&str>) -> Message {
    let mut text = String::new();

    if let Some(label) = label {
        text.push_str(&format!("[Session: {label}]\n\n"));
    }

    text.push_str("[Compacted conversation summary:]\n\n");

    if facts.is_empty() {
        text.push_str("- (No specific facts extracted from the compacted portion)\n");
    } else {
        for fact in facts {
            text.push_str(&format!("- {fact}\n"));
        }
    }

    Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_types::{ContentBlock, Message, Role, ToolResultContent};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn long_conversation() -> Vec<Message> {
        let mut msgs = Vec::new();
        for i in 0..10 {
            msgs.push(text_msg(Role::User, &format!("Question {i}")));
            msgs.push(text_msg(Role::Assistant, &format!("Answer {i}")));
        }
        msgs
    }

    #[test]
    fn short_conversation_returns_none() {
        let msgs = vec![
            text_msg(Role::User, "Hello"),
            text_msg(Role::Assistant, "Hi"),
        ];
        assert!(compact(&msgs, None).is_none());
    }

    #[test]
    fn preserves_recent_messages() {
        let msgs = long_conversation();
        let result = compact(&msgs, None).unwrap();
        // Should have summary + recent messages
        assert!(result.new_messages.len() < msgs.len());
        // Last message should be the same as original
        let last_orig = msgs.last().unwrap();
        let last_new = result.new_messages.last().unwrap();
        if let (ContentBlock::Text { text: orig }, ContentBlock::Text { text: new }) =
            (&last_orig.content[0], &last_new.content[0])
        {
            assert_eq!(orig, new);
        }
    }

    #[test]
    fn generates_archive() {
        let msgs = long_conversation();
        let result = compact(&msgs, None).unwrap();
        assert!(result.archive_markdown.contains("# Conversation Archive"));
        assert!(result.archive_markdown.contains("Question 0"));
    }

    #[test]
    fn extracts_tool_calls() {
        let mut msgs = long_conversation();
        msgs.insert(
            2,
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "/src/main.rs"}),
                }],
            },
        );
        msgs.insert(
            3,
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: vec![ToolResultContent::Text {
                        text: "fn main() {}".into(),
                    }],
                    is_error: None,
                }],
            },
        );
        let result = compact(&msgs, None).unwrap();
        let summary_text = match &result.new_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(summary_text.contains("Read"));
    }

    #[test]
    fn extracts_errors() {
        let mut msgs = long_conversation();
        msgs.insert(
            2,
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: vec![ToolResultContent::Text {
                        text: "Permission denied".into(),
                    }],
                    is_error: Some(true),
                }],
            },
        );
        let result = compact(&msgs, None).unwrap();
        let summary_text = match &result.new_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(summary_text.contains("Permission denied"));
    }

    #[test]
    fn summary_is_user_role() {
        let msgs = long_conversation();
        let result = compact(&msgs, None).unwrap();
        assert_eq!(result.new_messages[0].role, Role::User);
    }

    #[test]
    fn messages_removed_count() {
        let msgs = long_conversation();
        let result = compact(&msgs, None).unwrap();
        assert!(result.messages_removed > 0);
        // new_messages = 1 (summary) + preserved recent
        assert_eq!(
            result.new_messages.len(),
            1 + (msgs.len() - result.messages_removed)
        );
    }

    #[test]
    fn compact_with_label_includes_session_label() {
        let msgs = long_conversation();
        let result = compact(&msgs, Some("Fix auth bug")).unwrap();
        let summary_text = match &result.new_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(summary_text.contains("[Session: Fix auth bug]"));
    }

    #[test]
    fn strip_heavy_payloads_truncates_large_tool_results() {
        let big_text = "x".repeat(10_000);
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: vec![ToolResultContent::Text { text: big_text }],
                is_error: None,
            }],
        };
        let stripped = strip_heavy_payloads(&msg);
        if let ContentBlock::ToolResult { content, .. } = &stripped.content[0] {
            if let ToolResultContent::Text { text } = &content[0] {
                assert!(text.len() < 5000, "should be truncated: len={}", text.len());
                assert!(text.contains("truncated"));
            } else {
                panic!("expected text");
            }
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn strip_heavy_payloads_preserves_small_tool_results() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: vec![ToolResultContent::Text {
                    text: "small output".into(),
                }],
                is_error: None,
            }],
        };
        let stripped = strip_heavy_payloads(&msg);
        if let ContentBlock::ToolResult { content, .. } = &stripped.content[0] {
            if let ToolResultContent::Text { text } = &content[0] {
                assert_eq!(text, "small output");
            } else {
                panic!("expected text");
            }
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn strip_heavy_payloads_removes_thinking_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "very long thinking...".repeat(100),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
        };
        let stripped = strip_heavy_payloads(&msg);
        assert_eq!(stripped.content.len(), 1);
        assert!(matches!(&stripped.content[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn compaction_strips_heavy_payloads_in_recent() {
        let mut msgs = long_conversation();
        // Add a large tool result in the recent window (near the end)
        let big_result = "y".repeat(10_000);
        msgs.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "another question".into(),
            }],
        });
        msgs.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/big/file.rs"}),
            }],
        });
        msgs.push(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: vec![ToolResultContent::Text { text: big_result }],
                is_error: None,
            }],
        });
        msgs.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
        });

        let result = compact(&msgs, None).unwrap();
        // Find the tool result in recent messages
        let has_truncated = result.new_messages.iter().any(|m| {
            m.content.iter().any(|b| {
                if let ContentBlock::ToolResult { content, .. } = b {
                    content.iter().any(|c| {
                        if let ToolResultContent::Text { text } = c {
                            text.contains("truncated")
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            })
        });
        assert!(
            has_truncated,
            "large tool result should be truncated in recent messages"
        );
    }

    #[test]
    fn compact_without_label_omits_session_line() {
        let msgs = long_conversation();
        let result = compact(&msgs, None).unwrap();
        let summary_text = match &result.new_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(!summary_text.contains("[Session:"));
    }
}
