//! Utility functions for safe string handling.

/// Find the largest byte index <= `i` that is on a UTF-8 char boundary.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut pos = i;
    // Walk backwards while we're at a continuation byte (0b10xxxxxx)
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// Truncate `&str` to at most `max_bytes`, never splitting a UTF-8 codepoint.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        &s[..floor_char_boundary(s, max_bytes)]
    }
}

/// Truncate a `String` in place to at most `max_bytes`, never splitting a UTF-8 codepoint.
pub fn truncate_string(s: &mut String, max_bytes: usize) {
    if s.len() > max_bytes {
        s.truncate(floor_char_boundary(s, max_bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_exact_boundary() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn truncate_str_empty() {
        assert_eq!(truncate_str("", 5), "");
    }

    #[test]
    fn truncate_str_zero_max() {
        assert_eq!(truncate_str("hello", 0), "");
    }

    #[test]
    fn truncate_str_emoji() {
        // Each emoji is 4 bytes
        let s = "\u{1F600}\u{1F601}\u{1F602}"; // 12 bytes
        assert_eq!(truncate_str(s, 4), "\u{1F600}");
        assert_eq!(truncate_str(s, 5), "\u{1F600}"); // can't fit partial emoji
        assert_eq!(truncate_str(s, 8), "\u{1F600}\u{1F601}");
    }

    #[test]
    fn truncate_str_cjk() {
        // CJK chars are 3 bytes each
        let s = "\u{4e16}\u{754c}"; // 6 bytes
        assert_eq!(truncate_str(s, 3), "\u{4e16}");
        assert_eq!(truncate_str(s, 4), "\u{4e16}"); // can't fit partial char
        assert_eq!(truncate_str(s, 6), "\u{4e16}\u{754c}");
    }

    #[test]
    fn truncate_str_accented() {
        let s = "caf\u{00e9}"; // 'e' with accent = 2 bytes, total 5
        assert_eq!(truncate_str(s, 4), "caf"); // can't fit the 2-byte char
        assert_eq!(truncate_str(s, 5), "caf\u{00e9}");
    }

    #[test]
    fn truncate_string_in_place() {
        let mut s = String::from("hello world");
        truncate_string(&mut s, 5);
        assert_eq!(s, "hello");
    }

    #[test]
    fn truncate_string_no_op() {
        let mut s = String::from("hi");
        truncate_string(&mut s, 10);
        assert_eq!(s, "hi");
    }

    #[test]
    fn truncate_string_emoji() {
        let mut s = String::from("\u{1F600}\u{1F601}"); // 8 bytes
        truncate_string(&mut s, 5);
        assert_eq!(s, "\u{1F600}"); // only first emoji fits
    }
}
