//! Session data types.

use chet_types::{Message, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A persistent conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<Message>,
    pub total_usage: Usage,
    pub metadata: SessionMetadata,
    pub compaction_count: u32,
}

impl Session {
    /// Create a new empty session.
    pub fn new(model: String, cwd: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            total_usage: Usage::default(),
            metadata: SessionMetadata {
                model,
                cwd,
                label: None,
            },
            compaction_count: 0,
        }
    }

    /// Short hex prefix of the session ID for display.
    pub fn short_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    /// Generate a preview string from the first user message.
    pub fn preview(&self) -> String {
        for msg in &self.messages {
            if msg.role == chet_types::Role::User {
                for block in &msg.content {
                    if let chet_types::ContentBlock::Text { text } = block {
                        let trimmed = text.trim();
                        if trimmed.len() > 80 {
                            return format!("{}...", chet_types::truncate_str(trimmed, 77));
                        }
                        return trimmed.to_string();
                    }
                }
            }
        }
        String::new()
    }

    /// Auto-set the session label from the first user text message.
    /// No-op if a label is already set.
    pub fn auto_label(&mut self) {
        if self.metadata.label.is_some() {
            return;
        }
        for msg in &self.messages {
            if msg.role == chet_types::Role::User {
                for block in &msg.content {
                    if let chet_types::ContentBlock::Text { text } = block {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            let label = chet_types::truncate_str(trimmed, 60).to_string();
                            self.metadata.label = Some(label);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Build a summary for listing.
    pub fn to_summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            model: self.metadata.model.clone(),
            cwd: self.metadata.cwd.clone(),
            message_count: self.messages.len(),
            total_input_tokens: self.total_usage.input_tokens,
            total_output_tokens: self.total_usage.output_tokens,
            label: self.metadata.label.clone(),
            preview: self.preview(),
        }
    }
}

/// Session metadata stored alongside the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub model: String,
    pub cwd: String,
    pub label: Option<String>,
}

/// Lightweight summary for session listing.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub cwd: String,
    pub message_count: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub label: Option<String>,
    pub preview: String,
}

impl SessionSummary {
    /// Short hex prefix of the session ID for display.
    pub fn short_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    /// Human-readable age string (e.g. "2h ago", "3d ago").
    pub fn age(&self) -> String {
        let duration = Utc::now() - self.updated_at;
        let minutes = duration.num_minutes();
        if minutes < 1 {
            "just now".to_string()
        } else if minutes < 60 {
            format!("{minutes}m ago")
        } else if minutes < 1440 {
            format!("{}h ago", minutes / 60)
        } else {
            format!("{}d ago", minutes / 1440)
        }
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
    fn auto_label_sets_from_first_user_message() {
        let mut session = Session::new("test".into(), "/tmp".into());
        session
            .messages
            .push(text_msg(Role::User, "Fix the auth bug"));
        session.messages.push(text_msg(Role::Assistant, "OK"));
        session.auto_label();
        assert_eq!(session.metadata.label.as_deref(), Some("Fix the auth bug"));
    }

    #[test]
    fn auto_label_truncates_long_messages() {
        let mut session = Session::new("test".into(), "/tmp".into());
        let long_msg = "a".repeat(100);
        session.messages.push(text_msg(Role::User, &long_msg));
        session.auto_label();
        let label = session.metadata.label.as_deref().unwrap();
        assert!(label.len() <= 60);
    }

    #[test]
    fn auto_label_noop_if_already_set() {
        let mut session = Session::new("test".into(), "/tmp".into());
        session.metadata.label = Some("Existing label".into());
        session.messages.push(text_msg(Role::User, "New message"));
        session.auto_label();
        assert_eq!(session.metadata.label.as_deref(), Some("Existing label"));
    }

    #[test]
    fn auto_label_noop_if_no_user_messages() {
        let mut session = Session::new("test".into(), "/tmp".into());
        session.auto_label();
        assert!(session.metadata.label.is_none());
    }

    #[test]
    fn preview_truncates_with_unicode_safety() {
        let mut session = Session::new("test".into(), "/tmp".into());
        // 82 chars of emoji (each 4 bytes) - exceeds 80 char limit
        let emojis = "\u{1F600}".repeat(82);
        session.messages.push(text_msg(Role::User, &emojis));
        // Should not panic
        let preview = session.preview();
        assert!(preview.ends_with("..."));
    }
}
