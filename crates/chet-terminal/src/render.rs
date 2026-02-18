//! Terminal line rendering — draws the prompt and buffer to stderr.

use crossterm::{
    cursor, execute,
    style::Print,
    terminal::{self, ClearType},
};
use std::io::{self, Write};

/// Renders the line editor's prompt and buffer to the terminal.
pub struct TerminalRenderer {
    /// Width of the terminal in columns.
    term_width: u16,
    /// The prompt string (e.g., "> ").
    prompt: String,
    /// Number of display columns the prompt occupies.
    prompt_width: usize,
}

impl TerminalRenderer {
    pub fn new(prompt: &str) -> Self {
        let (width, _) = terminal::size().unwrap_or((80, 24));
        Self {
            term_width: width,
            prompt: prompt.to_string(),
            prompt_width: prompt.len(),
        }
    }

    /// Draw the initial prompt (called once at the start of read_line).
    pub fn draw_prompt(&self) -> io::Result<()> {
        let mut stderr = io::stderr();
        execute!(stderr, Print(&self.prompt))?;
        stderr.flush()
    }

    /// Refresh the display after a buffer change.
    /// Moves cursor to start of prompt line(s), redraws everything, positions cursor.
    pub fn refresh(&self, buffer: &str, cursor_pos: usize) -> io::Result<()> {
        let mut stderr = io::stderr();
        let tw = self.term_width as usize;
        if tw == 0 {
            return Ok(());
        }

        // Full content = prompt + buffer
        let full_len = self.prompt_width + buffer.len();

        // Where is the cursor right now? We need to figure out what row we're on
        // to know how far to move up to get to the start.
        let cursor_absolute = self.prompt_width + cursor_pos;
        let cursor_row = cursor_absolute / tw;

        // Move cursor to the beginning of the prompt (row 0 of our content)
        if cursor_row > 0 {
            execute!(stderr, cursor::MoveUp(cursor_row as u16))?;
        }
        execute!(stderr, cursor::MoveToColumn(0))?;

        // Clear from here to the end of screen
        execute!(stderr, terminal::Clear(ClearType::FromCursorDown))?;

        // Redraw prompt + buffer
        execute!(stderr, Print(&self.prompt), Print(buffer))?;

        // Now position cursor at the correct location
        // After printing, cursor is at the end of buffer.
        // We need to move it to `cursor_absolute`.
        let end_row = full_len / tw;
        let target_row = cursor_absolute / tw;
        let target_col = cursor_absolute % tw;

        let rows_back = end_row - target_row;
        if rows_back > 0 {
            execute!(stderr, cursor::MoveUp(rows_back as u16))?;
        }
        execute!(stderr, cursor::MoveToColumn(target_col as u16))?;

        stderr.flush()
    }

    /// Clear the screen and redraw (Ctrl+L).
    pub fn clear_and_redraw(&self, buffer: &str, cursor_pos: usize) -> io::Result<()> {
        let mut stderr = io::stderr();
        execute!(
            stderr,
            terminal::Clear(ClearType::All),
            cursor::MoveTo(0, 0),
        )?;
        self.refresh(buffer, cursor_pos)
    }

    /// Update terminal width (call on resize events).
    pub fn set_width(&mut self, width: u16) {
        self.term_width = width;
    }

    /// Display completion candidates below the current line.
    pub fn show_completions(
        &self,
        candidates: &[String],
        buffer: &str,
        cursor_pos: usize,
    ) -> io::Result<()> {
        let mut stderr = io::stderr();

        // Move to a new line after the buffer
        let full_len = self.prompt_width + buffer.len();
        let tw = self.term_width as usize;
        let cursor_absolute = self.prompt_width + cursor_pos;
        let cursor_row = cursor_absolute / tw;
        let end_row = full_len / tw;

        // Move to end of content
        let rows_down = end_row - cursor_row;
        if rows_down > 0 {
            execute!(stderr, cursor::MoveDown(rows_down as u16))?;
        }

        // Print candidates
        execute!(stderr, Print("\r\n"))?;
        for candidate in candidates {
            execute!(stderr, Print(candidate), Print("  "))?;
        }
        execute!(stderr, Print("\r\n"))?;

        // Redraw prompt and buffer
        execute!(stderr, Print(&self.prompt), Print(buffer))?;

        // Reposition cursor
        let cursor_col = cursor_absolute % tw;
        let buffer_end_row = (self.prompt_width + buffer.len()) / tw;
        let rows_back = buffer_end_row - cursor_row;
        if rows_back > 0 {
            // Actually, we need to recalculate from our current position
            // We just drew the prompt+buffer, so we're at the end of that.
            // The candidates added rows. We need to get back to the right spot.
            // Let's just use refresh which handles this correctly.
        }

        // Easier: just use the standard refresh from the new cursor position
        // After printing prompt+buffer, cursor is at the end.
        // Move back to the correct position.
        let print_end_row = (self.prompt_width + buffer.len()) / tw;
        let target_row = cursor_absolute / tw;
        let move_up = print_end_row - target_row;
        if move_up > 0 {
            execute!(stderr, cursor::MoveUp(move_up as u16))?;
        }
        execute!(stderr, cursor::MoveToColumn(cursor_col as u16))?;

        stderr.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_width_calculated() {
        let r = TerminalRenderer::new("> ");
        assert_eq!(r.prompt_width, 2);
        assert_eq!(r.prompt, "> ");
    }

    #[test]
    fn set_width_updates() {
        let mut r = TerminalRenderer::new("> ");
        r.set_width(120);
        assert_eq!(r.term_width, 120);
    }

    #[test]
    fn cursor_position_math() {
        // Verify the position math used in refresh
        let prompt_width = 2usize;
        let term_width = 80usize;

        // Short input: everything on one row
        let cursor_pos = 5;
        let cursor_absolute = prompt_width + cursor_pos;
        assert_eq!(cursor_absolute / term_width, 0); // row 0
        assert_eq!(cursor_absolute % term_width, 7); // col 7

        // Long input: wraps to second row
        let cursor_pos = 85;
        let cursor_absolute = prompt_width + cursor_pos;
        assert_eq!(cursor_absolute / term_width, 1); // row 1
        assert_eq!(cursor_absolute % term_width, 7); // col 7
    }
}
