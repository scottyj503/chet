//! Line editor — the main entry point for interactive input.

use crate::buffer::LineBuffer;
use crate::completion::Completer;
use crate::history::History;
use crate::keys::{EditorAction, map_key};
use crate::render::TerminalRenderer;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal;
use std::io;
use std::path::PathBuf;

/// Result of a `read_line` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadLineResult {
    /// User pressed Enter — the line contents (untrimmed).
    Line(String),
    /// User pressed Ctrl+D on an empty line.
    Eof,
    /// User pressed Ctrl+C.
    Interrupted,
}

/// RAII guard that disables raw mode on drop.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Interactive line editor with history and tab completion.
pub struct LineEditor {
    history: History,
    completer: Option<Box<dyn Completer>>,
    history_loaded: bool,
}

impl LineEditor {
    /// Create a new line editor that stores history at `history_path`.
    pub fn new(history_path: PathBuf) -> Self {
        Self {
            history: History::new(history_path),
            completer: None,
            history_loaded: false,
        }
    }

    /// Set the completer for tab completion.
    pub fn set_completer(&mut self, completer: Box<dyn Completer>) {
        self.completer = Some(completer);
    }

    /// Read a line of input from the user, displaying `prompt`.
    ///
    /// Enters raw mode, reads key events, handles editing/history/completion.
    /// Raw mode is always restored when this returns (via RAII guard).
    pub async fn read_line(&mut self, prompt: &str) -> io::Result<ReadLineResult> {
        // Lazy-load history on first call
        if !self.history_loaded {
            self.history.load();
            self.history_loaded = true;
        }

        // Move owned data into the blocking closure, get it back when done.
        let mut history = std::mem::replace(&mut self.history, History::new(PathBuf::new()));
        let completer = self.completer.take();
        let prompt = prompt.to_string();

        let (result, returned_history, returned_completer) =
            tokio::task::spawn_blocking(move || {
                let result = read_line_sync(&prompt, &mut history, completer.as_deref());
                (result, history, completer)
            })
            .await
            .map_err(io::Error::other)?;

        // Restore owned data
        self.history = returned_history;
        self.completer = returned_completer;

        result
    }

    /// Save history to disk.
    pub fn save_history(&self) -> io::Result<()> {
        self.history.save()
    }
}

/// Synchronous read loop — runs inside `spawn_blocking`.
fn read_line_sync(
    prompt: &str,
    history: &mut History,
    completer: Option<&dyn Completer>,
) -> io::Result<ReadLineResult> {
    let _guard = RawModeGuard::enable()?;
    let mut renderer = TerminalRenderer::new(prompt);
    let mut buffer = LineBuffer::new();

    renderer.draw_prompt()?;

    loop {
        let ev = event::read()?;

        match ev {
            Event::Key(key_event) => {
                // crossterm sends Release/Repeat events on some platforms
                if key_event.kind != KeyEventKind::Press {
                    continue;
                }

                match map_key(key_event) {
                    EditorAction::Insert(ch) => {
                        buffer.insert(ch);
                        history.reset_navigation();
                        renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                    }
                    EditorAction::Backspace => {
                        if buffer.delete_backward() {
                            history.reset_navigation();
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::Delete => {
                        if buffer.delete_forward() {
                            history.reset_navigation();
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::MoveLeft => {
                        if buffer.move_left() {
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::MoveRight => {
                        if buffer.move_right() {
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::MoveWordLeft => {
                        if buffer.move_word_left() {
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::MoveWordRight => {
                        if buffer.move_word_right() {
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::Home => {
                        buffer.move_home();
                        renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                    }
                    EditorAction::End => {
                        buffer.move_end();
                        renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                    }
                    EditorAction::KillToEnd => {
                        if buffer.delete_to_end() {
                            history.reset_navigation();
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::KillToStart => {
                        if buffer.delete_to_start() {
                            history.reset_navigation();
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::DeleteWord => {
                        if buffer.delete_word_backward() {
                            history.reset_navigation();
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::HistoryUp => {
                        let current = buffer.as_str();
                        if let Some(entry) = history.navigate_up(&current) {
                            buffer.set(entry);
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::HistoryDown => {
                        if let Some(entry) = history.navigate_down() {
                            buffer.set(entry);
                            renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                        }
                    }
                    EditorAction::Complete => {
                        if let Some(completer) = completer {
                            let buf_str = buffer.as_str();
                            let candidates = completer.complete(&buf_str, buffer.cursor());
                            match candidates.len() {
                                0 => {}
                                1 => {
                                    buffer.set(&candidates[0]);
                                    renderer.refresh(&buffer.as_str(), buffer.cursor())?;
                                }
                                _ => {
                                    renderer.show_completions(
                                        &candidates,
                                        &buffer.as_str(),
                                        buffer.cursor(),
                                    )?;
                                }
                            }
                        }
                    }
                    EditorAction::Submit => {
                        use crossterm::{execute, style::Print};
                        execute!(io::stderr(), Print("\r\n"))?;

                        let line = buffer.as_str();
                        history.add(&line);
                        history.reset_navigation();
                        return Ok(ReadLineResult::Line(line));
                    }
                    EditorAction::Cancel => {
                        use crossterm::{execute, style::Print};
                        execute!(io::stderr(), Print("^C\r\n"))?;
                        history.reset_navigation();
                        return Ok(ReadLineResult::Interrupted);
                    }
                    EditorAction::Eof => {
                        if buffer.is_empty() {
                            return Ok(ReadLineResult::Eof);
                        }
                        // Non-empty buffer: ignore Ctrl+D
                    }
                    EditorAction::ClearScreen => {
                        renderer.clear_and_redraw(&buffer.as_str(), buffer.cursor())?;
                    }
                    EditorAction::Noop => {}
                }
            }
            Event::Resize(width, _height) => {
                renderer.set_width(width);
                renderer.refresh(&buffer.as_str(), buffer.cursor())?;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_mode_guard_is_send() {
        // Verify RawModeGuard can exist in a Send context
        fn assert_send<T: Send>() {}
        assert_send::<RawModeGuard>();
    }

    #[test]
    fn read_line_result_variants() {
        let line = ReadLineResult::Line("hello".to_string());
        assert_eq!(line, ReadLineResult::Line("hello".to_string()));
        assert_ne!(line, ReadLineResult::Eof);
        assert_ne!(line, ReadLineResult::Interrupted);
    }
}
