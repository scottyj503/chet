//! Table parsing and rendering with box-drawing characters.

use crate::inline::render_inline;
use std::fmt::Write;

/// Column alignment parsed from the separator row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Alignment {
    Left,
    Center,
    Right,
}

/// Check if a line looks like a table row (starts and ends with `|`).
pub(crate) fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 3
}

/// Check if a line is a table separator row (e.g. `|---|:---:|---:|`).
pub(crate) fn is_table_separator(line: &str) -> bool {
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
pub(crate) fn render_table(lines: &[String], _term_width: u16) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(result.contains("Left"));
        assert!(result.contains("Right"));
        assert!(result.contains("Center"));
    }
}
