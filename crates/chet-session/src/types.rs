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
                            return format!("{}...", &trimmed[..77]);
                        }
                        return trimmed.to_string();
                    }
                }
            }
        }
        String::new()
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
