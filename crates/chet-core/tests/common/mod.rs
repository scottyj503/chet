//! Shared test infrastructure for chet-core integration tests.
//!
//! Provides mock providers, test tools, event capture, and event builder helpers.

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
    ApiError, ContentBlock, ContentDelta, CreateMessageRequest, CreateMessageResponse,
    MessageDelta, Role, StopReason, StreamEvent, ToolDefinition, ToolOutput, Usage,
    provider::{EventStream, Provider},
    tool::{Tool, ToolContext},
};
use futures_util::stream;

// ---------------------------------------------------------------------------
// MockProvider
// ---------------------------------------------------------------------------

/// A test provider that yields pre-configured events with optional delays.
pub struct MockProvider {
    pub events: Vec<(StreamEvent, Option<u64>)>,
}

impl MockProvider {
    pub fn new(events: Vec<(StreamEvent, Option<u64>)>) -> Self {
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
pub struct SequencedMockProvider {
    pub sequences: Vec<Vec<(StreamEvent, Option<u64>)>>,
    call_count: AtomicUsize,
}

impl SequencedMockProvider {
    pub fn new(sequences: Vec<Vec<(StreamEvent, Option<u64>)>>) -> Self {
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
pub struct SlowTool;

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
pub struct EchoTool {
    tool_name: String,
    read_only: bool,
}

impl EchoTool {
    pub fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            read_only: true,
        }
    }

    pub fn new_writable(name: &str) -> Self {
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
// FailingTool
// ---------------------------------------------------------------------------

/// A read-only tool that always fails with an error.
pub struct FailingTool {
    tool_name: String,
}

impl FailingTool {
    pub fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
        }
    }
}

impl Tool for FailingTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_name.clone(),
            description: "A tool that always fails".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            cache_control: None,
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, chet_types::ToolError>> + Send + '_>> {
        Box::pin(async move {
            Err(chet_types::ToolError::ExecutionFailed(
                "Simulated failure".to_string(),
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// EventCapture
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct EventCapture {
    pub saw_cancelled: bool,
    pub saw_done: bool,
    pub text_deltas: Vec<String>,
    pub tool_starts: Vec<String>,
    pub tool_ends: Vec<(String, String, bool)>,
    pub tool_blocked: Vec<(String, String)>,
}

impl EventCapture {
    pub fn callback(capture: Arc<Mutex<Self>>) -> impl FnMut(AgentEvent) {
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

pub fn message_start_event() -> StreamEvent {
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

pub fn text_block_start(index: usize) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::Text {
            text: String::new(),
        },
    }
}

pub fn text_delta(index: usize, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::TextDelta {
            text: text.to_string(),
        },
    }
}

pub fn content_block_stop(index: usize) -> StreamEvent {
    StreamEvent::ContentBlockStop { index }
}

pub fn tool_use_block_start(index: usize, id: &str, name: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::Value::Null,
        },
    }
}

pub fn input_json_delta(index: usize, json: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta {
        index,
        delta: ContentDelta::InputJsonDelta {
            partial_json: json.to_string(),
        },
    }
}

pub fn message_delta_end_turn() -> StreamEvent {
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

pub fn message_stop() -> StreamEvent {
    StreamEvent::MessageStop
}

pub fn message_delta_tool_use() -> StreamEvent {
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

pub fn make_agent(provider: Arc<dyn Provider>, registry: ToolRegistry) -> Agent {
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
