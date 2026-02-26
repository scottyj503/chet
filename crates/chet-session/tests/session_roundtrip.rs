//! Integration test for session round-trip persistence.
//!
//! Verifies that a realistic multi-turn conversation with diverse content blocks
//! (Text, ToolUse, ToolResult), usage tracking, metadata (label, model, cwd),
//! and compaction count all survive save → load faithfully.
//!
//! Run with: `cargo test -p chet-session --test session_roundtrip -- --ignored`

use chet_session::{Session, SessionStore};
use chet_types::{ContentBlock, Message, Role, ToolResultContent, Usage};
use tempfile::TempDir;

/// Build a realistic multi-turn session simulating a tool-use conversation.
fn build_test_session() -> Session {
    let mut session = Session::new(
        "claude-sonnet-4-5-20250929".into(),
        "/home/user/project".into(),
    );

    // Turn 1: user asks a question
    session.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "What files are in the src directory?".to_string(),
        }],
    });

    // Turn 1: assistant calls Glob tool
    session.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "tool_001".to_string(),
            name: "Glob".to_string(),
            input: serde_json::json!({"pattern": "src/**/*.rs"}),
        }],
    });

    // Turn 1: tool result
    session.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_001".to_string(),
            content: vec![ToolResultContent::Text {
                text: "src/main.rs\nsrc/lib.rs\nsrc/util.rs".to_string(),
            }],
            is_error: None,
        }],
    });

    // Turn 1: assistant final text response
    session.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "The src directory contains 3 files: main.rs, lib.rs, and util.rs.".to_string(),
        }],
    });

    // Turn 2: user asks to read a file
    session.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Read src/main.rs".to_string(),
        }],
    });

    // Turn 2: assistant calls Read tool
    session.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "tool_002".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
        }],
    });

    // Turn 2: tool result
    session.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_002".to_string(),
            content: vec![ToolResultContent::Text {
                text: "fn main() {\n    println!(\"Hello, world!\");\n}".to_string(),
            }],
            is_error: None,
        }],
    });

    // Turn 2: assistant final response
    session.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "Here's the contents of src/main.rs — a simple Hello World program.".to_string(),
        }],
    });

    // Set usage
    session.total_usage = Usage {
        input_tokens: 1500,
        output_tokens: 350,
        cache_creation_input_tokens: 100,
        cache_read_input_tokens: 50,
    };

    // Set label and compaction count
    session.metadata.label = Some("Explore src directory".to_string());
    session.compaction_count = 1;

    session
}

/// Save a realistic session and load it back, verifying all fields are intact.
#[tokio::test]
#[ignore]
async fn test_session_roundtrip_complex() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf()).await.unwrap();

    let original = build_test_session();
    let id = original.id;

    store.save(&original).await.unwrap();
    let loaded = store.load(id).await.unwrap();

    // Identity
    assert_eq!(loaded.id, original.id);
    assert_eq!(loaded.created_at, original.created_at);
    assert_eq!(loaded.updated_at, original.updated_at);

    // Metadata
    assert_eq!(loaded.metadata.model, "claude-sonnet-4-5-20250929");
    assert_eq!(loaded.metadata.cwd, "/home/user/project");
    assert_eq!(
        loaded.metadata.label.as_deref(),
        Some("Explore src directory")
    );

    // Compaction count
    assert_eq!(loaded.compaction_count, 1);

    // Usage
    assert_eq!(loaded.total_usage.input_tokens, 1500);
    assert_eq!(loaded.total_usage.output_tokens, 350);
    assert_eq!(loaded.total_usage.cache_creation_input_tokens, 100);
    assert_eq!(loaded.total_usage.cache_read_input_tokens, 50);

    // Message count
    assert_eq!(loaded.messages.len(), 8, "should have 8 messages");

    // Message 0: user text
    assert_eq!(loaded.messages[0].role, Role::User);
    assert!(matches!(
        &loaded.messages[0].content[0],
        ContentBlock::Text { text } if text.contains("What files")
    ));

    // Message 1: assistant ToolUse
    assert_eq!(loaded.messages[1].role, Role::Assistant);
    match &loaded.messages[1].content[0] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "tool_001");
            assert_eq!(name, "Glob");
            assert_eq!(input["pattern"], "src/**/*.rs");
        }
        other => panic!("expected ToolUse, got: {:?}", other),
    }

    // Message 2: ToolResult
    assert_eq!(loaded.messages[2].role, Role::User);
    match &loaded.messages[2].content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "tool_001");
            assert!(is_error.is_none());
            match &content[0] {
                ToolResultContent::Text { text } => {
                    assert!(text.contains("main.rs"));
                    assert!(text.contains("lib.rs"));
                }
                other => panic!("expected Text content, got: {:?}", other),
            }
        }
        other => panic!("expected ToolResult, got: {:?}", other),
    }

    // Message 3: assistant text
    assert_eq!(loaded.messages[3].role, Role::Assistant);
    assert!(matches!(
        &loaded.messages[3].content[0],
        ContentBlock::Text { text } if text.contains("3 files")
    ));

    // Message 5: second ToolUse (Read)
    match &loaded.messages[5].content[0] {
        ContentBlock::ToolUse { name, input, .. } => {
            assert_eq!(name, "Read");
            assert_eq!(input["file_path"], "src/main.rs");
        }
        other => panic!("expected ToolUse, got: {:?}", other),
    }

    // Message 7: final assistant text
    assert!(matches!(
        &loaded.messages[7].content[0],
        ContentBlock::Text { text } if text.contains("Hello World")
    ));
}

/// Verify that the session summary (from list) reflects the stored data.
#[tokio::test]
#[ignore]
async fn test_session_list_after_save() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf()).await.unwrap();

    let session = build_test_session();
    let id = session.id;
    store.save(&session).await.unwrap();

    let summaries = store.list().await.unwrap();
    assert_eq!(summaries.len(), 1);

    let summary = &summaries[0];
    assert_eq!(summary.id, id);
    assert_eq!(summary.model, "claude-sonnet-4-5-20250929");
    assert_eq!(summary.cwd, "/home/user/project");
    assert_eq!(summary.message_count, 8);
    assert_eq!(summary.total_input_tokens, 1500);
    assert_eq!(summary.total_output_tokens, 350);
    assert_eq!(summary.label.as_deref(), Some("Explore src directory"));
    assert!(
        summary.preview.contains("What files"),
        "preview should come from first user message: {:?}",
        summary.preview
    );
}

/// Save, modify, save again — verify the update persists.
#[tokio::test]
#[ignore]
async fn test_session_update_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf()).await.unwrap();

    let mut session = build_test_session();
    let id = session.id;
    store.save(&session).await.unwrap();

    // Add another turn
    session.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Thanks!".to_string(),
        }],
    });
    session.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "You're welcome!".to_string(),
        }],
    });
    session.total_usage.input_tokens += 200;
    session.total_usage.output_tokens += 50;

    store.save(&session).await.unwrap();
    let loaded = store.load(id).await.unwrap();

    assert_eq!(
        loaded.messages.len(),
        10,
        "should have 10 messages after update"
    );
    assert_eq!(loaded.total_usage.input_tokens, 1700);
    assert_eq!(loaded.total_usage.output_tokens, 400);

    // Verify only one file exists (overwrite, not duplicate)
    let summaries = store.list().await.unwrap();
    assert_eq!(summaries.len(), 1);
}
