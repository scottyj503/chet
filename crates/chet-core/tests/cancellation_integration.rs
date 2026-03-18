//! End-to-end integration tests for `Agent::run()`.
//!
//! These tests exercise:
//! 1. Cancellation — both `tokio::select!` cancellation points in the agent loop
//! 2. Multi-tool-use turns — multiple tool_use blocks in a single response
//! 3. Plan mode tool blocking — read-only safety net
//! 4. Subagent end-to-end — parent spawns child via SubagentTool
//!
//! Run with: `cargo test -p chet-core --test cancellation_integration -- --ignored`

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chet_core::{Agent, SubagentTool};
use chet_permissions::PermissionEngine;
use chet_session::compact;
use chet_tools::ToolRegistry;
use chet_types::{ChetError, ContentBlock, Message, Role, StreamEvent, provider::Provider};
use tokio_util::sync::CancellationToken;

use common::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Cancel arrives while the agent is streaming text deltas from the provider.
/// Expects: Err(Cancelled), saw_cancelled, at least 1 delta, no Done event.
#[tokio::test]
#[ignore]
async fn test_cancel_mid_stream() {
    let events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Hello "), Some(50)),
        (text_delta(0, "world "), Some(200)),
        (text_delta(0, "this "), Some(200)),
        (text_delta(0, "should "), Some(200)),
        (text_delta(0, "not "), Some(200)),
        (text_delta(0, "arrive"), Some(200)),
        (content_block_stop(0), Some(200)),
        (message_delta_end_turn(), None),
        (StreamEvent::MessageStop, None),
    ];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(events));
    let agent = make_agent(provider, ToolRegistry::new());

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Cancel after 150ms — should get the first delta, maybe second, but not all
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel_clone.cancel();
    });

    let capture = Arc::new(Mutex::new(EventCapture::default()));
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Hi".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(matches!(result, Err(ChetError::Cancelled)));
    let c = capture.lock().unwrap();
    assert!(c.saw_cancelled, "should have seen Cancelled event");
    assert!(
        !c.text_deltas.is_empty(),
        "should have received at least 1 delta"
    );
    assert!(!c.saw_done, "should NOT have seen Done event");
}

/// Cancel arrives while a tool (SlowTool) is executing its long sleep.
/// Expects: Err(Cancelled), saw_cancelled, tool_starts contains "SlowTool",
/// assistant message popped (messages.len() == 1, only the original user msg).
#[tokio::test]
#[ignore]
async fn test_cancel_mid_tool() {
    let events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "tool_001", "SlowTool"), None),
        (input_json_delta(0, r#"{"sleep_ms": 5000}"#), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (StreamEvent::MessageStop, None),
    ];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(events));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SlowTool));
    let agent = make_agent(provider, registry);

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Cancel 100ms after stream finishes — tool will be mid-sleep
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_clone.cancel();
    });

    let capture = Arc::new(Mutex::new(EventCapture::default()));
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Run slow tool".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(matches!(result, Err(ChetError::Cancelled)));
    let c = capture.lock().unwrap();
    assert!(c.saw_cancelled, "should have seen Cancelled event");
    assert!(
        c.tool_starts.contains(&"SlowTool".to_string()),
        "should have started SlowTool"
    );
    // The assistant message should have been popped on cancel
    assert_eq!(
        messages.len(),
        1,
        "assistant message should be popped; only user message remains"
    );
    assert_eq!(messages[0].role, Role::User);
}

/// Cancel fires after the agent has already completed normally.
/// The token is cancelled 500ms after the fast response finishes.
/// Expects: Ok(usage), saw_done, NOT saw_cancelled.
#[tokio::test]
#[ignore]
async fn test_cancel_after_completion() {
    let events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Done!"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (StreamEvent::MessageStop, None),
    ];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(events));
    let agent = make_agent(provider, ToolRegistry::new());

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Cancel well after completion
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel_clone.cancel();
    });

    let capture = Arc::new(Mutex::new(EventCapture::default()));
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Quick question".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok(), "should complete successfully: {:?}", result);
    let c = capture.lock().unwrap();
    assert!(c.saw_done, "should have seen Done event");
    assert!(!c.saw_cancelled, "should NOT have seen Cancelled event");
}

/// MockProvider returns 2 tool_use blocks in one response. Both tools execute,
/// results are sent back, and the agent completes with a final text response.
#[tokio::test]
#[ignore]
async fn test_multi_tool_use_turn() {
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "EchoA"), None),
        (input_json_delta(0, r#"{"msg":"alpha"}"#), None),
        (content_block_stop(0), None),
        (tool_use_block_start(1, "t2", "EchoB"), None),
        (input_json_delta(1, r#"{"msg":"beta"}"#), None),
        (content_block_stop(1), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Both tools done"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new("EchoA")));
    registry.register(Arc::new(EchoTool::new("EchoB")));

    let agent = make_agent(provider, registry);
    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Use both tools".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok(), "should complete successfully: {:?}", result);

    let c = capture.lock().unwrap();
    assert!(c.saw_done, "should have seen Done event");
    assert!(!c.saw_cancelled, "should NOT have seen Cancelled event");
    assert!(c.tool_starts.contains(&"EchoA".to_string()));
    assert!(c.tool_starts.contains(&"EchoB".to_string()));
    assert_eq!(c.tool_ends.len(), 2, "both tools should have ended");
    assert!(!c.tool_ends[0].2, "EchoA should not be an error");
    assert!(!c.tool_ends[1].2, "EchoB should not be an error");
    drop(c);

    // Message structure: [user, assistant(2 tool_use), user(2 tool_result), assistant(text)]
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[1].role, Role::Assistant);
    let tool_use_count = messages[1]
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .count();
    assert_eq!(tool_use_count, 2);

    assert_eq!(messages[2].role, Role::User);
    let tool_result_count = messages[2]
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .count();
    assert_eq!(tool_result_count, 2);

    assert_eq!(messages[3].role, Role::Assistant);
    let final_text: String = messages[3]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(final_text, "Both tools done");
}

/// Agent in read-only mode blocks non-read-only tools.
#[tokio::test]
#[ignore]
async fn test_plan_mode_tool_blocking() {
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "WriteTool"), None),
        (input_json_delta(0, r#"{"msg":"should be blocked"}"#), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Understood, tool was blocked"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new_writable("WriteTool")));

    let mut agent = make_agent(provider, registry);
    agent.set_read_only_mode(true);

    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Write something".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok(), "should complete successfully: {:?}", result);

    let c = capture.lock().unwrap();
    assert!(c.saw_done);
    assert_eq!(c.tool_blocked.len(), 1);
    assert_eq!(c.tool_blocked[0].0, "WriteTool");
    assert!(c.tool_blocked[0].1.contains("read-only"));
    assert_eq!(c.tool_starts.len(), 1);
    assert!(c.tool_ends.is_empty(), "tool should NOT have executed");
    drop(c);

    assert_eq!(messages.len(), 4);
    if let Some(ContentBlock::ToolResult { is_error, .. }) = messages[2]
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolResult { .. }))
    {
        assert_eq!(*is_error, Some(true));
    }

    let final_text: String = messages[3]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(final_text, "Understood, tool was blocked");
}

/// Parent agent calls SubagentTool, child returns text, parent produces final response.
#[tokio::test]
#[ignore]
async fn test_subagent_end_to_end() {
    let subagent_input = r#"{"prompt":"What is 2+2?","description":"math question"}"#;

    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "Subagent"), None),
        (input_json_delta(0, subagent_input), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "The answer is 4"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let call3_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Subagent says: 4"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> = Arc::new(SequencedMockProvider::new(vec![
        call1_events,
        call2_events,
        call3_events,
    ]));

    let permissions = Arc::new(PermissionEngine::ludicrous());
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SubagentTool::new(
        Arc::clone(&provider),
        Arc::clone(&permissions),
        "test-model".to_string(),
        1024,
        PathBuf::from("/tmp"),
    )));

    let agent = Agent::new(
        Arc::clone(&provider),
        registry,
        permissions,
        "test-model".to_string(),
        1024,
        PathBuf::from("/tmp"),
    );

    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Ask a subagent what 2+2 is".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok(), "should complete successfully: {:?}", result);

    let c = capture.lock().unwrap();
    assert!(c.saw_done);
    assert!(c.tool_starts.contains(&"Subagent".to_string()));
    assert_eq!(c.tool_ends.len(), 1);
    assert!(c.tool_ends[0].1.contains("The answer is 4"));
    drop(c);

    assert_eq!(messages.len(), 4);
    let tool_result_text = messages[2]
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.iter().find_map(|c| match c {
                chet_types::ToolResultContent::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or("");
    assert!(tool_result_text.contains("The answer is 4"));

    let final_text: String = messages[3]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(final_text, "Subagent says: 4");
}

/// Token is already cancelled before the agent starts.
#[tokio::test]
#[ignore]
async fn test_cancel_before_start() {
    let events = vec![
        (message_start_event(), Some(200)),
        (text_block_start(0), Some(200)),
        (text_delta(0, "Never"), Some(200)),
        (content_block_stop(0), Some(200)),
        (message_delta_end_turn(), None),
        (StreamEvent::MessageStop, None),
    ];

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(events));
    let agent = make_agent(provider, ToolRegistry::new());

    let cancel = CancellationToken::new();
    cancel.cancel(); // Already cancelled

    let capture = Arc::new(Mutex::new(EventCapture::default()));
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Hi".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(matches!(result, Err(ChetError::Cancelled)));
    let c = capture.lock().unwrap();
    assert!(c.saw_cancelled);
    assert!(c.text_deltas.is_empty());
    assert!(!c.saw_done);
}

/// Compact a long conversation with a label, then run agent in read-only mode.
#[tokio::test]
#[ignore]
async fn test_compaction_preserves_label_and_plan_mode() {
    let mut conversation: Vec<Message> = Vec::new();
    for i in 0..7 {
        conversation.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("Question {i}"),
            }],
        });
        conversation.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: format!("Answer {i}"),
            }],
        });
    }
    assert!(conversation.len() >= 12);

    let label = "Fix auth bug";
    let result = compact(&conversation, Some(label));
    assert!(result.is_some());
    let compacted = result.unwrap();

    // Label survives in summary
    let summary_text = match &compacted.new_messages[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text in summary"),
    };
    assert!(summary_text.contains("[Session: Fix auth bug]"));
    assert!(summary_text.contains("Compacted"));

    // Plan mode blocks tools after compaction
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "WriteTool"), None),
        (input_json_delta(0, r#"{"msg":"should be blocked"}"#), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];
    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "OK, tool was blocked"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new_writable("WriteTool")));

    let mut agent = make_agent(provider, registry);
    agent.set_read_only_mode(true);

    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = compacted.new_messages;
    messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Write something for me".to_string(),
        }],
    });

    let run_result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(run_result.is_ok());
    let c = capture.lock().unwrap();
    assert!(c.saw_done);
    assert_eq!(c.tool_blocked.len(), 1);
    assert!(c.tool_blocked[0].1.contains("read-only"));
    assert!(c.tool_ends.is_empty());
    drop(c);

    let first_msg_text = match &messages[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text in first message"),
    };
    assert!(first_msg_text.contains("[Session: Fix auth bug]"));
}

/// Read-only tool failure doesn't cancel sibling read-only tools.
#[tokio::test]
#[ignore]
async fn test_parallel_read_only_failure_isolation() {
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "EchoA"), None),
        (input_json_delta(0, r#"{"msg":"hello"}"#), None),
        (content_block_stop(0), None),
        (tool_use_block_start(1, "t2", "FailTool"), None),
        (input_json_delta(1, r#"{}"#), None),
        (content_block_stop(1), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Got results"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new("EchoA")));
    registry.register(Arc::new(FailingTool::new("FailTool")));

    let agent = make_agent(provider, registry);
    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Use both tools".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok());
    let c = capture.lock().unwrap();
    assert!(c.saw_done);
    assert_eq!(c.tool_ends.len(), 2);
    let error_count = c.tool_ends.iter().filter(|(_, _, err)| *err).count();
    assert_eq!(error_count, 1);
    drop(c);

    let tool_results: Vec<_> = messages[2]
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 2);
}

/// Mixed read-only + writable tools: read-only run in parallel, writable runs after.
#[tokio::test]
#[ignore]
async fn test_mixed_parallel_and_sequential_tools() {
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "EchoA"), None),
        (input_json_delta(0, r#"{"msg":"read"}"#), None),
        (content_block_stop(0), None),
        (tool_use_block_start(1, "t2", "WritableEcho"), None),
        (input_json_delta(1, r#"{"msg":"write"}"#), None),
        (content_block_stop(1), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "Both done"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new("EchoA")));
    registry.register(Arc::new(EchoTool::new_writable("WritableEcho")));

    let agent = make_agent(provider, registry);
    let cancel = CancellationToken::new();
    let capture = Arc::new(Mutex::new(EventCapture::default()));

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Use both tools".to_string(),
        }],
    }];

    let result = agent
        .run(
            &mut messages,
            cancel,
            EventCapture::callback(capture.clone()),
        )
        .await;

    assert!(result.is_ok());
    let c = capture.lock().unwrap();
    assert!(c.saw_done);
    assert_eq!(c.tool_ends.len(), 2);
    assert!(c.tool_ends.iter().all(|(_, _, err)| !*err));
    drop(c);

    assert_eq!(messages.len(), 4);
    let results = &messages[2].content;
    assert_eq!(results.len(), 2);
    match &results[0] {
        ContentBlock::ToolResult { tool_use_id, .. } => assert_eq!(tool_use_id, "t1"),
        other => panic!("expected ToolResult, got {other:?}"),
    }
    match &results[1] {
        ContentBlock::ToolResult { tool_use_id, .. } => assert_eq!(tool_use_id, "t2"),
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
