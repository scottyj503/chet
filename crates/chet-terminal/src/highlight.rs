//! Code block syntax highlighting via syntect.

use crossterm::style::{Attribute, Color, SetAttribute, SetForegroundColor};
use std::fmt::Write;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, ThemeSet};
use syntect::parsing::SyntaxSet;

/// Wraps syntect for line-by-line syntax highlighting of code blocks.
pub struct CodeHighlighter {
    // Box for stable heap addresses that won't move when we take &mut self.
    syntax_set: Box<SyntaxSet>,
    theme_set: Box<ThemeSet>,
    state: Option<HighlightState>,
}

struct HighlightState {
    highlighter: HighlightLines<'static>,
    // Raw pointer to the boxed SyntaxSet so highlight_line can reference it.
    syntax_set_ptr: *const SyntaxSet,
}

// Safety: SyntaxSet, ThemeSet, and HighlightLines contain only parsed grammar
// data — no thread-local state. The raw pointer points to our own Box.
unsafe impl Send for HighlightState {}

impl Default for CodeHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeHighlighter {
    pub fn new() -> Self {
        Self {
            syntax_set: Box::new(SyntaxSet::load_defaults_newlines()),
            theme_set: Box::new(ThemeSet::load_defaults()),
            state: None,
        }
    }

    /// Begin a new code block. `language` should be the language name/extension
    /// from the fenced code block (e.g. "rust", "py", "js").
    pub fn start_block(&mut self, language: &str) {
        // Safety: Both syntax_set and theme_set are in Boxes with stable heap
        // addresses. We extend their lifetimes to 'static via raw pointers.
        // This is safe because:
        // 1. The Boxes outlive `state` (state is cleared in end_block/finish).
        // 2. Field drop order is declaration order, so state drops before
        //    syntax_set and theme_set.
        let ss_ptr: *const SyntaxSet = &*self.syntax_set;
        let ts_ptr: *const ThemeSet = &*self.theme_set;
        let highlighter = unsafe {
            let ss: &'static SyntaxSet = &*ss_ptr;
            let ts: &'static ThemeSet = &*ts_ptr;
            let syntax = ss
                .find_syntax_by_token(language)
                .unwrap_or_else(|| ss.find_syntax_plain_text());
            let theme = &ts.themes["base16-ocean.dark"];
            HighlightLines::new(syntax, theme)
        };

        self.state = Some(HighlightState {
            highlighter,
            syntax_set_ptr: ss_ptr,
        });
    }

    /// Highlight a single line of code. Returns the styled string with ANSI escapes.
    /// Must be called between `start_block()` and `end_block()`.
    pub fn highlight_line(&mut self, line: &str) -> String {
        let Some(state) = &mut self.state else {
            // No active block — return as plain dim text
            return format!(
                "{}{}{}",
                SetAttribute(Attribute::Dim),
                line,
                SetAttribute(Attribute::Reset)
            );
        };

        // syntect expects lines with endings for proper state tracking
        let line_with_ending = if line.ends_with('\n') {
            line.to_string()
        } else {
            format!("{line}\n")
        };

        // Safety: syntax_set is still alive in our Box
        let syntax_set_ref = unsafe { &*state.syntax_set_ptr };

        match state
            .highlighter
            .highlight_line(&line_with_ending, syntax_set_ref)
        {
            Ok(ranges) => syntect_to_ansi(&ranges),
            Err(_) => line.to_string(),
        }
    }

    /// End the current code block, dropping highlighter state.
    pub fn end_block(&mut self) {
        self.state = None;
    }

    /// Whether we're currently inside a code block.
    pub fn is_active(&self) -> bool {
        self.state.is_some()
    }
}

/// Convert syntect highlight ranges to ANSI escape sequences.
fn syntect_to_ansi(ranges: &[(Style, &str)]) -> String {
    let mut out = String::new();
    for (style, text) in ranges {
        // Skip trailing newline — we handle line endings ourselves
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            continue;
        }

        let fg = style.foreground;
        let _ = write!(
            out,
            "{}",
            SetForegroundColor(Color::Rgb {
                r: fg.r,
                g: fg.g,
                b: fg.b,
            })
        );

        if style.font_style.contains(FontStyle::BOLD) {
            let _ = write!(out, "{}", SetAttribute(Attribute::Bold));
        }
        if style.font_style.contains(FontStyle::ITALIC) {
            let _ = write!(out, "{}", SetAttribute(Attribute::Italic));
        }
        if style.font_style.contains(FontStyle::UNDERLINE) {
            let _ = write!(out, "{}", SetAttribute(Attribute::Underlined));
        }

        let _ = write!(out, "{text}");
        let _ = write!(out, "{}", SetAttribute(Attribute::Reset));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust_code() {
        let mut h = CodeHighlighter::new();
        h.start_block("rust");
        let result = h.highlight_line("let x = 42;");
        assert!(!result.is_empty());
        // Should contain ANSI escape codes
        assert!(result.contains('\x1b'));
        h.end_block();
    }

    #[test]
    fn highlight_unknown_language_falls_back() {
        let mut h = CodeHighlighter::new();
        h.start_block("nonexistent_language_xyz");
        let result = h.highlight_line("hello world");
        assert!(result.contains("hello world"));
        h.end_block();
    }

    #[test]
    fn highlight_without_block_returns_dim() {
        let mut h = CodeHighlighter::new();
        let result = h.highlight_line("plain text");
        assert!(result.contains("plain text"));
    }

    #[test]
    fn is_active_tracks_state() {
        let mut h = CodeHighlighter::new();
        assert!(!h.is_active());
        h.start_block("rust");
        assert!(h.is_active());
        h.end_block();
        assert!(!h.is_active());
    }

    #[test]
    fn multiple_lines_maintain_state() {
        let mut h = CodeHighlighter::new();
        h.start_block("rust");
        let line1 = h.highlight_line("fn main() {");
        let line2 = h.highlight_line("    println!(\"hi\");");
        let line3 = h.highlight_line("}");
        // All should have ANSI escapes
        assert!(line1.contains('\x1b'));
        assert!(line2.contains('\x1b'));
        assert!(line3.contains('\x1b'));
        h.end_block();
    }

    #[test]
    fn syntect_to_ansi_produces_output() {
        let result = syntect_to_ansi(&[]);
        assert!(result.is_empty());
    }
}
