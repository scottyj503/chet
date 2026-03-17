//! Inline markdown rendering: **bold**, *italic*, `code`, [text](url).

use crate::style;
use crossterm::style::{Attribute, SetAttribute};
use std::fmt::Write;

/// Parse and render inline markdown: **bold**, *italic*, `code`, [text](url).
pub(crate) fn render_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Escaped character
        if chars[i] == '\\' && i + 1 < len {
            let next = chars[i + 1];
            if matches!(next, '*' | '`' | '[' | ']' | '(' | ')' | '\\' | '_') {
                out.push(next);
                i += 2;
                continue;
            }
        }

        // Bold + italic: ***text***
        if i + 2 < len && chars[i] == '*' && chars[i + 1] == '*' && chars[i + 2] == '*' {
            if let Some(end) = find_closing(&chars, i + 3, &['*', '*', '*']) {
                let inner: String = chars[i + 3..end].iter().collect();
                let _ = write!(
                    out,
                    "{}{}{}{}",
                    SetAttribute(Attribute::Bold),
                    SetAttribute(Attribute::Italic),
                    inner,
                    SetAttribute(Attribute::Reset)
                );
                i = end + 3;
                continue;
            }
        }

        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing(&chars, i + 2, &['*', '*']) {
                let inner: String = chars[i + 2..end].iter().collect();
                let _ = write!(
                    out,
                    "{}{}{}",
                    SetAttribute(Attribute::Bold),
                    inner,
                    SetAttribute(Attribute::Reset)
                );
                i = end + 2;
                continue;
            }
        }

        // Italic: *text*
        if chars[i] == '*' {
            if let Some(end) = find_closing(&chars, i + 1, &['*']) {
                // Make sure it's not empty
                if end > i + 1 {
                    let inner: String = chars[i + 1..end].iter().collect();
                    let _ = write!(
                        out,
                        "{}{}{}",
                        SetAttribute(Attribute::Italic),
                        inner,
                        SetAttribute(Attribute::Reset)
                    );
                    i = end + 1;
                    continue;
                }
            }
        }

        // Inline code: `text`
        if chars[i] == '`' {
            if let Some(end) = find_char(&chars, i + 1, '`') {
                let inner: String = chars[i + 1..end].iter().collect();
                let _ = write!(out, "{}", style::code_span(&inner));
                i = end + 1;
                continue;
            }
        }

        // Link: [text](url)
        if chars[i] == '[' {
            if let Some((text_str, url, end_pos)) = parse_link(&chars, i) {
                let _ = write!(out, "{}", style::link(&text_str, &url));
                i = end_pos;
                continue;
            }
        }

        // Plain character
        out.push(chars[i]);
        i += 1;
    }

    out
}

/// Find closing marker sequence starting at `start`.
fn find_closing(chars: &[char], start: usize, marker: &[char]) -> Option<usize> {
    let mlen = marker.len();
    if chars.len() < mlen {
        return None;
    }
    (start..=chars.len() - mlen).find(|&i| chars[i..i + mlen] == *marker)
}

/// Find a single character starting at `start`.
fn find_char(chars: &[char], start: usize, ch: char) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == ch)
}

/// Parse a markdown link `[text](url)` starting at position `i` (the `[`).
/// Returns (text, url, end_position) where end_position is past the closing `)`.
fn parse_link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    // Find closing ]
    let close_bracket = find_char(chars, i + 1, ']')?;
    // Must be followed by (
    if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
        return None;
    }
    // Find closing )
    let close_paren = find_char(chars, close_bracket + 2, ')')?;

    let text: String = chars[i + 1..close_bracket].iter().collect();
    let url: String = chars[close_bracket + 2..close_paren].iter().collect();

    Some((text, url, close_paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_bold() {
        let result = render_inline("This is **bold** text");
        assert!(result.contains("bold"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn render_italic() {
        let result = render_inline("This is *italic* text");
        assert!(result.contains("italic"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn render_inline_code() {
        let result = render_inline("Use `cargo build` to compile");
        assert!(result.contains("cargo build"));
    }

    #[test]
    fn render_link() {
        let result = render_inline("Visit [Rust](https://rust-lang.org) for more");
        assert!(result.contains("Rust"));
        assert!(result.contains("https://rust-lang.org"));
    }

    #[test]
    fn unmatched_bold_renders_literally() {
        let result = render_inline("this has **no closing");
        assert!(result.contains("**no closing"));
    }

    #[test]
    fn escaped_markdown_renders_literally() {
        let result = render_inline("this is \\*not italic\\*");
        assert!(result.contains("*not italic*"));
        assert!(!result.contains('\x1b'));
    }

    #[test]
    fn bold_italic_combined() {
        let result = render_inline("***bold italic***");
        assert!(result.contains("bold italic"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn parse_link_function() {
        let chars: Vec<char> = "[text](url)".chars().collect();
        let result = parse_link(&chars, 0);
        assert!(result.is_some());
        let (text, url, end) = result.unwrap();
        assert_eq!(text, "text");
        assert_eq!(url, "url");
        assert_eq!(end, 11);
    }
}
