//! Integration tests for the full SSE → MessageStream → StreamEvent pipeline.
//!
//! These tests simulate realistic API responses by feeding complete SSE
//! sequences through MessageStream and verifying all events are correctly
//! parsed and yielded in order.
//!
//! Run with: `cargo test -p chet-api --test stream_integration -- --ignored`
//! Or all ignored tests: `cargo test --workspace -- --ignored`

use chet_api::MessageStream;
use chet_types::{ContentDelta, StreamEvent};
use futures_util::StreamExt;

/// Create a MessageStream from raw SSE text (simulating a complete API response).
fn stream_from_sse(sse_text: &str) -> MessageStream {
    let bytes = bytes::Bytes::from(sse_text.to_owned());
    let byte_stream = futures_util::stream::iter(vec![Ok::<_, reqwest::Error>(bytes)]);
    MessageStream::new(byte_stream)
}

/// Create a MessageStream from multiple byte chunks (simulating chunked transfer).
fn stream_from_chunks(chunks: Vec<&str>) -> MessageStream {
    let byte_stream = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|s| Ok::<_, reqwest::Error>(bytes::Bytes::from(s.to_owned())))
            .collect::<Vec<_>>(),
    );
    MessageStream::new(byte_stream)
}

/// Collect all events from a MessageStream.
async fn collect_events(mut stream: MessageStream) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        events.push(result.expect("stream event should parse successfully"));
    }
    events
}

// ---------------------------------------------------------------------------
// Test: simple text-only chat response
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_simple_chat_response() {
    let sse = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5-20250929\",\"stop_reason\":null,\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: ping\n\
data: {}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world!\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":10}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

    let events = collect_events(stream_from_sse(sse)).await;

    // Verify event sequence
    assert!(matches!(events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(events[1], StreamEvent::ContentBlockStart { .. }));
    assert!(matches!(events[2], StreamEvent::Ping));

    // Text deltas
    match &events[3] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::TextDelta { text },
            ..
        } => assert_eq!(text, "Hello"),
        other => panic!("Expected TextDelta, got {other:?}"),
    }
    match &events[4] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::TextDelta { text },
            ..
        } => assert_eq!(text, " world!"),
        other => panic!("Expected TextDelta, got {other:?}"),
    }

    assert!(matches!(events[5], StreamEvent::ContentBlockStop { .. }));
    assert!(matches!(events[6], StreamEvent::MessageDelta { .. }));
    assert!(matches!(events[7], StreamEvent::MessageStop));
    assert_eq!(events.len(), 8);
}

// ---------------------------------------------------------------------------
// Test: tool use response (model calls a tool)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_tool_use_response() {
    let sse = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5-20250929\",\"stop_reason\":null,\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"I'll read that file.\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_abc123\",\"name\":\"Read\",\"input\":{}}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\": \\\"/tmp/\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"test.txt\\\"}\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":50}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

    let events = collect_events(stream_from_sse(sse)).await;

    // Verify we get: message_start, text block, tool_use block, message_delta, message_stop
    assert!(matches!(events[0], StreamEvent::MessageStart { .. }));

    // Text block: start, delta, stop
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart { index: 0, .. }
    ));
    match &events[2] {
        StreamEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta { text },
        } => assert_eq!(text, "I'll read that file."),
        other => panic!("Expected TextDelta, got {other:?}"),
    }
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStop { index: 0 }
    ));

    // Tool use block: start, json deltas, stop
    match &events[4] {
        StreamEvent::ContentBlockStart {
            index: 1,
            content_block,
        } => match content_block {
            chet_types::ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "toolu_abc123");
                assert_eq!(name, "Read");
            }
            other => panic!("Expected ToolUse, got {other:?}"),
        },
        other => panic!("Expected ContentBlockStart, got {other:?}"),
    }

    // JSON input deltas
    assert!(matches!(
        events[5],
        StreamEvent::ContentBlockDelta {
            index: 1,
            delta: ContentDelta::InputJsonDelta { .. },
        }
    ));
    assert!(matches!(
        events[6],
        StreamEvent::ContentBlockDelta {
            index: 1,
            delta: ContentDelta::InputJsonDelta { .. },
        }
    ));

    assert!(matches!(
        events[7],
        StreamEvent::ContentBlockStop { index: 1 }
    ));

    // Stop reason should be tool_use
    match &events[8] {
        StreamEvent::MessageDelta { delta, .. } => {
            assert_eq!(delta.stop_reason, Some(chet_types::StopReason::ToolUse));
        }
        other => panic!("Expected MessageDelta, got {other:?}"),
    }

    assert!(matches!(events[9], StreamEvent::MessageStop));
    assert_eq!(events.len(), 10);
}

// ---------------------------------------------------------------------------
// Test: extended thinking response
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_thinking_response() {
    let sse = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test3\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5-20250929\",\"stop_reason\":null,\"usage\":{\"input_tokens\":50,\"output_tokens\":1}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me think about this...\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_abc\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"The answer is 42.\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":30}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

    let events = collect_events(stream_from_sse(sse)).await;

    // Thinking block
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart { index: 0, .. }
    ));
    match &events[2] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::ThinkingDelta { thinking },
            ..
        } => assert_eq!(thinking, "Let me think about this..."),
        other => panic!("Expected ThinkingDelta, got {other:?}"),
    }
    match &events[3] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::SignatureDelta { signature },
            ..
        } => assert_eq!(signature, "sig_abc"),
        other => panic!("Expected SignatureDelta, got {other:?}"),
    }

    // Events: [0] msg_start, [1] think_start, [2] think_delta, [3] sig_delta,
    //         [4] think_stop, [5] text_start, [6] text_delta, [7] text_stop,
    //         [8] msg_delta, [9] msg_stop
    assert!(matches!(
        events[5],
        StreamEvent::ContentBlockStart { index: 1, .. }
    ));
    match &events[6] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::TextDelta { text },
            ..
        } => assert_eq!(text, "The answer is 42."),
        other => panic!("Expected TextDelta, got {other:?}"),
    }

    assert_eq!(events.len(), 10);
}

// ---------------------------------------------------------------------------
// Test: error response from API
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_error_event() {
    let sse = "\
event: error\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\
\n";

    let events = collect_events(stream_from_sse(sse)).await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        StreamEvent::Error { error } => {
            assert_eq!(error.error_type, "overloaded_error");
            assert_eq!(error.message, "Overloaded");
        }
        other => panic!("Expected Error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test: chunked delivery (events split across multiple TCP chunks)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_chunked_delivery() {
    // Simulate the API sending data in small, irregular chunks
    let stream = stream_from_chunks(vec![
        "event: mes",
        "sage_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_c\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5-20250929\",\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
        "\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);

    let events = collect_events(stream).await;

    assert!(matches!(events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(events[1], StreamEvent::ContentBlockStart { .. }));
    match &events[2] {
        StreamEvent::ContentBlockDelta {
            delta: ContentDelta::TextDelta { text },
            ..
        } => assert_eq!(text, "Hi"),
        other => panic!("Expected TextDelta, got {other:?}"),
    }
    assert!(matches!(events[3], StreamEvent::ContentBlockStop { .. }));
    assert!(matches!(events[4], StreamEvent::MessageDelta { .. }));
    assert!(matches!(events[5], StreamEvent::MessageStop));
    assert_eq!(events.len(), 6);
}

// ---------------------------------------------------------------------------
// Test: prompt caching fields in usage
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_usage_with_cache_fields() {
    let sse = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_cache\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-5-20250929\",\"stop_reason\":null,\"usage\":{\"input_tokens\":100,\"output_tokens\":1,\"cache_creation_input_tokens\":2400,\"cache_read_input_tokens\":500}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Cached!\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
\n";

    let events = collect_events(stream_from_sse(sse)).await;

    // Verify cache fields parsed in MessageStart
    match &events[0] {
        StreamEvent::MessageStart { message } => {
            assert_eq!(message.usage.input_tokens, 100);
            assert_eq!(message.usage.cache_creation_input_tokens, 2400);
            assert_eq!(message.usage.cache_read_input_tokens, 500);
        }
        other => panic!("Expected MessageStart, got {other:?}"),
    }
}
