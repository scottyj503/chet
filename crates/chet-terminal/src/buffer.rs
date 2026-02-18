//! Editable line buffer with cursor management.

/// A line buffer backed by `Vec<char>` for O(1) cursor indexing.
#[derive(Debug, Clone)]
pub struct LineBuffer {
    chars: Vec<char>,
    cursor: usize,
}

#[allow(dead_code)]
impl LineBuffer {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
        }
    }

    /// Insert a character at the cursor position.
    pub fn insert(&mut self, ch: char) {
        self.chars.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// Insert a string at the cursor position.
    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.insert(ch);
        }
    }

    /// Delete the character before the cursor (backspace).
    pub fn delete_backward(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    /// Delete the character at the cursor (delete key).
    pub fn delete_forward(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    /// Move cursor one position left.
    pub fn move_left(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    /// Move cursor one position right.
    pub fn move_right(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    /// Move cursor to start of line.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end of line.
    pub fn move_end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Move cursor one word left (to start of previous word).
    pub fn move_word_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        // Skip whitespace
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        // Skip word characters
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        true
    }

    /// Move cursor one word right (to end of next word).
    pub fn move_word_right(&mut self) -> bool {
        let len = self.chars.len();
        if self.cursor >= len {
            return false;
        }
        // Skip word characters
        while self.cursor < len && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        // Skip whitespace
        while self.cursor < len && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        true
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word_backward(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let start = self.cursor;
        // Skip whitespace
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        // Skip word characters
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        self.chars.drain(self.cursor..start);
        true
    }

    /// Delete from cursor to end of line (Ctrl+K).
    pub fn delete_to_end(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.truncate(self.cursor);
            true
        } else {
            false
        }
    }

    /// Delete from start of line to cursor (Ctrl+U).
    pub fn delete_to_start(&mut self) -> bool {
        if self.cursor > 0 {
            self.chars.drain(..self.cursor);
            self.cursor = 0;
            true
        } else {
            false
        }
    }

    /// Clear the entire buffer.
    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    /// Replace buffer contents with the given string.
    pub fn set(&mut self, s: &str) {
        self.chars = s.chars().collect();
        self.cursor = self.chars.len();
    }

    /// Return the buffer contents as a String.
    pub fn as_str(&self) -> String {
        self.chars.iter().collect()
    }

    /// Return the current cursor position (in chars).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Return the number of characters in the buffer.
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// Return true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_empty() {
        let buf = LineBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.cursor(), 0);
        assert_eq!(buf.as_str(), "");
    }

    #[test]
    fn insert_appends_at_cursor() {
        let mut buf = LineBuffer::new();
        buf.insert('h');
        buf.insert('i');
        assert_eq!(buf.as_str(), "hi");
        assert_eq!(buf.cursor(), 2);
    }

    #[test]
    fn insert_in_middle() {
        let mut buf = LineBuffer::new();
        buf.insert('a');
        buf.insert('c');
        buf.move_left();
        buf.insert('b');
        assert_eq!(buf.as_str(), "abc");
        assert_eq!(buf.cursor(), 2);
    }

    #[test]
    fn insert_str_works() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello");
        assert_eq!(buf.as_str(), "hello");
        assert_eq!(buf.cursor(), 5);
    }

    #[test]
    fn delete_backward() {
        let mut buf = LineBuffer::new();
        buf.insert_str("abc");
        assert!(buf.delete_backward());
        assert_eq!(buf.as_str(), "ab");
        assert_eq!(buf.cursor(), 2);
    }

    #[test]
    fn delete_backward_at_start_returns_false() {
        let mut buf = LineBuffer::new();
        assert!(!buf.delete_backward());
    }

    #[test]
    fn delete_forward() {
        let mut buf = LineBuffer::new();
        buf.insert_str("abc");
        buf.move_home();
        assert!(buf.delete_forward());
        assert_eq!(buf.as_str(), "bc");
        assert_eq!(buf.cursor(), 0);
    }

    #[test]
    fn delete_forward_at_end_returns_false() {
        let mut buf = LineBuffer::new();
        buf.insert_str("abc");
        assert!(!buf.delete_forward());
    }

    #[test]
    fn move_left_right() {
        let mut buf = LineBuffer::new();
        buf.insert_str("abc");
        assert!(buf.move_left());
        assert_eq!(buf.cursor(), 2);
        assert!(buf.move_right());
        assert_eq!(buf.cursor(), 3);
        // Can't go past end
        assert!(!buf.move_right());
    }

    #[test]
    fn move_left_at_start_returns_false() {
        let buf = &mut LineBuffer::new();
        assert!(!buf.move_left());
    }

    #[test]
    fn home_and_end() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello");
        buf.move_home();
        assert_eq!(buf.cursor(), 0);
        buf.move_end();
        assert_eq!(buf.cursor(), 5);
    }

    #[test]
    fn move_word_left() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello world foo");
        buf.move_word_left();
        assert_eq!(buf.cursor(), 12); // start of "foo"
        buf.move_word_left();
        assert_eq!(buf.cursor(), 6); // start of "world"
        buf.move_word_left();
        assert_eq!(buf.cursor(), 0); // start of "hello"
        assert!(!buf.move_word_left()); // at start
    }

    #[test]
    fn move_word_right() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello world foo");
        buf.move_home();
        buf.move_word_right();
        assert_eq!(buf.cursor(), 6); // after "hello "
        buf.move_word_right();
        assert_eq!(buf.cursor(), 12); // after "world "
        buf.move_word_right();
        assert_eq!(buf.cursor(), 15); // end of "foo"
        assert!(!buf.move_word_right()); // at end
    }

    #[test]
    fn delete_word_backward() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello world");
        assert!(buf.delete_word_backward());
        assert_eq!(buf.as_str(), "hello ");
        assert_eq!(buf.cursor(), 6);
    }

    #[test]
    fn delete_word_backward_at_start() {
        let mut buf = LineBuffer::new();
        assert!(!buf.delete_word_backward());
    }

    #[test]
    fn delete_to_end() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello world");
        buf.move_home();
        buf.move_word_right(); // cursor at 6
        assert!(buf.delete_to_end());
        assert_eq!(buf.as_str(), "hello ");
    }

    #[test]
    fn delete_to_end_at_end_returns_false() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hi");
        assert!(!buf.delete_to_end());
    }

    #[test]
    fn delete_to_start() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello world");
        buf.move_home();
        buf.move_word_right(); // cursor at 6
        assert!(buf.delete_to_start());
        assert_eq!(buf.as_str(), "world");
        assert_eq!(buf.cursor(), 0);
    }

    #[test]
    fn delete_to_start_at_start_returns_false() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hi");
        buf.move_home();
        assert!(!buf.delete_to_start());
    }

    #[test]
    fn clear_resets_everything() {
        let mut buf = LineBuffer::new();
        buf.insert_str("hello");
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.cursor(), 0);
    }

    #[test]
    fn set_replaces_content() {
        let mut buf = LineBuffer::new();
        buf.insert_str("old");
        buf.set("new content");
        assert_eq!(buf.as_str(), "new content");
        assert_eq!(buf.cursor(), 11); // at end
    }
}
