//! Integration test for non-interactive pipe mode.
//!
//! Verifies that when TTY flags are false, the full agent event callback pipeline
//! produces zero ANSI escape sequences. Simulates the same event handling as
//! `run_agent()` in main.rs with captured `Vec<u8>` writers.
//!
//! Run with: `cargo test -p chet --test pipe_mode -- --ignored`

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chet_core::{Agent, AgentEvent};
use chet_permissions::PermissionEngine;
use chet_terminal::StreamingMarkdownRenderer;
use chet_tools::ToolRegistry;
use chet_types::{
    ContentBlock, ContentDelta, CreateMessageRequest, CreateMessageResponse, Message, MessageDelta,
    Role, StopReason, StreamEvent, ToolDefinition, ToolOutput, Usage,
    provider::{EventStream, Provider},
    tool::{Tool, ToolContext},
};
use futures_util::stream;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// SequencedMockProvider (duplicated from chet-core tests)
// ---------------------------------------------------------------------------

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
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, chet_types::ApiError>> + Send + 'a>> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        assert!(
            idx < self.sequences.len(),
            "SequencedMockProvider: call {idx} exceeds {} sequences",
            self.sequences.len()
        );
        let events = self.sequences[idx].clone();
        Box::pin(async move {
            let stream = stream::unfold(events.into_iter(), |mut iter| async move {
                let (event, delay_ms) = iter.next()?;
                if let Some(ms) = delay_ms {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
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
// EchoTool (duplicated from chet-core tests)
// ---------------------------------------------------------------------------

struct EchoTool {
    tool_name: String,
}

impl EchoTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
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
        true
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

// ---------------------------------------------------------------------------
// Helper: build an Agent
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
// Test
// ---------------------------------------------------------------------------

/// Runs an agent through a tool-use turn followed by a markdown text response,
/// capturing all output via the same event callback logic as `run_agent()` with
/// TTY=false. Asserts zero `\x1b` bytes in both stdout and stderr buffers.
#[tokio::test]
#[ignore]
async fn test_pipe_mode_no_ansi() {
    // Call 1: tool_use for EchoTool
    let call1_events = vec![
        (message_start_event(), None),
        (tool_use_block_start(0, "t1", "EchoTool"), None),
        (input_json_delta(0, r#"{"msg":"hello"}"#), None),
        (content_block_stop(0), None),
        (message_delta_tool_use(), None),
        (message_stop(), None),
    ];

    // Call 2: markdown text response with heading, code block, and bold
    let call2_events = vec![
        (message_start_event(), None),
        (text_block_start(0), None),
        (text_delta(0, "# Result\n"), None),
        (text_delta(0, "\n"), None),
        (text_delta(0, "The tool returned **success**.\n"), None),
        (text_delta(0, "\n"), None),
        (text_delta(0, "```rust\n"), None),
        (text_delta(0, "fn main() {}\n"), None),
        (text_delta(0, "```\n"), None),
        (content_block_stop(0), None),
        (message_delta_end_turn(), None),
        (message_stop(), None),
    ];

    let provider: Arc<dyn Provider> =
        Arc::new(SequencedMockProvider::new(vec![call1_events, call2_events]));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool::new("EchoTool")));

    let agent = make_agent(provider, registry);
    let cancel = CancellationToken::new();

    // Captured output buffers
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    // Plain-mode markdown renderer writing to our captured stdout buffer
    let mut renderer = {
        let buf = Arc::clone(&stdout_buf);
        StreamingMarkdownRenderer::new_plain(Box::new(SharedWriter(buf)))
    };

    let stderr_is_tty = false;

    // Build event callback mirroring run_agent() with stdout_is_tty=false, stderr_is_tty=false
    let stderr_cb = Arc::clone(&stderr_buf);
    let mut first_text = true;

    let callback = move |event: AgentEvent| {
        match event {
            AgentEvent::TextDelta(text) => {
                if first_text {
                    chet_terminal::spinner::clear_line(stderr_is_tty);
                    first_text = false;
                }
                renderer.push(&text);
            }
            AgentEvent::ThinkingDelta(text) => {
                if first_text {
                    chet_terminal::spinner::clear_line(stderr_is_tty);
                    first_text = false;
                }
                // Non-TTY: no ANSI wrapping
                let mut buf = stderr_cb.lock().unwrap();
                let _ = write!(buf, "{text}");
            }
            AgentEvent::ToolStart { name, .. } => {
                chet_terminal::spinner::clear_line(stderr_is_tty);
                renderer.finish();
                let mut buf = stderr_cb.lock().unwrap();
                let _ = writeln!(
                    buf,
                    "{}",
                    chet_terminal::style::tool_start(&name, stderr_is_tty)
                );
                first_text = true;
            }
            AgentEvent::ToolEnd {
                name,
                output,
                is_error,
            } => {
                chet_terminal::spinner::clear_line(stderr_is_tty);
                let mut buf = stderr_cb.lock().unwrap();
                if is_error {
                    let _ = writeln!(
                        buf,
                        "{}",
                        chet_terminal::style::tool_error(&name, &output, stderr_is_tty)
                    );
                } else {
                    let _ = writeln!(
                        buf,
                        "{}",
                        chet_terminal::style::tool_success(&name, &output, stderr_is_tty)
                    );
                }
                first_text = true;
            }
            AgentEvent::ToolBlocked { name, reason } => {
                chet_terminal::spinner::clear_line(stderr_is_tty);
                let mut buf = stderr_cb.lock().unwrap();
                let _ = writeln!(
                    buf,
                    "{}",
                    chet_terminal::style::tool_blocked(&name, &reason, stderr_is_tty)
                );
            }
            AgentEvent::Done => {
                chet_terminal::spinner::clear_line(stderr_is_tty);
                renderer.finish();
            }
            _ => {}
        }
    };

    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Use EchoTool then explain".to_string(),
        }],
    }];

    let result = agent.run(&mut messages, cancel, callback).await;
    assert!(result.is_ok(), "agent should complete: {:?}", result);

    // Collect output
    let stdout_output = {
        let buf = stdout_buf.lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    };
    let stderr_output = {
        let buf = stderr_buf.lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    };

    // --- Core assertion: zero ANSI escapes ---
    assert!(
        !stdout_output.contains('\x1b'),
        "stdout must not contain ANSI escapes, got: {stdout_output:?}"
    );
    assert!(
        !stderr_output.contains('\x1b'),
        "stderr must not contain ANSI escapes, got: {stderr_output:?}"
    );

    // --- Stdout contains raw markdown (plain passthrough) ---
    assert!(
        stdout_output.contains("# Result"),
        "stdout should contain heading: {stdout_output:?}"
    );
    assert!(
        stdout_output.contains("**success**"),
        "stdout should contain bold markdown: {stdout_output:?}"
    );
    assert!(
        stdout_output.contains("fn main()"),
        "stdout should contain code: {stdout_output:?}"
    );

    // --- Stderr contains plain tool events ---
    assert!(
        stderr_output.contains("> EchoTool"),
        "stderr should contain tool start: {stderr_output:?}"
    );
    assert!(
        stderr_output.contains("OK EchoTool"),
        "stderr should contain tool success: {stderr_output:?}"
    );
}

// ---------------------------------------------------------------------------
// SharedWriter — wraps Arc<Mutex<Vec<u8>>> for use as io::Write
// ---------------------------------------------------------------------------

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
