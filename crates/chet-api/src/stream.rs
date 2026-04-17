//! Async stream that converts SSE events into typed StreamEvents.

use chet_types::sse::{SseParser, parse_stream_event};
use chet_types::{ApiError, StreamEvent};
use futures_core::Stream;
use pin_project_lite::pin_project;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

pin_project! {
    /// An async stream of typed [`StreamEvent`]s from the Anthropic Messages API.
    pub struct MessageStream {
        #[pin]
        inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
        parser: SseParser,
        pending_events: VecDeque<Result<StreamEvent, ApiError>>,
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
            pending_events: VecDeque::new(),
        }
    }
}

impl Stream for MessageStream {
    type Item = Result<StreamEvent, ApiError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // Drain any previously buffered events first
        if let Some(event) = this.pending_events.pop_front() {
            if !this.pending_events.is_empty() {
                // More events buffered — wake immediately so we yield them
                cx.waker().wake_by_ref();
            }
            return Poll::Ready(Some(event));
        }

        // Poll the underlying byte stream
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                let text = String::from_utf8_lossy(&bytes);
                let sse_events = this.parser.feed(&text);

                // Parse all SSE events into typed StreamEvents
                for sse_event in sse_events {
                    match parse_stream_event(&sse_event.event_type, &sse_event.data) {
                        Ok(Some(stream_event)) => {
                            this.pending_events.push_back(Ok(stream_event));
                        }
                        Ok(None) => {} // Unknown/skipped event type
                        Err(e) => {
                            this.pending_events.push_back(Err(e));
                        }
                    }
                }

                if this.pending_events.is_empty() {
                    // Got bytes but no complete event yet — wake to try again
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    let event = this.pending_events.pop_front().unwrap();
                    if !this.pending_events.is_empty() {
                        cx.waker().wake_by_ref();
                    }
                    Poll::Ready(Some(event))
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(ApiError::Network(e.to_string())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_types::ContentDelta;
    use futures_util::StreamExt;

    /// Create a MessageStream from raw SSE text chunks.
    fn stream_from_chunks(chunks: Vec<&str>) -> MessageStream {
        let byte_stream = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|s| Ok(bytes::Bytes::from(s.to_owned())))
                .collect::<Vec<Result<bytes::Bytes, reqwest::Error>>>(),
        );
        MessageStream::new(byte_stream)
    }

    #[tokio::test]
    async fn test_single_event_per_chunk() {
        let mut stream = stream_from_chunks(vec!["event: ping\ndata: {}\n\n"]);
        let event = stream.next().await.unwrap().unwrap();
        assert!(matches!(event, StreamEvent::Ping));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_multiple_events_in_one_chunk() {
        // This is the bug that was fixed — previously only the first event was yielded
        let mut stream = stream_from_chunks(vec![
            "event: ping\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        ]);

        let event1 = stream.next().await.unwrap().unwrap();
        assert!(matches!(event1, StreamEvent::Ping));

        let event2 = stream.next().await.unwrap().unwrap();
        assert!(matches!(event2, StreamEvent::MessageStop));

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_event_split_across_chunks() {
        let mut stream = stream_from_chunks(vec!["event: ping\n", "data: {}\n\n"]);
        let event = stream.next().await.unwrap().unwrap();
        assert!(matches!(event, StreamEvent::Ping));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_content_block_delta_text() {
        let data = r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let mut stream = stream_from_chunks(vec![&format!(
            "event: content_block_delta\ndata: {data}\n\n"
        )]);
        let event = stream.next().await.unwrap().unwrap();
        match event {
            StreamEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::TextDelta { text },
            } => {
                assert_eq!(index, 0);
                assert_eq!(text, "Hello");
            }
            other => panic!("Expected ContentBlockDelta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_three_events_in_one_chunk() {
        let mut stream = stream_from_chunks(vec![
            "event: ping\ndata: {}\n\nevent: ping\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        ]);

        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            StreamEvent::Ping
        ));
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            StreamEvent::Ping
        ));
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            StreamEvent::MessageStop
        ));
        assert!(stream.next().await.is_none());
    }
}
