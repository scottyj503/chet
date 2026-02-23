//! End-to-end cancellation tests for `Agent::run()`.
//!
//! These tests exercise both `tokio::select!` cancellation points in the agent loop:
//! 1. Mid-stream: cancel arrives while SSE events are being consumed
//! 2. Mid-tool: cancel arrives while a tool is executing
//!
//! Run with: `cargo test -p chet-core --test cancellation_integration -- --ignored`

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chet_core::{Agent, AgentEvent};
use chet_permissions::PermissionEngine;
use chet_tools::ToolRegistry;
use chet_types::{
    ApiError, ChetError, ContentBlock, ContentDelta, CreateMessageRequest, CreateMessageResponse,
    Message, MessageDelta, Role, StopReason, StreamEvent, ToolDefinition, ToolOutput, Usage,
    provider::{EventStream, Provider},
    tool::{Tool, ToolContext},
};
use futures_util::stream;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// MockProvider
// ---------------------------------------------------------------------------

/// A test provider that yields pre-configured events with optional delays.
struct MockProvider {
    events: Vec<(StreamEvent, Option<u64>)>,
}

impl MockProvider {
    fn new(events: Vec<(StreamEvent, Option<u64>)>) -> Self {
        Self { events }
    }
}

impl Provider for MockProvider {
    fn create_message_stream<'a>(
        &'a self,
        _request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>> {
        let events = self.events.clone();
        Box::pin(async move {
            let stream = stream::unfold(events.into_iter(), |mut iter| async move {
                let (event, delay_ms) = iter.next()?;
                if let Some(ms) = delay_ms {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
                Some((Ok(event), iter))
            });
            Ok(Box::pin(stream) as EventStream)
        })
    }

    fn name(&self) -> &str {
        "mock"
    }
}

// ---------------------------------------------------------------------------
// SlowTool
// ---------------------------------------------------------------------------

/// A tool that sleeps for a configurable duration. Used to test mid-tool cancellation.
struct SlowTool;

impl Tool for SlowTool {
    fn name(&self) -> &str {
        "SlowTool"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "SlowTool".to_string(),
            description: "Sleeps for sleep_ms milliseconds".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "sleep_ms": { "type": "integer" }
                }
            }),
            cache_control: None,
        }
    }

    fn is_read_only(&self) -> bool {
        true // auto-permitted, no permission prompt needed
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, chet_types::ToolError>> + Send + '_>> {
        Box::pin(async move {
            let sleep_ms = input
                .get("sleep_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(5000);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            Ok(ToolOutput::text("done"))
        })
    }
}

// ---------------------------------------------------------------------------
// EventCapture
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct EventCapture {
    saw_cancelled: bool,
    saw_done: bool,
    text_deltas: Vec<String>,
    tool_starts: Vec<String>,
}

impl EventCapture {
    fn callback(capture: Arc<Mutex<Self>>) -> impl FnMut(AgentEvent) {
        move |event| {
            let mut c = capture.lock().unwrap();
            match event {
                AgentEvent::Cancelled => c.saw_cancelled = true,
                AgentEvent::Done => c.saw_done = true,
                AgentEvent::TextDelta(t) => c.text_deltas.push(t),
                AgentEvent::ToolStart { name, .. } => c.tool_starts.push(name),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event builder helpers
// ---------------------------------------------------------------------------

fn message_start_event() -> StreamEvent {
    StreamEvent::MessageStart {
        message: CreateMessageResponse {
            id: "msg_test".to_string(),
            response_type: "message".to_string(),
            role: Role::Assistant,
            content: vec![],
            model: "test-model".to_string(),
            stop_reason: None,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
        },
    }
}

fn text_block_start(index: usize) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::Text {
            text: String::new(),
        },
    }
}

fn text_delta(index: usize, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::TextDelta {
            text: text.to_string(),
        },
    }
}

fn content_block_stop(index: usize) -> StreamEvent {
    StreamEvent::ContentBlockStop { index }
}

fn tool_use_block_start(index: usize, id: &str, name: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::Value::Null,
        },
    }
}

fn input_json_delta(index: usize, json: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::InputJsonDelta {
            partial_json: json.to_string(),
        },
    }
}

fn message_delta_end_turn() -> StreamEvent {
    StreamEvent::MessageDelta {
        delta: MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
        },
        usage: Some(Usage {
            input_tokens: 0,
            output_tokens: 5,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }),
    }
}

fn message_delta_tool_use() -> StreamEvent {
    StreamEvent::MessageDelta {
        delta: MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
        },
        usage: Some(Usage {
            input_tokens: 0,
            output_tokens: 5,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }),
    }
}

// ---------------------------------------------------------------------------
// Helper: build an Agent with a MockProvider
// ---------------------------------------------------------------------------

fn make_agent(provider: Arc<dyn Provider>, registry: ToolRegistry) -> Agent {
    let permissions = Arc::new(PermissionEngine::ludicrous());
    Agent::new(
        provider,
        registry,
        permissions,
        "test-model".to_string(),
        1024,
        PathBuf::from("/tmp"),
    )
}

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
    // Stream a fast tool_use block, then message_delta with stop_reason=tool_use
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

/// Token is already cancelled before the agent starts.
/// Expects: Err(Cancelled), saw_cancelled, no deltas received.
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
    assert!(c.saw_cancelled, "should have seen Cancelled event");
    assert!(
        c.text_deltas.is_empty(),
        "should NOT have received any deltas"
    );
    assert!(!c.saw_done, "should NOT have seen Done event");
}
