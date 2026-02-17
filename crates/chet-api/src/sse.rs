//! Server-Sent Events (SSE) parser.
//!
//! Parses raw bytes from an HTTP response into SSE events according to
//! the W3C EventSource specification.

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
                // Comment line, skip
                continue;
            }

            if let Some((field, value)) = line.split_once(':') {
                // Trim leading space from value per SSE spec
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => event_type = Some(value.to_string()),
                    "data" => data_lines.push(value.to_string()),
                    _ => {} // Ignore unknown fields
                }
            } else if line == "data" {
                // Field with no value
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_event() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: message_start\ndata: {\"type\":\"message_start\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"type\":\"message_start\"}");
    }

    #[test]
    fn test_multiple_events() {
        let mut parser = SseParser::new();
        let events = parser.feed(
            "event: ping\ndata: {}\n\nevent: message_start\ndata: {\"type\":\"message_start\"}\n\n",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type.as_deref(), Some("ping"));
        assert_eq!(events[1].event_type.as_deref(), Some("message_start"));
    }

    #[test]
    fn test_partial_event() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: ping\n");
        assert_eq!(events.len(), 0);
        let events = parser.feed("data: {}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("ping"));
    }

    #[test]
    fn test_comment_lines_ignored() {
        let mut parser = SseParser::new();
        let events = parser.feed(": comment\nevent: ping\ndata: {}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("ping"));
    }

    #[test]
    fn test_data_with_leading_space() {
        let mut parser = SseParser::new();
        let events = parser.feed("data: hello world\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello world");
    }
}
