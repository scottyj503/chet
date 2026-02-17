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
/// Returns `None` if the conversation is too short to compact.
pub fn compact(messages: &[Message]) -> Option<CompactionResult> {
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
    let summary_msg = build_summary_message(&facts);

    let mut new_messages = vec![summary_msg];
    new_messages.extend_from_slice(recent_messages);

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
                                format!("{}...", &cmd[..57])
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
                                format!("{}...", &text[..97])
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
                                    &text[..2000]
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
                        &thinking[..thinking.len().min(200)]
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
fn build_summary_message(facts: &[String]) -> Message {
    let mut text = String::from(
        "[This conversation was compacted. Key facts from the earlier conversation:]\n\n",
    );

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
        assert!(compact(&msgs).is_none());
    }

    #[test]
    fn preserves_recent_messages() {
        let msgs = long_conversation();
        let result = compact(&msgs).unwrap();
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
        let result = compact(&msgs).unwrap();
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
        let result = compact(&msgs).unwrap();
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
        let result = compact(&msgs).unwrap();
        let summary_text = match &result.new_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(summary_text.contains("Permission denied"));
    }

    #[test]
    fn summary_is_user_role() {
        let msgs = long_conversation();
        let result = compact(&msgs).unwrap();
        assert_eq!(result.new_messages[0].role, Role::User);
    }

    #[test]
    fn messages_removed_count() {
        let msgs = long_conversation();
        let result = compact(&msgs).unwrap();
        assert!(result.messages_removed > 0);
        // new_messages = 1 (summary) + preserved recent
        assert_eq!(
            result.new_messages.len(),
            1 + (msgs.len() - result.messages_removed)
        );
    }
}
