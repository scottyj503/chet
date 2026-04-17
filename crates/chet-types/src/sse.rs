//! Server-Sent Events (SSE) parser and stream event parser.
//!
//! Generic W3C SSE parsing + Anthropic-specific event type mapping.
//! Shared by all providers that use SSE (Anthropic direct, Vertex AI).

use crate::{
    ApiError, ApiErrorResponse, ContentBlock, ContentDelta, CreateMessageResponse, MessageDelta,
    StreamEvent, Usage,
};

/// A single SSE event parsed from the stream.
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: String,
}

/// Incremental SSE parser that processes bytes into events.
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed a chunk of text and return any complete events.
    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // Process complete event blocks (separated by double newlines)
        while let Some(pos) = self.buffer.find("\n\n") {
            let block = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            if let Some(event) = Self::parse_block(&block) {
                events.push(event);
            }
        }

        events
    }

    /// Parse a single SSE block (lines between double newlines) into an event.
    fn parse_block(block: &str) -> Option<SseEvent> {
        let mut event_type = None;
        let mut data_lines = Vec::new();

        for line in block.lines() {
            if line.starts_with(':') {
                continue;
            }

            if let Some((field, value)) = line.split_once(':') {
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => event_type = Some(value.to_string()),
                    "data" => data_lines.push(value.to_string()),
                    _ => {}
                }
            } else if line == "data" {
                data_lines.push(String::new());
            }
        }

        if data_lines.is_empty() {
            return None;
        }

        Some(SseEvent {
            event_type,
            data: data_lines.join("\n"),
        })
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse an SSE event into a typed StreamEvent.
///
/// Maps Anthropic SSE event names (message_start, content_block_delta, etc.)
/// to the `StreamEvent` enum. Used by both Anthropic direct and Vertex AI providers.
pub fn parse_stream_event(
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
                error: ApiErrorResponse,
            }
            let w: Wrapper = serde_json::from_str(data).map_err(parse_err)?;
            Ok(Some(StreamEvent::Error { error: w.error }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_event() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: message_start\ndata: {\"type\":\"message_start\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("message_start"));
    }

    #[test]
    fn test_multiple_events() {
        let mut parser = SseParser::new();
        let events = parser.feed(
            "event: ping\ndata: {}\n\nevent: message_start\ndata: {\"type\":\"message_start\"}\n\n",
        );
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_partial_event() {
        let mut parser = SseParser::new();
        assert_eq!(parser.feed("event: ping\n").len(), 0);
        assert_eq!(parser.feed("data: {}\n\n").len(), 1);
    }

    #[test]
    fn test_comment_lines_ignored() {
        let mut parser = SseParser::new();
        let events = parser.feed(": comment\nevent: ping\ndata: {}\n\n");
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_parse_ping() {
        let result = parse_stream_event(&Some("ping".to_string()), "{}").unwrap();
        assert!(matches!(result, Some(StreamEvent::Ping)));
    }

    #[test]
    fn test_parse_message_stop() {
        let result = parse_stream_event(&Some("message_stop".to_string()), "{}").unwrap();
        assert!(matches!(result, Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn test_parse_unknown_event() {
        let result = parse_stream_event(&Some("unknown_type".to_string()), "{}").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_no_event_type() {
        let result = parse_stream_event(&None, "{}").unwrap();
        assert!(result.is_none());
    }
}
