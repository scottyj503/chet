//! AWS EventStream binary frame parser for Bedrock streaming responses.
//!
//! Bedrock uses AWS EventStream framing instead of SSE. Each frame has:
//! - 12-byte prelude: total_len (4) + headers_len (4) + prelude_crc (4)
//! - Headers section: key-value pairs (event type, content type, etc.)
//! - Payload: the actual JSON data (same format as Anthropic SSE)
//! - 4-byte message CRC at the end

use bytes::{Buf, BytesMut};

/// A parsed EventStream message.
#[derive(Debug, Clone)]
pub struct EventStreamMessage {
    pub headers: Vec<(String, String)>,
    pub payload: Vec<u8>,
}

impl EventStreamMessage {
    /// Get header value by name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Get the event type from the `:event-type` header.
    pub fn event_type(&self) -> Option<&str> {
        self.header(":event-type")
    }

    /// Get the payload as a UTF-8 string.
    pub fn payload_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.payload).ok()
    }
}

/// Incremental EventStream parser. Feed it bytes, get back complete messages.
pub struct EventStreamParser {
    buffer: BytesMut,
}

impl EventStreamParser {
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::new(),
        }
    }

    /// Feed a chunk of bytes and return any complete messages.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<EventStreamMessage> {
        self.buffer.extend_from_slice(chunk);
        let mut messages = Vec::new();

        while let Some(msg) = self.try_parse_message() {
            messages.push(msg);
        }

        messages
    }

    /// Try to parse one complete message from the buffer.
    fn try_parse_message(&mut self) -> Option<EventStreamMessage> {
        // Need at least 12 bytes for the prelude
        if self.buffer.len() < 12 {
            return None;
        }

        // Read total length from the first 4 bytes (big-endian)
        let total_len = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;

        // Need the full message
        if self.buffer.len() < total_len {
            return None;
        }

        // Extract the full message and advance the buffer
        let msg_bytes = self.buffer.split_to(total_len);

        // Parse the prelude
        let headers_len =
            u32::from_be_bytes([msg_bytes[4], msg_bytes[5], msg_bytes[6], msg_bytes[7]]) as usize;
        // Skip prelude CRC (bytes 8-11)

        // Parse headers (start after 12-byte prelude)
        let headers_start = 12;
        let headers_end = headers_start + headers_len;
        let headers = parse_headers(&msg_bytes[headers_start..headers_end]);

        // Payload is between headers and the 4-byte message CRC
        let payload_start = headers_end;
        let payload_end = total_len - 4; // last 4 bytes are message CRC
        let payload = if payload_start < payload_end {
            msg_bytes[payload_start..payload_end].to_vec()
        } else {
            Vec::new()
        };

        Some(EventStreamMessage { headers, payload })
    }
}

impl Default for EventStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse EventStream headers from raw bytes.
/// Header format: name_len(1) + name + type(1) + value_len(2) + value
fn parse_headers(mut data: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::new();

    while data.len() > 3 {
        // Header name: 1-byte length + name bytes
        let name_len = data[0] as usize;
        data = &data[1..];
        if data.len() < name_len {
            break;
        }
        let name = String::from_utf8_lossy(&data[..name_len]).to_string();
        data = &data[name_len..];

        // Header type (1 byte) — we only care about type 7 (string)
        if data.is_empty() {
            break;
        }
        let header_type = data[0];
        data = &data[1..];

        match header_type {
            7 => {
                // String value: 2-byte length + value bytes
                if data.len() < 2 {
                    break;
                }
                let val_len = u16::from_be_bytes([data[0], data[1]]) as usize;
                data = &data[2..];
                if data.len() < val_len {
                    break;
                }
                let value = String::from_utf8_lossy(&data[..val_len]).to_string();
                data = &data[val_len..];
                headers.push((name, value));
            }
            _ => {
                // Unknown type — skip rest of headers since we don't know the length
                break;
            }
        }
    }

    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal EventStream message with string headers and a payload.
    fn build_message(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
        // Build headers section
        let mut header_bytes = Vec::new();
        for (name, value) in headers {
            header_bytes.push(name.len() as u8);
            header_bytes.extend_from_slice(name.as_bytes());
            header_bytes.push(7); // string type
            header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
            header_bytes.extend_from_slice(value.as_bytes());
        }

        let headers_len = header_bytes.len() as u32;
        let total_len = 12 + header_bytes.len() + payload.len() + 4; // prelude + headers + payload + msg_crc

        let mut msg = Vec::new();
        msg.extend_from_slice(&(total_len as u32).to_be_bytes());
        msg.extend_from_slice(&headers_len.to_be_bytes());
        msg.extend_from_slice(&[0u8; 4]); // prelude CRC (not validated)
        msg.extend_from_slice(&header_bytes);
        msg.extend_from_slice(payload);
        msg.extend_from_slice(&[0u8; 4]); // message CRC (not validated)
        msg
    }

    #[test]
    fn parse_single_message() {
        let mut parser = EventStreamParser::new();
        let msg = build_message(
            &[
                (":event-type", "chunk"),
                (":content-type", "application/json"),
            ],
            b"{\"type\":\"content_block_delta\"}",
        );
        let messages = parser.feed(&msg);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event_type(), Some("chunk"));
        assert_eq!(
            messages[0].header(":content-type"),
            Some("application/json")
        );
        assert!(
            messages[0]
                .payload_str()
                .unwrap()
                .contains("content_block_delta")
        );
    }

    #[test]
    fn parse_partial_then_complete() {
        let msg = build_message(&[(":event-type", "chunk")], b"{}");
        let mut parser = EventStreamParser::new();

        // Feed first half
        let half = msg.len() / 2;
        assert!(parser.feed(&msg[..half]).is_empty());

        // Feed second half
        let messages = parser.feed(&msg[half..]);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn parse_two_messages_at_once() {
        let msg1 = build_message(&[(":event-type", "chunk")], b"{\"a\":1}");
        let msg2 = build_message(&[(":event-type", "chunk")], b"{\"b\":2}");

        let mut combined = msg1;
        combined.extend_from_slice(&msg2);

        let mut parser = EventStreamParser::new();
        let messages = parser.feed(&combined);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn parse_empty_payload() {
        let mut parser = EventStreamParser::new();
        let msg = build_message(&[(":event-type", "ping")], b"");
        let messages = parser.feed(&msg);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].payload.is_empty());
    }

    #[test]
    fn insufficient_bytes_returns_empty() {
        let mut parser = EventStreamParser::new();
        assert!(parser.feed(&[0u8; 5]).is_empty()); // less than 12 bytes
    }
}
