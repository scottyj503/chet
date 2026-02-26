//! Integration tests for the retry/backoff logic in `ApiClient`.
//!
//! Uses a raw TCP test server to simulate retryable HTTP errors (429, 500)
//! and verify that `ApiClient::create_message_stream()` retries transparently.
//!
//! Run with: `cargo test -p chet-api --test retry_integration -- --ignored`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chet_api::{ApiClient, MessageStream, RetryConfig};
use chet_types::StreamEvent;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Minimal valid SSE response that produces a complete message cycle.
const SSE_SUCCESS_BODY: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"test\",\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Retried OK\"}}\n\
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

/// Build the HTTP response for a 429 rate limit error.
fn http_429_response() -> String {
    let body = r#"{"error":{"message":"rate limited"}}"#;
    format!(
        "HTTP/1.1 429 Too Many Requests\r\n\
         Content-Type: application/json\r\n\
         Retry-After: 0.01\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

/// Build the HTTP response for a 500 server error.
fn http_500_response() -> String {
    let body = r#"{"error":{"message":"internal error"}}"#;
    format!(
        "HTTP/1.1 500 Internal Server Error\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

/// Build the HTTP response for a 200 OK with SSE body.
fn http_200_sse_response() -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        SSE_SUCCESS_BODY
    )
}

/// Build the HTTP response for a 401 auth error (non-retryable).
fn http_401_response() -> String {
    let body = r#"{"error":{"message":"invalid api key"}}"#;
    format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    )
}

/// Start a test TCP server that returns pre-configured responses.
/// `responses` is a list of HTTP response strings — one per incoming connection.
/// Returns the server address and a handle to the request counter.
async fn start_test_server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    tokio::spawn(async move {
        let responses = Arc::new(responses);
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let idx = counter_clone.fetch_add(1, Ordering::SeqCst);
            let responses = Arc::clone(&responses);

            tokio::spawn(async move {
                // Read the HTTP request (consume it so the socket doesn't hang)
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await;

                // Send the pre-configured response for this request index
                if idx < responses.len() {
                    let _ = socket.write_all(responses[idx].as_bytes()).await;
                    let _ = socket.flush().await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://{addr}"), counter)
}

/// Build an ApiClient with fast retry config pointing at the test server.
fn make_client(base_url: &str) -> ApiClient {
    ApiClient::new("test-key", base_url)
        .unwrap()
        .with_retry_config(RetryConfig {
            max_retries: 2,
            initial_delay_ms: 10, // fast for tests
            max_delay_ms: 100,
            backoff_factor: 2.0,
        })
}

/// Build a minimal CreateMessageRequest for testing.
fn test_request() -> chet_types::CreateMessageRequest {
    chet_types::CreateMessageRequest {
        model: "test-model".to_string(),
        max_tokens: 100,
        messages: vec![chet_types::Message {
            role: chet_types::Role::User,
            content: vec![chet_types::ContentBlock::Text {
                text: "test".to_string(),
            }],
        }],
        system: None,
        tools: None,
        stop_sequences: None,
        temperature: None,
        thinking: None,
        stream: true,
    }
}

/// Collect all events from a MessageStream.
async fn collect_events(mut stream: MessageStream) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        events.push(result.expect("stream event should parse"));
    }
    events
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 429 on first attempt, 200 on second. Retry should be transparent.
#[tokio::test]
#[ignore]
async fn test_retry_on_429_then_success() {
    let (base_url, counter) =
        start_test_server(vec![http_429_response(), http_200_sse_response()]).await;

    let client = make_client(&base_url);
    let request = test_request();

    let stream = client.create_message_stream(&request).await;
    assert!(
        stream.is_ok(),
        "should succeed after retry: {}",
        stream.err().map(|e| format!("{e:?}")).unwrap_or_default()
    );

    // Verify 2 requests were made (1 failed + 1 success)
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "should have made 2 requests"
    );

    // Verify the stream yields valid events
    let events = collect_events(stream.unwrap()).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::MessageStart { .. })),
        "should have MessageStart event"
    );
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::MessageStop)),
        "should have MessageStop event"
    );
}

/// 500 on first attempt, 200 on second. Server errors are retryable.
#[tokio::test]
#[ignore]
async fn test_retry_on_500_then_success() {
    let (base_url, counter) =
        start_test_server(vec![http_500_response(), http_200_sse_response()]).await;

    let client = make_client(&base_url);
    let request = test_request();

    let stream = client.create_message_stream(&request).await;
    assert!(
        stream.is_ok(),
        "should succeed after retry: {}",
        stream.err().map(|e| format!("{e:?}")).unwrap_or_default()
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "should have made 2 requests"
    );
}

/// 429 on all attempts (3 total with max_retries=2). Should fail after exhausting retries.
#[tokio::test]
#[ignore]
async fn test_retry_exhausted() {
    let (base_url, counter) = start_test_server(vec![
        http_429_response(),
        http_429_response(),
        http_429_response(),
    ])
    .await;

    let client = make_client(&base_url);
    let request = test_request();

    let result = client.create_message_stream(&request).await;
    assert!(result.is_err(), "should fail after exhausting retries");
    match result {
        Err(chet_types::ApiError::RateLimited { .. }) => {} // expected
        Err(e) => panic!("expected RateLimited, got: {e:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "should have made 3 requests (1 + 2 retries)"
    );
}

/// 401 is not retryable — should fail immediately without retrying.
#[tokio::test]
#[ignore]
async fn test_no_retry_on_401() {
    let (base_url, counter) = start_test_server(vec![
        http_401_response(),
        http_200_sse_response(), // should never be reached
    ])
    .await;

    let client = make_client(&base_url);
    let request = test_request();

    let result = client.create_message_stream(&request).await;
    assert!(result.is_err(), "should fail on 401");
    match result {
        Err(chet_types::ApiError::Auth { .. }) => {} // expected
        Err(e) => panic!("expected Auth error, got: {e:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "should have made only 1 request (no retry)"
    );
}
