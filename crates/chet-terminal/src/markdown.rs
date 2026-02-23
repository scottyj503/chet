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

// ---------------------------------------------------------------------------
// Table parsing and rendering helpers
// ---------------------------------------------------------------------------

/// Column alignment parsed from the separator row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Alignment {
    Left,
    Center,
    Right,
}

/// Check if a line looks like a table row (starts and ends with `|`).
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 3
}

/// Check if a line is a table separator row (e.g. `|---|:---:|---:|`).
fn is_table_separator(line: &str) -> bool {
    if !is_table_row(line) {
        return false;
    }
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    if inner.is_empty() {
        return false;
    }
    inner.split('|').all(|cell| {
        let cell = cell.trim();
        if cell.is_empty() {
            return false;
        }
        cell.chars().all(|c| c == '-' || c == ':' || c == ' ') && cell.contains('-')
    })
}

/// Parse column alignments from a separator row.
fn parse_alignments(separator: &str) -> Vec<Alignment> {
    let inner = separator
        .trim()
        .trim_start_matches('|')
        .trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| {
            let cell = cell.trim();
            let left = cell.starts_with(':');
            let right = cell.ends_with(':');
            match (left, right) {
                (true, true) => Alignment::Center,
                (false, true) => Alignment::Right,
                _ => Alignment::Left,
            }
        })
        .collect()
}

/// Parse a table row into cells (trimmed).
fn parse_table_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Render a complete table with box-drawing characters.
///
/// `lines` must have at least 2 entries: header row + separator row.
/// Additional data rows follow.
fn render_table(lines: &[String], _term_width: u16) -> String {
    if lines.len() < 2 {
        return lines.join("\n") + "\n";
    }

    let alignments = parse_alignments(&lines[1]);
    let header_cells = parse_table_row(&lines[0]);
    let data_rows: Vec<Vec<String>> = lines[2..].iter().map(|l| parse_table_row(l)).collect();

    let num_cols = header_cells.len();

    // Compute max column widths
    let mut widths = vec![0usize; num_cols];
    for (i, cell) in header_cells.iter().enumerate() {
        if i < num_cols {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in &data_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }
    // Ensure minimum column width of 1
    for w in &mut widths {
        *w = (*w).max(1);
    }

    let mut out = String::new();

    // Top border: ┌──────┬──────┐
    out.push('┌');
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        if i < num_cols - 1 {
            out.push('┬');
        }
    }
    out.push('┐');
    out.push('\n');

    // Header row: │ Col1 │ Col2 │
    out.push('│');
    for (i, cell) in header_cells.iter().enumerate() {
        let w = if i < num_cols { widths[i] } else { cell.len() };
        let align = alignments.get(i).copied().unwrap_or(Alignment::Left);
        let styled = render_inline(cell);
        let padded = pad_cell(&styled, cell.len(), w, align);
        let _ = write!(out, " {padded} ");
        if i < num_cols - 1 || i == header_cells.len() - 1 {
            out.push('│');
        }
    }
    out.push('\n');

    // Header separator: ├──────┼──────┤
    out.push('├');
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        if i < num_cols - 1 {
            out.push('┼');
        }
    }
    out.push('┤');
    out.push('\n');

    // Data rows: │ a    │ b    │
    for row in &data_rows {
        out.push('│');
        for (i, w) in widths.iter().enumerate().take(num_cols) {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let align = alignments.get(i).copied().unwrap_or(Alignment::Left);
            let styled = render_inline(cell);
            let padded = pad_cell(&styled, cell.len(), *w, align);
            let _ = write!(out, " {padded} ");
            out.push('│');
        }
        out.push('\n');
    }

    // Bottom border: └──────┴──────┘
    out.push('└');
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        if i < num_cols - 1 {
            out.push('┴');
        }
    }
    out.push('┘');
    out.push('\n');

    out
}

/// Pad a styled cell string to the target width, respecting alignment.
///
/// `visible_len` is the length of the raw (unstyled) cell text.
/// `target_width` is the column width to pad to.
fn pad_cell(styled: &str, visible_len: usize, target_width: usize, align: Alignment) -> String {
    if visible_len >= target_width {
        return styled.to_string();
    }
    let padding = target_width - visible_len;
    match align {
        Alignment::Left => format!("{styled}{}", " ".repeat(padding)),
        Alignment::Right => format!("{}{styled}", " ".repeat(padding)),
        Alignment::Center => {
            let left = padding / 2;
            let right = padding - left;
            format!("{}{styled}{}", " ".repeat(left), " ".repeat(right))
        }
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

    // --- Table detection helpers ---

    #[test]
    fn is_table_row_detects_pipe_lines() {
        assert!(is_table_row("| a | b |"));
        assert!(is_table_row("|---|---|"));
        assert!(is_table_row("| col1 | col2 | col3 |"));
        assert!(!is_table_row("not a table"));
        assert!(!is_table_row("| only start"));
        assert!(!is_table_row("only end |"));
        assert!(!is_table_row("||")); // too short
    }

    #[test]
    fn is_table_separator_detects_separator_rows() {
        assert!(is_table_separator("|---|---|"));
        assert!(is_table_separator("| --- | --- |"));
        assert!(is_table_separator("|:---|---:|"));
        assert!(is_table_separator("|:---:|:---:|"));
        assert!(is_table_separator("| :---: | :---: |"));
        assert!(!is_table_separator("| a | b |"));
        assert!(!is_table_separator("not a row"));
        // Single-column separator is valid
        assert!(is_table_separator("| -- |"));
    }

    #[test]
    fn parse_alignments_extracts_alignment() {
        let aligns = parse_alignments("|---|:---:|---:|");
        assert_eq!(
            aligns,
            vec![Alignment::Left, Alignment::Center, Alignment::Right]
        );
    }

    #[test]
    fn parse_alignments_default_left() {
        let aligns = parse_alignments("|---|---|");
        assert_eq!(aligns, vec![Alignment::Left, Alignment::Left]);
    }

    #[test]
    fn parse_table_row_splits_cells() {
        let cells = parse_table_row("| hello | world |");
        assert_eq!(cells, vec!["hello", "world"]);
    }

    #[test]
    fn parse_table_row_trims_whitespace() {
        let cells = parse_table_row("|  spaces  |  here  |");
        assert_eq!(cells, vec!["spaces", "here"]);
    }

    // --- Table rendering ---

    #[test]
    fn render_table_produces_box_drawing() {
        let lines = vec![
            "| Col1 | Col2 |".to_string(),
            "|------|------|".to_string(),
            "| a    | b    |".to_string(),
        ];
        let result = render_table(&lines, 80);
        assert!(result.contains('┌'));
        assert!(result.contains('┐'));
        assert!(result.contains('├'));
        assert!(result.contains('┤'));
        assert!(result.contains('└'));
        assert!(result.contains('┘'));
        assert!(result.contains('│'));
        assert!(result.contains('─'));
        assert!(result.contains("Col1"));
        assert!(result.contains("Col2"));
        assert!(result.contains("a"));
        assert!(result.contains("b"));
    }

    #[test]
    fn render_table_alignment() {
        let lines = vec![
            "| Left | Right | Center |".to_string(),
            "|:-----|------:|:------:|".to_string(),
            "| a    | b     | c      |".to_string(),
        ];
        let result = render_table(&lines, 80);
        // Just verify it renders without panicking and has content
        assert!(result.contains("Left"));
        assert!(result.contains("Right"));
        assert!(result.contains("Center"));
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
        // Table should have box-drawing chars
        assert!(out.contains('┌'));
        assert!(out.contains("Col1"));
        // Text after table should also appear
        assert!(out.contains("Some text after"));
    }

    #[test]
    fn finish_flushes_pending_table() {
        let (mut r, buf) = test_renderer();
        r.push("| Col1 | Col2 |\n");
        r.push("|------|------|\n");
        r.push("| a    | b    |\n");
        // Don't send a non-table line, just finish
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
        // Should NOT contain box-drawing chars
        assert!(!out.contains('┌'));
        assert!(out.contains("not a table"));
        assert!(out.contains("just pipes"));
    }

    #[test]
    fn single_pipe_line_then_text_renders_both() {
        let (mut r, buf) = test_renderer();
        r.push("| header |\n");
        // Next line is not a table row, so flush as text
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
