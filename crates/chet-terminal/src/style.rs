//! Terminal style helpers using crossterm ANSI escape sequences.

use crossterm::style::{Attribute, Color, SetAttribute, SetForegroundColor};
use std::fmt::Write;

/// Wrap text in bold.
pub fn bold(text: &str) -> String {
    format!(
        "{}{}{}",
        SetAttribute(Attribute::Bold),
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Wrap text in dim (faint).
pub fn dim(text: &str) -> String {
    format!(
        "{}{}{}",
        SetAttribute(Attribute::Dim),
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Wrap text in italic.
pub fn italic(text: &str) -> String {
    format!(
        "{}{}{}",
        SetAttribute(Attribute::Italic),
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Wrap text in underline.
pub fn underline(text: &str) -> String {
    format!(
        "{}{}{}",
        SetAttribute(Attribute::Underlined),
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Wrap text in a foreground color.
pub fn fg_color(text: &str, color: Color) -> String {
    format!(
        "{}{}{}",
        SetForegroundColor(color),
        text,
        SetForegroundColor(Color::Reset)
    )
}

/// Render inline code span: dim + colored.
pub fn code_span(text: &str) -> String {
    format!(
        "{}{}{}{}",
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Dim),
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Render a heading with bold + color based on level.
pub fn heading(text: &str, level: u8) -> String {
    let color = match level {
        1 => Color::Green,
        2 => Color::Blue,
        3 => Color::Magenta,
        _ => Color::Yellow,
    };
    let prefix = "#".repeat(level as usize);
    format!(
        "{}{}{}  {}{}",
        SetForegroundColor(color),
        SetAttribute(Attribute::Bold),
        prefix,
        text,
        SetAttribute(Attribute::Reset)
    )
}

/// Render a link: underlined text + dim URL.
pub fn link(text: &str, url: &str) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{}{}{}",
        SetAttribute(Attribute::Underlined),
        text,
        SetAttribute(Attribute::Reset)
    );
    let _ = write!(
        out,
        " ({}{}{})",
        SetAttribute(Attribute::Dim),
        url,
        SetAttribute(Attribute::Reset)
    );
    out
}

/// Render a horizontal rule spanning the given width.
pub fn horizontal_rule(width: u16) -> String {
    let w = width.min(80) as usize;
    format!(
        "{}{}{}",
        SetAttribute(Attribute::Dim),
        "─".repeat(w),
        SetAttribute(Attribute::Reset)
    )
}

/// Render a blockquote prefix (dim bar + space).
pub fn blockquote_prefix() -> String {
    format!(
        "{}│ {}",
        SetAttribute(Attribute::Dim),
        SetAttribute(Attribute::Reset)
    )
}

/// Render a list bullet for the given depth.
pub fn list_bullet(depth: u8) -> String {
    let indent = "  ".repeat(depth as usize);
    format!(
        "{}{}•{}",
        indent,
        SetForegroundColor(Color::DarkYellow),
        SetForegroundColor(Color::Reset)
    )
}

/// Render an ordered list number.
pub fn list_number(n: u32, depth: u8) -> String {
    let indent = "  ".repeat(depth as usize);
    format!(
        "{}{}{}{}.",
        indent,
        SetForegroundColor(Color::DarkYellow),
        n,
        SetForegroundColor(Color::Reset)
    )
}

// ---------------------------------------------------------------------------
// Tool event styling
// ---------------------------------------------------------------------------

/// Format a tool start event: "  ⚡ name" (cyan icon, bold name).
pub fn tool_start(name: &str) -> String {
    format!(
        "  {}⚡{} {}{}{}",
        SetForegroundColor(Color::Cyan),
        SetForegroundColor(Color::Reset),
        SetAttribute(Attribute::Bold),
        name,
        SetAttribute(Attribute::Reset),
    )
}

/// Format a tool success event: "  ✓ name output" (green icon).
pub fn tool_success(name: &str, output: &str) -> String {
    let mut s = format!(
        "  {}✓{} {}{}{}",
        SetForegroundColor(Color::Green),
        SetForegroundColor(Color::Reset),
        SetAttribute(Attribute::Bold),
        name,
        SetAttribute(Attribute::Reset),
    );
    if !output.is_empty() {
        let _ = write!(
            s,
            " {}{}{}",
            SetAttribute(Attribute::Dim),
            output,
            SetAttribute(Attribute::Reset),
        );
    }
    s
}

/// Format a tool error event: "  ✗ name output" (red icon).
pub fn tool_error(name: &str, output: &str) -> String {
    let mut s = format!(
        "  {}✗{} {}{}{}",
        SetForegroundColor(Color::Red),
        SetForegroundColor(Color::Reset),
        SetAttribute(Attribute::Bold),
        name,
        SetAttribute(Attribute::Reset),
    );
    if !output.is_empty() {
        let _ = write!(
            s,
            " {}{}{}",
            SetForegroundColor(Color::Red),
            output,
            SetForegroundColor(Color::Reset),
        );
    }
    s
}

/// Format a tool blocked event: "  ⊘ name reason" (yellow icon).
pub fn tool_blocked(name: &str, reason: &str) -> String {
    let mut s = format!(
        "  {}⊘{} {}{}{}",
        SetForegroundColor(Color::Yellow),
        SetForegroundColor(Color::Reset),
        SetAttribute(Attribute::Bold),
        name,
        SetAttribute(Attribute::Reset),
    );
    if !reason.is_empty() {
        let _ = write!(
            s,
            " {}{}{}",
            SetForegroundColor(Color::Yellow),
            reason,
            SetForegroundColor(Color::Reset),
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_wraps_text() {
        let result = bold("hello");
        assert!(result.contains("hello"));
        // Should contain bold attribute escape sequences
        assert!(result.starts_with('\x1b'));
    }

    #[test]
    fn dim_wraps_text() {
        let result = dim("faint");
        assert!(result.contains("faint"));
        assert!(result.starts_with('\x1b'));
    }

    #[test]
    fn italic_wraps_text() {
        let result = italic("slant");
        assert!(result.contains("slant"));
        assert!(result.starts_with('\x1b'));
    }

    #[test]
    fn underline_wraps_text() {
        let result = underline("line");
        assert!(result.contains("line"));
        assert!(result.starts_with('\x1b'));
    }

    #[test]
    fn code_span_contains_text() {
        let result = code_span("x + 1");
        assert!(result.contains("x + 1"));
    }

    #[test]
    fn heading_includes_hashes_and_text() {
        let h1 = heading("Title", 1);
        assert!(h1.contains("#  Title"));
        let h3 = heading("Sub", 3);
        assert!(h3.contains("###  Sub"));
    }

    #[test]
    fn link_shows_text_and_url() {
        let result = link("click", "https://example.com");
        assert!(result.contains("click"));
        assert!(result.contains("https://example.com"));
    }

    #[test]
    fn horizontal_rule_has_correct_width() {
        let hr = horizontal_rule(40);
        // Count the ─ characters (3 bytes each in UTF-8)
        let dashes: usize = hr.chars().filter(|&c| c == '─').count();
        assert_eq!(dashes, 40);
    }

    #[test]
    fn list_bullet_has_indent() {
        let b0 = list_bullet(0);
        assert!(b0.contains('•'));
        let b1 = list_bullet(1);
        assert!(b1.contains("  "));
        assert!(b1.contains('•'));
    }

    #[test]
    fn list_number_formats_correctly() {
        let n = list_number(3, 0);
        assert!(n.contains("3"));
        assert!(n.contains('.'));
    }

    // --- Tool styling tests ---

    #[test]
    fn tool_start_contains_icon_and_name() {
        let result = tool_start("Bash");
        assert!(result.contains('⚡'));
        assert!(result.contains("Bash"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn tool_success_contains_icon_name_output() {
        let result = tool_success("Read", "42 lines");
        assert!(result.contains('✓'));
        assert!(result.contains("Read"));
        assert!(result.contains("42 lines"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn tool_success_no_output() {
        let result = tool_success("Write", "");
        assert!(result.contains('✓'));
        assert!(result.contains("Write"));
    }

    #[test]
    fn tool_error_contains_icon_name_output() {
        let result = tool_error("Bash", "exit code 1");
        assert!(result.contains('✗'));
        assert!(result.contains("Bash"));
        assert!(result.contains("exit code 1"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn tool_error_no_output() {
        let result = tool_error("Bash", "");
        assert!(result.contains('✗'));
        assert!(result.contains("Bash"));
    }

    #[test]
    fn tool_blocked_contains_icon_name_reason() {
        let result = tool_blocked("Write", "not permitted");
        assert!(result.contains('⊘'));
        assert!(result.contains("Write"));
        assert!(result.contains("not permitted"));
        assert!(result.contains('\x1b'));
    }

    #[test]
    fn tool_blocked_no_reason() {
        let result = tool_blocked("Write", "");
        assert!(result.contains('⊘'));
        assert!(result.contains("Write"));
    }

    #[test]
    fn tool_styling_has_leading_indent() {
        assert!(tool_start("Bash").contains("  "));
        assert!(tool_success("Bash", "ok").contains("  "));
        assert!(tool_error("Bash", "fail").contains("  "));
        assert!(tool_blocked("Bash", "no").contains("  "));
    }
}
