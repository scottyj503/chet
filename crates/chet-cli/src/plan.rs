//! Plan mode logic: approval prompts, plan file saving, and turn management.

use chet_session::Session;
use chet_types::{ContentBlock, Message, Role};
use chrono::Utc;
use std::io::{self, BufRead, Write};

pub(crate) enum PlanApproval {
    Approve,
    Refine,
    Discard,
}

/// Prompt the user to approve, refine, or discard a plan.
pub(crate) async fn prompt_plan_approval() -> PlanApproval {
    eprint!("  > ");
    let _ = io::stderr().flush();

    let result = tokio::task::spawn_blocking(|| {
        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line).ok();
        line
    })
    .await
    .unwrap_or_default();

    match result.trim().to_lowercase().as_str() {
        "a" | "approve" => PlanApproval::Approve,
        "r" | "refine" => PlanApproval::Refine,
        "d" | "discard" => PlanApproval::Discard,
        _ => {
            eprintln!("Invalid choice, defaulting to refine.");
            PlanApproval::Refine
        }
    }
}

/// Extract text content from the last assistant message.
pub(crate) fn extract_last_assistant_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
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
}

/// Save plan text to ~/.chet/plans/<short-session-id>-<timestamp>.md
pub(crate) async fn save_plan_file(
    config_dir: &std::path::Path,
    session: &Session,
    plan_text: &str,
) -> Option<std::path::PathBuf> {
    let plans_dir = config_dir.join("plans");
    if let Err(e) = tokio::fs::create_dir_all(&plans_dir).await {
        eprintln!("Warning: failed to create plans directory: {e}");
        return None;
    }

    let timestamp = Utc::now().format("%Y%m%dT%H%M");
    let filename = format!("{}-{}.md", session.short_id(), timestamp);
    let path = plans_dir.join(&filename);

    let tmp = path.with_extension("tmp");
    match tokio::fs::write(&tmp, plan_text).await {
        Ok(()) => match tokio::fs::rename(&tmp, &path).await {
            Ok(()) => Some(path),
            Err(e) => {
                eprintln!("Warning: failed to save plan file: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("Warning: failed to save plan file: {e}");
            None
        }
    }
}

/// Remove the last turn (user message + assistant response + any tool-result messages).
/// Peels from the end: assistant messages, tool-result user messages, then the triggering user text.
pub(crate) fn pop_last_turn(messages: &mut Vec<Message>) {
    // Pop trailing assistant messages
    while let Some(last) = messages.last() {
        if last.role == Role::Assistant {
            messages.pop();
        } else {
            break;
        }
    }

    // Pop tool-result user messages (content is all ToolResult blocks)
    while let Some(last) = messages.last() {
        if last.role == Role::User && is_tool_result_message(last) {
            messages.pop();
        } else {
            break;
        }
    }

    // Pop assistant messages interleaved with tool results
    while let Some(last) = messages.last() {
        if last.role == Role::Assistant {
            messages.pop();
            // After popping assistant, check for more tool-result messages
            while let Some(last) = messages.last() {
                if last.role == Role::User && is_tool_result_message(last) {
                    messages.pop();
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }

    // Pop the triggering user text message
    if let Some(last) = messages.last() {
        if last.role == Role::User && !is_tool_result_message(last) {
            messages.pop();
        }
    }
}

/// Extract a session label from plan text.
/// Uses the first markdown heading, or the first non-empty line, truncated to 60 chars.
pub(crate) fn label_from_plan(plan_text: &str) -> Option<String> {
    for line in plan_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip markdown heading prefix
        let label = trimmed.trim_start_matches('#').trim();
        if label.is_empty() {
            continue;
        }
        return Some(chet_types::truncate_str(label, 60).to_string());
    }
    None
}

fn is_tool_result_message(msg: &Message) -> bool {
    !msg.content.is_empty()
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_types::ToolResultContent;

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn tool_result_msg(tool_use_id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: vec![ToolResultContent::Text {
                    text: text.to_string(),
                }],
                is_error: None,
            }],
        }
    }

    #[test]
    fn pop_last_turn_simple_exchange() {
        let mut msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "hi there"),
        ];
        pop_last_turn(&mut msgs);
        assert!(msgs.is_empty());
    }

    #[test]
    fn pop_last_turn_with_tool_results() {
        let mut msgs = vec![
            text_msg(Role::User, "earlier question"),
            text_msg(Role::Assistant, "earlier answer"),
            text_msg(Role::User, "read my files"),
            text_msg(Role::Assistant, "let me read"),
            tool_result_msg("t1", "file contents"),
            text_msg(Role::Assistant, "here's the answer"),
        ];
        pop_last_turn(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn pop_last_turn_preserves_earlier_messages() {
        let mut msgs = vec![
            text_msg(Role::User, "first"),
            text_msg(Role::Assistant, "first reply"),
            text_msg(Role::User, "second"),
            text_msg(Role::Assistant, "second reply"),
        ];
        pop_last_turn(&mut msgs);
        assert_eq!(msgs.len(), 2);
        if let ContentBlock::Text { text } = &msgs[0].content[0] {
            assert_eq!(text, "first");
        }
    }

    #[test]
    fn pop_last_turn_empty() {
        let mut msgs: Vec<Message> = vec![];
        pop_last_turn(&mut msgs);
        assert!(msgs.is_empty());
    }

    #[test]
    fn extract_last_assistant_text_finds_text() {
        let msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "the plan"),
        ];
        assert_eq!(
            extract_last_assistant_text(&msgs),
            Some("the plan".to_string())
        );
    }

    #[test]
    fn extract_last_assistant_text_skips_user() {
        let msgs = vec![text_msg(Role::User, "hello")];
        assert_eq!(extract_last_assistant_text(&msgs), None);
    }

    #[test]
    fn is_tool_result_message_true() {
        let msg = tool_result_msg("t1", "output");
        assert!(is_tool_result_message(&msg));
    }

    #[test]
    fn is_tool_result_message_false_for_text() {
        let msg = text_msg(Role::User, "hello");
        assert!(!is_tool_result_message(&msg));
    }

    #[test]
    fn label_from_plan_uses_heading() {
        let plan = "# Fix auth bug\n\nSome details about the plan.";
        assert_eq!(label_from_plan(plan), Some("Fix auth bug".to_string()));
    }

    #[test]
    fn label_from_plan_uses_first_line_if_no_heading() {
        let plan = "Refactor the database layer\n\nMore details.";
        assert_eq!(
            label_from_plan(plan),
            Some("Refactor the database layer".to_string())
        );
    }

    #[test]
    fn label_from_plan_truncates() {
        let long_heading = format!("# {}", "a".repeat(100));
        let label = label_from_plan(&long_heading).unwrap();
        assert!(label.len() <= 60);
    }

    #[test]
    fn label_from_plan_empty_returns_none() {
        assert_eq!(label_from_plan(""), None);
        assert_eq!(label_from_plan("   \n  \n"), None);
    }
}
