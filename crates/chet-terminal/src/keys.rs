//! Key event mapping — translates crossterm KeyEvents to EditorActions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Actions the line editor can perform in response to key input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    /// Insert a character at the cursor position.
    Insert(char),
    /// Delete the character before the cursor (Backspace).
    Backspace,
    /// Delete the character at the cursor (Delete).
    Delete,
    /// Move cursor one position left.
    MoveLeft,
    /// Move cursor one position right.
    MoveRight,
    /// Move cursor one word left (Ctrl+Left).
    MoveWordLeft,
    /// Move cursor one word right (Ctrl+Right).
    MoveWordRight,
    /// Move cursor to start of line (Home / Ctrl+A).
    Home,
    /// Move cursor to end of line (End / Ctrl+E).
    End,
    /// Navigate to previous history entry.
    HistoryUp,
    /// Navigate to next history entry.
    HistoryDown,
    /// Trigger tab completion.
    Complete,
    /// Submit the current line (Enter).
    Submit,
    /// Cancel the current line (Ctrl+C).
    Cancel,
    /// End of input (Ctrl+D).
    Eof,
    /// Kill text from cursor to end of line (Ctrl+K).
    KillToEnd,
    /// Kill text from start of line to cursor (Ctrl+U).
    KillToStart,
    /// Delete the word before the cursor (Ctrl+W).
    DeleteWord,
    /// Clear screen (Ctrl+L).
    ClearScreen,
    /// No action — ignore this key event.
    Noop,
}

/// Map a crossterm `KeyEvent` to an `EditorAction`.
pub fn map_key(event: KeyEvent) -> EditorAction {
    let mods = event.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);

    match event.code {
        // Ctrl key combos
        KeyCode::Char('a') if ctrl => EditorAction::Home,
        KeyCode::Char('e') if ctrl => EditorAction::End,
        KeyCode::Char('k') if ctrl => EditorAction::KillToEnd,
        KeyCode::Char('u') if ctrl => EditorAction::KillToStart,
        KeyCode::Char('w') if ctrl => EditorAction::DeleteWord,
        KeyCode::Char('c') if ctrl => EditorAction::Cancel,
        KeyCode::Char('d') if ctrl => EditorAction::Eof,
        KeyCode::Char('l') if ctrl => EditorAction::ClearScreen,
        KeyCode::Char('b') if ctrl => EditorAction::MoveLeft,
        KeyCode::Char('f') if ctrl => EditorAction::MoveRight,
        KeyCode::Char('p') if ctrl => EditorAction::HistoryUp,
        KeyCode::Char('n') if ctrl => EditorAction::HistoryDown,
        KeyCode::Char('h') if ctrl => EditorAction::Backspace,

        // Alt+arrow word movement
        KeyCode::Left if alt => EditorAction::MoveWordLeft,
        KeyCode::Right if alt => EditorAction::MoveWordRight,

        // Alt+b / Alt+f word movement (emacs style)
        KeyCode::Char('b') if alt => EditorAction::MoveWordLeft,
        KeyCode::Char('f') if alt => EditorAction::MoveWordRight,

        // Basic character input
        KeyCode::Char(c) => EditorAction::Insert(c),

        // Navigation keys
        KeyCode::Left if ctrl => EditorAction::MoveWordLeft,
        KeyCode::Right if ctrl => EditorAction::MoveWordRight,
        KeyCode::Left => EditorAction::MoveLeft,
        KeyCode::Right => EditorAction::MoveRight,
        KeyCode::Home => EditorAction::Home,
        KeyCode::End => EditorAction::End,
        KeyCode::Up => EditorAction::HistoryUp,
        KeyCode::Down => EditorAction::HistoryDown,

        // Editing keys
        KeyCode::Backspace => EditorAction::Backspace,
        KeyCode::Delete => EditorAction::Delete,
        KeyCode::Tab => EditorAction::Complete,
        KeyCode::Enter => EditorAction::Submit,

        // Everything else
        _ => EditorAction::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn insert_regular_chars() {
        assert_eq!(map_key(key(KeyCode::Char('a'))), EditorAction::Insert('a'));
        assert_eq!(map_key(key(KeyCode::Char('Z'))), EditorAction::Insert('Z'));
        assert_eq!(map_key(key(KeyCode::Char('5'))), EditorAction::Insert('5'));
    }

    #[test]
    fn ctrl_a_is_home() {
        assert_eq!(map_key(ctrl(KeyCode::Char('a'))), EditorAction::Home);
    }

    #[test]
    fn ctrl_e_is_end() {
        assert_eq!(map_key(ctrl(KeyCode::Char('e'))), EditorAction::End);
    }

    #[test]
    fn ctrl_c_is_cancel() {
        assert_eq!(map_key(ctrl(KeyCode::Char('c'))), EditorAction::Cancel);
    }

    #[test]
    fn ctrl_d_is_eof() {
        assert_eq!(map_key(ctrl(KeyCode::Char('d'))), EditorAction::Eof);
    }

    #[test]
    fn ctrl_k_kills_to_end() {
        assert_eq!(map_key(ctrl(KeyCode::Char('k'))), EditorAction::KillToEnd);
    }

    #[test]
    fn ctrl_u_kills_to_start() {
        assert_eq!(map_key(ctrl(KeyCode::Char('u'))), EditorAction::KillToStart);
    }

    #[test]
    fn ctrl_w_deletes_word() {
        assert_eq!(map_key(ctrl(KeyCode::Char('w'))), EditorAction::DeleteWord);
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(map_key(key(KeyCode::Left)), EditorAction::MoveLeft);
        assert_eq!(map_key(key(KeyCode::Right)), EditorAction::MoveRight);
        assert_eq!(map_key(key(KeyCode::Up)), EditorAction::HistoryUp);
        assert_eq!(map_key(key(KeyCode::Down)), EditorAction::HistoryDown);
    }

    #[test]
    fn word_movement() {
        assert_eq!(map_key(ctrl(KeyCode::Left)), EditorAction::MoveWordLeft);
        assert_eq!(map_key(ctrl(KeyCode::Right)), EditorAction::MoveWordRight);
        assert_eq!(map_key(alt(KeyCode::Char('b'))), EditorAction::MoveWordLeft);
        assert_eq!(
            map_key(alt(KeyCode::Char('f'))),
            EditorAction::MoveWordRight
        );
    }

    #[test]
    fn special_keys() {
        assert_eq!(map_key(key(KeyCode::Backspace)), EditorAction::Backspace);
        assert_eq!(map_key(key(KeyCode::Delete)), EditorAction::Delete);
        assert_eq!(map_key(key(KeyCode::Tab)), EditorAction::Complete);
        assert_eq!(map_key(key(KeyCode::Enter)), EditorAction::Submit);
        assert_eq!(map_key(key(KeyCode::Home)), EditorAction::Home);
        assert_eq!(map_key(key(KeyCode::End)), EditorAction::End);
    }

    #[test]
    fn unknown_keys_are_noop() {
        assert_eq!(map_key(key(KeyCode::F(1))), EditorAction::Noop);
        assert_eq!(map_key(key(KeyCode::Esc)), EditorAction::Noop);
    }
}
