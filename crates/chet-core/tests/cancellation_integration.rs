//! End-to-end integration tests for `Agent::run()`.
//!
//! These tests exercise:
//! 1. Cancellation — both `tokio::select!` cancellation points in the agent loop
//! 2. Multi-tool-use turns — multiple tool_use blocks in a single response
//!
//! Run with: `cargo test -p chet-core --test cancellation_integration -- --ignored`

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
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
// SequencedMockProvider
// ---------------------------------------------------------------------------

/// A test provider that returns different event lists on successive calls.
/// Each `create_message_stream()` call consumes the next entry from `sequences`.
/// Panics if called more times than there are sequences (indicates a test bug).
struct SequencedMockProvider {
    sequences: Vec<Vec<(StreamEvent, Option<u64>)>>,
    call_count: AtomicUsize,
}

impl SequencedMockProvider {
    fn new(sequences: Vec<Vec<(StreamEvent, Option<u64>)>>) -> Self {
        Self {
            sequences,
            call_count: AtomicUsize::new(0),
        }
    }
}

impl Provider for SequencedMockProvider {
    fn create_message_stream<'a>(
        &'a self,
        _request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        assert!(
            idx < self.sequences.len(),
            "SequencedMockProvider: call {idx} exceeds {} sequences (test bug)",
            self.sequences.len()
        );
        let events = self.sequences[idx].clone();
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
        "sequenced-mock"
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
// EchoTool
// ---------------------------------------------------------------------------

/// A generic test tool that echoes its input JSON as text output.
/// The name and read-only status are configurable.
struct EchoTool {
    tool_name: String,
    read_only: bool,
}

impl EchoTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            read_only: true,
        }
    }

    fn new_writable(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            read_only: false,
        }
    }
}

impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_name.clone(),
            description: format!("Echo tool named {}", self.tool_name),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "msg": { "type": "string" }
                }
            }),
            cache_control: None,
        }
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, chet_types::ToolError>> + Send + '_>> {
        Box::pin(async move { Ok(ToolOutput::text(input.to_string())) })
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
    tool_ends: Vec<(String, String, bool)>,
    tool_blocked: Vec<(String, String)>,
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
                AgentEvent::ToolEnd {
                    name,
                    output,
                    is_error,
                } => c.tool_ends.push((name, output, is_error)),
                AgentEvent::ToolBlocked { name, reason } => c.tool_blocked.push((name, reason)),
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

fn message_stop() -> StreamEvent {
    StreamEvent::MessageStop
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

/// MockProvider returns 2 tool_use blocks in one response. Both tools execute,
/// results are sent back, and the agent completes with a final text response.
/// Validates the common real-world pattern of parallel tool calls.
#[tokio::test]
#[ignore]
async fn test_multi_tool_use_turn() {
    // Call 1: two tool_use blocks in one response
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

    // Call 2: final text response after tool results
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

    // Agent should complete successfully
    assert!(result.is_ok(), "should complete successfully: {:?}", result);

    let c = capture.lock().unwrap();
    assert!(c.saw_done, "should have seen Done event");
    assert!(!c.saw_cancelled, "should NOT have seen Cancelled event");

    // Both tools should have started and ended
    assert!(
        c.tool_starts.contains(&"EchoA".to_string()),
        "should have started EchoA"
    );
    assert!(
        c.tool_starts.contains(&"EchoB".to_string()),
        "should have started EchoB"
    );
    assert_eq!(c.tool_ends.len(), 2, "both tools should have ended");
    assert!(
        !c.tool_ends[0].2,
        "EchoA should not be an error: {:?}",
        c.tool_ends[0]
    );
    assert!(
        !c.tool_ends[1].2,
        "EchoB should not be an error: {:?}",
        c.tool_ends[1]
    );

    // Drop lock before inspecting messages
    drop(c);

    // Message structure: [user, assistant(2 tool_use), user(2 tool_result), assistant(text)]
    assert_eq!(
        messages.len(),
        4,
        "expected 4 messages, got {}: {:?}",
        messages.len(),
        messages.iter().map(|m| &m.role).collect::<Vec<_>>()
    );

    // Message 0: original user message
    assert_eq!(messages[0].role, Role::User);

    // Message 1: assistant with 2 ToolUse content blocks
    assert_eq!(messages[1].role, Role::Assistant);
    let tool_use_blocks: Vec<_> = messages[1]
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .collect();
    assert_eq!(
        tool_use_blocks.len(),
        2,
        "assistant message should have 2 ToolUse blocks"
    );

    // Message 2: user with 2 ToolResult content blocks
    assert_eq!(messages[2].role, Role::User);
    let tool_result_blocks: Vec<_> = messages[2]
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .collect();
    assert_eq!(
        tool_result_blocks.len(),
        2,
        "user message should have 2 ToolResult blocks"
    );

    // Message 3: final assistant text
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

/// Agent in read-only mode receives a tool_use for a non-read-only tool.
/// The tool should be blocked (not executed), ToolBlocked event should fire,
/// and the agent should continue to produce the final text response.
#[tokio::test]
#[ignore]
async fn test_plan_mode_tool_blocking() {
    // Call 1: API requests a non-read-only tool ("WriteTool")
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "WriteTool"), None),
        (input_json_delta(0, r#"{"msg":"should be blocked"}"#), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    // Call 2: after receiving the blocked tool result, API responds with text
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

    // Agent should complete successfully (blocking a tool is not a fatal error)
    assert!(result.is_ok(), "should complete successfully: {:?}", result);

    let c = capture.lock().unwrap();
    assert!(c.saw_done, "should have seen Done event");
    assert!(!c.saw_cancelled, "should NOT have seen Cancelled event");

    // Tool should have been blocked (not executed)
    assert_eq!(
        c.tool_blocked.len(),
        1,
        "exactly one tool should be blocked"
    );
    assert_eq!(c.tool_blocked[0].0, "WriteTool");
    assert!(
        c.tool_blocked[0].1.contains("read-only"),
        "reason should mention read-only: {:?}",
        c.tool_blocked[0].1
    );
    // ToolStart fires during streaming (before the read-only check), but ToolEnd should not
    assert_eq!(c.tool_starts.len(), 1, "ToolStart fires during streaming");
    assert_eq!(c.tool_starts[0], "WriteTool");
    assert!(c.tool_ends.is_empty(), "tool should NOT have executed");

    // Drop lock before inspecting messages
    drop(c);

    // Message structure: [user, assistant(1 tool_use), user(1 tool_result with is_error), assistant(text)]
    assert_eq!(
        messages.len(),
        4,
        "expected 4 messages, got {}: {:?}",
        messages.len(),
        messages.iter().map(|m| &m.role).collect::<Vec<_>>()
    );

    // Message 2: tool result should be an error
    assert_eq!(messages[2].role, Role::User);
    let tool_result = messages[2]
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolResult { .. }));
    assert!(tool_result.is_some(), "should have a ToolResult block");
    if let Some(ContentBlock::ToolResult { is_error, .. }) = tool_result {
        assert_eq!(*is_error, Some(true), "tool result should be an error");
    }

    // Message 3: final assistant text
    assert_eq!(messages[3].role, Role::Assistant);
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
