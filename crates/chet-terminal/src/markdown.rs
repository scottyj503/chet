//! Streaming markdown renderer — converts TextDelta chunks into styled terminal output.
//!
//! Text arrives as arbitrary token-level fragments. We buffer until we have complete
//! lines, then render each line with inline markdown styling and code block highlight.

use crate::highlight::CodeHighlighter;
use crate::inline::render_inline;
use crate::style;
use crate::table::{is_table_row, is_table_separator, render_table};
use crossterm::style::{Attribute, SetAttribute};
use crossterm::terminal;
use std::io::Write;

/// Tracks what block-level context we're currently in.
struct RenderState {
    in_code_block: bool,
    code_lang: String,
    table_buffer: Vec<String>,
    in_table: bool,
}

impl RenderState {
    fn new() -> Self {
        Self {
            in_code_block: false,
            code_lang: String::new(),
            table_buffer: Vec::new(),
            in_table: false,
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
    plain: bool,
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
            plain: false,
        }
    }

    /// Create a plain-mode renderer that passes through raw markdown without ANSI styling.
    ///
    /// Used when stdout is not a TTY (e.g., piped output in CI/CD).
    pub fn new_plain(writer: Box<dyn Write>) -> Self {
        Self {
            writer,
            buffer: String::new(),
            state: RenderState::new(),
            highlighter: CodeHighlighter::new(),
            term_width: 80,
            plain: true,
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
            plain: false,
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
            if self.plain {
                let _ = write!(self.writer, "{remaining}");
                let _ = self.writer.flush();
                return;
            }
            self.render_line(&remaining);
        }

        // Flush pending table
        self.flush_table();

        // If we're still in a code block at finish, close it
        if self.state.in_code_block {
            self.state.in_code_block = false;
            self.highlighter.end_block();
        }

        let _ = self.writer.flush();
    }

    /// Render a single complete line.
    fn render_line(&mut self, line: &str) {
        // Plain mode: pass through raw markdown without any styling
        if self.plain {
            let _ = writeln!(self.writer, "{line}");
            let _ = self.writer.flush();
            return;
        }

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

        // Table detection
        if is_table_row(trimmed) {
            if !self.state.in_table {
                // Start buffering potential table
                self.state.in_table = true;
                self.state.table_buffer.clear();
            }
            self.state.table_buffer.push(trimmed.to_string());

            // Check if we have header + separator (confirm table)
            // or if second line is NOT separator (flush as plain text)
            if self.state.table_buffer.len() == 2
                && !is_table_separator(&self.state.table_buffer[1])
            {
                // Not a table — flush as plain text
                let lines: Vec<String> = std::mem::take(&mut self.state.table_buffer);
                self.state.in_table = false;
                for l in &lines {
                    self.render_plain_line(l);
                }
            }
            let _ = self.writer.flush();
            return;
        }

        // Non-table line arrived while in table mode — flush the table
        if self.state.in_table {
            self.flush_table();
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
        self.render_plain_line(trimmed);
    }

    /// Render a line as normal paragraph text with inline markdown.
    fn render_plain_line(&mut self, line: &str) {
        let styled = render_inline(line);
        let _ = writeln!(self.writer, "{styled}");
        let _ = self.writer.flush();
    }

    /// Flush any buffered table lines, rendering them as a formatted table.
    fn flush_table(&mut self) {
        if self.state.table_buffer.is_empty() {
            self.state.in_table = false;
            return;
        }

        let lines = std::mem::take(&mut self.state.table_buffer);
        self.state.in_table = false;

        // Need at least header + separator (2 lines) for a valid table
        if lines.len() < 2 || !is_table_separator(&lines[1]) {
            // Render as plain text
            for line in &lines {
                self.render_plain_line(line);
            }
            return;
        }

        let rendered = render_table(&lines, self.term_width);
        let _ = write!(self.writer, "{rendered}");
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
        assert!(get_output(&buf).is_empty());
        r.push("world\n");
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
        assert!(out.contains("rust"));
        assert!(out.contains("  "));
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
    fn finish_closes_open_code_block() {
        let (mut r, buf) = test_renderer();
        r.push("```rust\nlet x = 1;\n");
        r.finish();
        let out = get_output(&buf);
        assert!(out.contains("let"));
        assert!(out.contains('\x1b'));
    }

    // --- Table integration with renderer ---

    #[test]
    fn table_followed_by_text_flushes() {
        let (mut r, buf) = test_renderer();
        r.push("| Col1 | Col2 |\n");
        r.push("|------|------|\n");
        r.push("| a    | b    |\n");
        r.push("Some text after\n");
        let out = get_output(&buf);
        assert!(out.contains('┌'));
        assert!(out.contains("Col1"));
        assert!(out.contains("Some text after"));
    }

    #[test]
    fn finish_flushes_pending_table() {
        let (mut r, buf) = test_renderer();
        r.push("| Col1 | Col2 |\n");
        r.push("|------|------|\n");
        r.push("| a    | b    |\n");
        r.finish();
        let out = get_output(&buf);
        assert!(out.contains('┌'));
        assert!(out.contains("Col1"));
        assert!(out.contains("a"));
    }

    #[test]
    fn pipe_lines_without_separator_render_as_text() {
        let (mut r, buf) = test_renderer();
        r.push("| not a table |\n");
        r.push("| just pipes |\n");
        let out = get_output(&buf);
        assert!(!out.contains('┌'));
        assert!(out.contains("not a table"));
        assert!(out.contains("just pipes"));
    }

    #[test]
    fn single_pipe_line_then_text_renders_both() {
        let (mut r, buf) = test_renderer();
        r.push("| header |\n");
        r.push("normal text\n");
        let out = get_output(&buf);
        assert!(out.contains("header"));
        assert!(out.contains("normal text"));
        assert!(!out.contains('┌'));
    }

    // --- Plain mode tests ---

    fn test_plain_renderer() -> (
        StreamingMarkdownRenderer,
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = TestWriter(buf.clone());
        let renderer = StreamingMarkdownRenderer::new_plain(Box::new(writer));
        (renderer, buf)
    }

    #[test]
    fn plain_mode_no_ansi_in_headings() {
        let (mut r, buf) = test_plain_renderer();
        r.push("# Title\n");
        r.push("**bold text**\n");
        let out = get_output(&buf);
        assert!(out.contains("# Title"));
        assert!(out.contains("**bold text**"));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn plain_mode_no_ansi_in_code_blocks() {
        let (mut r, buf) = test_plain_renderer();
        r.push("```rust\nlet x = 42;\n```\n");
        let out = get_output(&buf);
        assert!(out.contains("```rust"));
        assert!(out.contains("let x = 42;"));
        assert!(out.contains("```"));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn plain_mode_finish_flushes_without_ansi() {
        let (mut r, buf) = test_plain_renderer();
        r.push("no newline");
        r.finish();
        let out = get_output(&buf);
        assert!(out.contains("no newline"));
        assert!(!out.contains('\x1b'));
    }
}
