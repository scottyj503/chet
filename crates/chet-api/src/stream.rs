//! Async stream that converts SSE events into typed StreamEvents.

use crate::sse::SseParser;
use chet_types::{
    ApiError, ContentBlock, ContentDelta, CreateMessageResponse, MessageDelta, StreamEvent, Usage,
};
use futures_core::Stream;
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

pin_project! {
    /// An async stream of typed [`StreamEvent`]s from the Anthropic Messages API.
    pub struct MessageStream {
        #[pin]
        inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
        parser: SseParser,
        text_buf: String,
    }
}

impl MessageStream {
    /// Create a new MessageStream from a reqwest byte stream.
    pub fn new(
        byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(byte_stream),
            parser: SseParser::new(),
            text_buf: String::new(),
        }
    }
}

impl Stream for MessageStream {
    type Item = Result<StreamEvent, ApiError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // Try to parse any buffered text first
        if !this.text_buf.is_empty() {
            let events = this.parser.feed(this.text_buf.as_str());
            this.text_buf.clear();
            for sse_event in events {
                match parse_stream_event(&sse_event.event_type, &sse_event.data) {
                    Ok(Some(stream_event)) => return Poll::Ready(Some(Ok(stream_event))),
                    Ok(None) => continue,
                    Err(e) => return Poll::Ready(Some(Err(e))),
                }
            }
        }

        // Poll the underlying byte stream
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                let text = String::from_utf8_lossy(&bytes);
                let events = this.parser.feed(&text);

                for sse_event in events {
                    match parse_stream_event(&sse_event.event_type, &sse_event.data) {
                        Ok(Some(stream_event)) => return Poll::Ready(Some(Ok(stream_event))),
                        Ok(None) => continue,
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }

                // Got bytes but no complete event yet — wake to try again
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(ApiError::Network(e.to_string())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Parse an SSE event into a typed StreamEvent.
fn parse_stream_event(
    event_type: &Option<String>,
    data: &str,
) -> Result<Option<StreamEvent>, ApiError> {
    let event_type = match event_type {
        Some(t) => t.as_str(),
        None => return Ok(None),
    };

    let parse_err = |e: serde_json::Error| ApiError::StreamParse(format!("{event_type}: {e}"));

    match event_type {
        "message_start" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                message: CreateMessageResponse,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::MessageStart { message: w.message }))
        }
        "content_block_start" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                index: usize,
                content_block: ContentBlock,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::ContentBlockStart {
                index: w.index,
                content_block: w.content_block,
            }))
        }
        "content_block_delta" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                index: usize,
                delta: ContentDelta,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::ContentBlockDelta {
                index: w.index,
                delta: w.delta,
            }))
        }
        "content_block_stop" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                index: usize,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::ContentBlockStop { index: w.index }))
        }
        "message_delta" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                delta: MessageDelta,
                usage: Option<Usage>,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::MessageDelta {
                delta: w.delta,
                usage: w.usage,
            }))
        }
        "message_stop" => Ok(Some(StreamEvent::MessageStop)),
        "ping" => Ok(Some(StreamEvent::Ping)),
        "error" => {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                error: chet_types::ApiErrorResponse,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::Error { error: w.error }))
        }
        _ => {
            tracing::debug!("Unknown SSE event type: {event_type}");
            Ok(None)
        }
    }
}
