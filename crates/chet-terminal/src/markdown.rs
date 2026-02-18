//! Streaming markdown renderer — converts TextDelta chunks into styled terminal output.
//!
//! Text arrives as arbitrary token-level fragments. We buffer until we have complete
//! lines, then render each line with inline markdown styling and code block highlighting.

use crate::highlight::CodeHighlighter;
use crate::style;
use crossterm::style::{Attribute, SetAttribute};
use crossterm::terminal;
use std::fmt::Write as FmtWrite;
use std::io::Write;

/// Tracks what block-level context we're currently in.
struct RenderState {
    in_code_block: bool,
    code_lang: String,
}

impl RenderState {
    fn new() -> Self {
        Self {
            in_code_block: false,
            code_lang: String::new(),
        }
    }
}

/// A streaming markdown renderer that accepts text deltas and writes styled output.
///
/// Call `push()` for each text delta, then `finish()` when the response is complete.
pub struct StreamingMarkdownRenderer {
    writer: Box<dyn Write>,
    buffer: String,
    state: RenderState,
    highlighter: CodeHighlighter,
    term_width: u16,
}

impl StreamingMarkdownRenderer {
    /// Create a new renderer writing to the given output.
    pub fn new(writer: Box<dyn Write>) -> Self {
        let (width, _) = terminal::size().unwrap_or((80, 24));
        Self {
            writer,
            buffer: String::new(),
            state: RenderState::new(),
            highlighter: CodeHighlighter::new(),
            term_width: width,
        }
    }

    /// Create a renderer with a specific width (useful for testing).
    #[cfg(test)]
    fn with_width(writer: Box<dyn Write>, width: u16) -> Self {
        Self {
            writer,
            buffer: String::new(),
            state: RenderState::new(),
            highlighter: CodeHighlighter::new(),
            term_width: width,
        }
    }

    /// Push a text delta into the renderer. Complete lines are rendered immediately.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);

        // Process all complete lines (ending with \n)
        while let Some(newline_pos) = self.buffer.find('\n') {
            let line: String = self.buffer[..newline_pos].to_string();
            self.buffer = self.buffer[newline_pos + 1..].to_string();
            self.render_line(&line);
        }
    }

    /// Flush any remaining buffered text. Call when the response is complete.
    pub fn finish(&mut self) {
        if !self.buffer.is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            self.render_line(&remaining);
        }

        // If we're still in a code block at finish, close it
        if self.state.in_code_block {
            self.state.in_code_block = false;
            self.highlighter.end_block();
        }

        let _ = self.writer.flush();
    }

    /// Render a single complete line.
    fn render_line(&mut self, line: &str) {
        let trimmed = line.trim();

        // Check for code fence toggle
        if trimmed.starts_with("```") {
            if self.state.in_code_block {
                // Closing fence
                self.state.in_code_block = false;
                self.state.code_lang.clear();
                self.highlighter.end_block();
                // Print a dim closing line
                let _ = writeln!(
                    self.writer,
                    "{}```{}",
                    SetAttribute(Attribute::Dim),
                    SetAttribute(Attribute::Reset)
                );
            } else {
                // Opening fence — extract language
                let lang = trimmed.trim_start_matches('`').trim().to_string();
                self.state.in_code_block = true;
                self.state.code_lang = lang.clone();
                self.highlighter.start_block(&lang);
                // Print a dim opening line with language
                if lang.is_empty() {
                    let _ = writeln!(
                        self.writer,
                        "{}```{}",
                        SetAttribute(Attribute::Dim),
                        SetAttribute(Attribute::Reset)
                    );
                } else {
                    let _ = writeln!(
                        self.writer,
                        "{}```{}{}",
                        SetAttribute(Attribute::Dim),
                        lang,
                        SetAttribute(Attribute::Reset)
                    );
                }
            }
            let _ = self.writer.flush();
            return;
        }

        // Inside a code block — syntax highlight
        if self.state.in_code_block {
            let highlighted = self.highlighter.highlight_line(line);
            let _ = writeln!(self.writer, "  {highlighted}");
            let _ = self.writer.flush();
            return;
        }

        // Heading
        if let Some(heading) = parse_heading(trimmed) {
            let styled = style::heading(&render_inline(heading.text), heading.level);
            let _ = writeln!(self.writer, "{styled}");
            let _ = self.writer.flush();
            return;
        }

        // Horizontal rule
        if is_horizontal_rule(trimmed) {
            let _ = writeln!(self.writer, "{}", style::horizontal_rule(self.term_width));
            let _ = self.writer.flush();
            return;
        }

        // Blockquote
        if let Some(content) = trimmed
            .strip_prefix("> ")
            .or_else(|| if trimmed == ">" { Some("") } else { None })
        {
            let prefix = style::blockquote_prefix();
            let styled = render_inline(content);
            let _ = writeln!(self.writer, "{prefix}{styled}");
            let _ = self.writer.flush();
            return;
        }

        // Unordered list item
        if let Some((depth, content)) = parse_unordered_list(line) {
            let bullet = style::list_bullet(depth);
            let styled = render_inline(content);
            let _ = writeln!(self.writer, "{bullet} {styled}");
            let _ = self.writer.flush();
            return;
        }

        // Ordered list item
        if let Some((depth, number, content)) = parse_ordered_list(line) {
            let num = style::list_number(number, depth);
            let styled = render_inline(content);
            let _ = writeln!(self.writer, "{num} {styled}");
            let _ = self.writer.flush();
            return;
        }

        // Empty line
        if trimmed.is_empty() {
            let _ = writeln!(self.writer);
            let _ = self.writer.flush();
            return;
        }

        // Normal paragraph text — apply inline markdown
        let styled = render_inline(trimmed);
        let _ = writeln!(self.writer, "{styled}");
        let _ = self.writer.flush();
    }
}

// ---------------------------------------------------------------------------
// Block-level parsing helpers
// ---------------------------------------------------------------------------

struct HeadingInfo<'a> {
    level: u8,
    text: &'a str,
}

fn parse_heading(line: &str) -> Option<HeadingInfo<'_>> {
    if !line.starts_with('#') {
        return None;
    }
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    // Must be followed by space or be empty
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some(HeadingInfo {
        level: hashes as u8,
        text: rest.trim(),
    })
}

fn is_horizontal_rule(line: &str) -> bool {
    if line.len() < 3 {
        return false;
    }
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < 3 {
        return false;
    }
    let first = chars[0];
    (first == '-' || first == '*' || first == '_') && chars.iter().all(|&c| c == first)
}

fn parse_unordered_list(line: &str) -> Option<(u8, &str)> {
    // Count leading spaces for depth
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[indent..];

    if let Some(content) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        let depth = (indent / 2) as u8;
        Some((depth, content))
    } else {
        None
    }
}

fn parse_ordered_list(line: &str) -> Option<(u8, u32, &str)> {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[indent..];

    // Match digits followed by ". "
    let digit_end = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digit_end == 0 {
        return None;
    }
    let after_digits = &rest[digit_end..];
    if let Some(content) = after_digits.strip_prefix(". ") {
        let number: u32 = rest[..digit_end].parse().ok()?;
        let depth = (indent / 2) as u8;
        Some((depth, number, content))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Inline markdown rendering
// ---------------------------------------------------------------------------

/// Parse and render inline markdown: **bold**, *italic*, `code`, [text](url).
fn render_inline(text: &str) -> String {
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

    /// Helper: create a renderer that writes to a Vec<u8> buffer for testing.
    fn test_renderer() -> (
        StreamingMarkdownRenderer,
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = TestWriter(buf.clone());
        let renderer = StreamingMarkdownRenderer::with_width(Box::new(writer), 80);
        (renderer, buf)
    }

    #[derive(Clone)]
    struct TestWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn get_output(buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    // --- push() buffering tests ---

    #[test]
    fn push_buffers_partial_lines() {
        let (mut r, buf) = test_renderer();
        r.push("hello ");
        // No newline yet — nothing rendered
        assert!(get_output(&buf).is_empty());
        r.push("world\n");
        // Now we have a complete line
        let out = get_output(&buf);
        assert!(out.contains("hello world"));
    }

    #[test]
    fn push_handles_multiple_lines_at_once() {
        let (mut r, buf) = test_renderer();
        r.push("line1\nline2\n");
        let out = get_output(&buf);
        assert!(out.contains("line1"));
        assert!(out.contains("line2"));
    }

    #[test]
    fn finish_flushes_remaining() {
        let (mut r, buf) = test_renderer();
        r.push("no newline");
        assert!(get_output(&buf).is_empty());
        r.finish();
        let out = get_output(&buf);
        assert!(out.contains("no newline"));
    }

    #[test]
    fn push_token_fragments() {
        let (mut r, buf) = test_renderer();
        r.push("Hel");
        r.push("lo ");
        r.push("Wor");
        r.push("ld!\n");
        let out = get_output(&buf);
        assert!(out.contains("Hello World!"));
    }

    // --- Heading rendering ---

    #[test]
    fn render_heading_h1() {
        let (mut r, buf) = test_renderer();
        r.push("# Title\n");
        let out = get_output(&buf);
        assert!(out.contains("#  Title"));
    }

    #[test]
    fn render_heading_h3() {
        let (mut r, buf) = test_renderer();
        r.push("### Sub heading\n");
        let out = get_output(&buf);
        assert!(out.contains("###  Sub heading"));
    }

    // --- Code block rendering ---

    #[test]
    fn render_code_block() {
        let (mut r, buf) = test_renderer();
        r.push("```rust\nlet x = 42;\n```\n");
        let out = get_output(&buf);
        // Opening fence
        assert!(out.contains("rust"));
        // Code line should be indented
        assert!(out.contains("  "));
        // Contains ANSI codes from highlighting
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn render_code_block_no_language() {
        let (mut r, buf) = test_renderer();
        r.push("```\nplain code\n```\n");
        let out = get_output(&buf);
        assert!(out.contains("plain code"));
    }

    #[test]
    fn code_block_across_pushes() {
        let (mut r, buf) = test_renderer();
        r.push("```py\n");
        r.push("x = 1\n");
        r.push("y = 2\n");
        r.push("```\n");
        let out = get_output(&buf);
        assert!(out.contains("py"));
    }

    // --- Inline formatting ---

    #[test]
    fn render_bold() {
        let (mut r, buf) = test_renderer();
        r.push("This is **bold** text\n");
        let out = get_output(&buf);
        assert!(out.contains("bold"));
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn render_italic() {
        let (mut r, buf) = test_renderer();
        r.push("This is *italic* text\n");
        let out = get_output(&buf);
        assert!(out.contains("italic"));
    }

    #[test]
    fn render_inline_code() {
        let (mut r, buf) = test_renderer();
        r.push("Use `cargo build` to compile\n");
        let out = get_output(&buf);
        assert!(out.contains("cargo build"));
    }

    #[test]
    fn render_link() {
        let (mut r, buf) = test_renderer();
        r.push("Visit [Rust](https://rust-lang.org) for more\n");
        let out = get_output(&buf);
        assert!(out.contains("Rust"));
        assert!(out.contains("https://rust-lang.org"));
    }

    // --- Block elements ---

    #[test]
    fn render_horizontal_rule() {
        let (mut r, buf) = test_renderer();
        r.push("---\n");
        let out = get_output(&buf);
        assert!(out.contains('─'));
    }

    #[test]
    fn render_blockquote() {
        let (mut r, buf) = test_renderer();
        r.push("> quoted text\n");
        let out = get_output(&buf);
        assert!(out.contains("│"));
        assert!(out.contains("quoted text"));
    }

    #[test]
    fn render_unordered_list() {
        let (mut r, buf) = test_renderer();
        r.push("- item one\n- item two\n");
        let out = get_output(&buf);
        assert!(out.contains('•'));
        assert!(out.contains("item one"));
        assert!(out.contains("item two"));
    }

    #[test]
    fn render_ordered_list() {
        let (mut r, buf) = test_renderer();
        r.push("1. first\n2. second\n");
        let out = get_output(&buf);
        assert!(out.contains("1"));
        assert!(out.contains("first"));
        assert!(out.contains("2"));
        assert!(out.contains("second"));
    }

    #[test]
    fn render_nested_list() {
        let (mut r, buf) = test_renderer();
        r.push("- top\n  - nested\n");
        let out = get_output(&buf);
        assert!(out.contains("top"));
        assert!(out.contains("nested"));
    }

    // --- Edge cases ---

    #[test]
    fn empty_push_is_noop() {
        let (mut r, buf) = test_renderer();
        r.push("");
        assert!(get_output(&buf).is_empty());
    }

    #[test]
    fn unmatched_bold_renders_literally() {
        let (mut r, buf) = test_renderer();
        r.push("this has **no closing\n");
        let out = get_output(&buf);
        assert!(out.contains("**no closing"));
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

    // --- Parsing helpers ---

    #[test]
    fn parse_heading_levels() {
        assert!(parse_heading("# H1").is_some());
        assert!(parse_heading("## H2").is_some());
        assert!(parse_heading("### H3").is_some());
        assert_eq!(parse_heading("# H1").unwrap().level, 1);
        assert_eq!(parse_heading("### H3").unwrap().level, 3);
        assert_eq!(parse_heading("### H3").unwrap().text, "H3");
    }

    #[test]
    fn parse_heading_rejects_non_headings() {
        assert!(parse_heading("not a heading").is_none());
        assert!(parse_heading("#nospace").is_none());
        assert!(parse_heading("####### too many").is_none());
    }

    #[test]
    fn horizontal_rule_detection() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("***"));
        assert!(is_horizontal_rule("___"));
        assert!(is_horizontal_rule("-----"));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("abc"));
    }

    #[test]
    fn parse_unordered_list_items() {
        assert_eq!(parse_unordered_list("- hello"), Some((0, "hello")));
        assert_eq!(parse_unordered_list("* hello"), Some((0, "hello")));
        assert_eq!(parse_unordered_list("  - nested"), Some((1, "nested")));
        assert!(parse_unordered_list("not a list").is_none());
    }

    #[test]
    fn parse_ordered_list_items() {
        assert_eq!(parse_ordered_list("1. first"), Some((0, 1, "first")));
        assert_eq!(parse_ordered_list("10. tenth"), Some((0, 10, "tenth")));
        assert_eq!(
            parse_ordered_list("  1. indented"),
            Some((1, 1, "indented"))
        );
        assert!(parse_ordered_list("not a list").is_none());
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

    #[test]
    fn finish_closes_open_code_block() {
        let (mut r, buf) = test_renderer();
        r.push("```rust\nlet x = 1;\n");
        // Don't close the code block — finish should handle it
        r.finish();
        let out = get_output(&buf);
        // Syntect splits tokens, so "let" and "x" may not be contiguous
        assert!(out.contains("let"));
        assert!(out.contains('\x1b')); // has ANSI styling
    }
}
