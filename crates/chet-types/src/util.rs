//! Utility functions for safe string handling and atomic file I/O.

use std::io;
use std::path::Path;

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

/// Write content to a file atomically via a temporary file + rename.
/// Prevents corruption from crashes or concurrent writes.
pub fn atomic_write_file(path: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
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
    fn atomic_write_file_creates_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        atomic_write_file(&path, b"hello world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
        // tmp file should not remain
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn atomic_write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("dir").join("file.txt");
        atomic_write_file(&path, b"nested").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
    }

    #[test]
    fn atomic_write_file_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        atomic_write_file(&path, b"first").unwrap();
        atomic_write_file(&path, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn truncate_string_emoji() {
        let mut s = String::from("\u{1F600}\u{1F601}"); // 8 bytes
        truncate_string(&mut s, 5);
        assert_eq!(s, "\u{1F600}"); // only first emoji fits
    }
}
