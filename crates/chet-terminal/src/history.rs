//! Command history with file persistence and navigation.

use std::fs;
use std::io;
use std::path::PathBuf;

const MAX_ENTRIES: usize = 1000;

/// Persistent command history with up/down navigation.
#[derive(Debug)]
pub struct History {
    entries: Vec<String>,
    path: PathBuf,
    /// Current navigation position. `None` = not navigating (at bottom).
    nav_index: Option<usize>,
    /// Stashed input from before history navigation started.
    stash: String,
}

#[allow(dead_code)]
impl History {
    /// Create a new History that persists to the given file path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            path,
            nav_index: None,
            stash: String::new(),
        }
    }

    /// Load history from disk. Silently ignores missing/unreadable files.
    pub fn load(&mut self) {
        if let Ok(contents) = fs::read_to_string(&self.path) {
            self.entries = contents
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            // Enforce max size on load
            if self.entries.len() > MAX_ENTRIES {
                let excess = self.entries.len() - MAX_ENTRIES;
                self.entries.drain(..excess);
            }
        }
    }

    /// Save history to disk.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = self.entries.join("\n");
        fs::write(
            &self.path,
            if content.is_empty() {
                content
            } else {
                content + "\n"
            },
        )
    }

    /// Add an entry to history. Skips consecutive duplicates and empty strings.
    pub fn add(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        // Skip consecutive duplicates
        if self.entries.last().map(|s| s.as_str()) == Some(trimmed) {
            return;
        }
        self.entries.push(trimmed.to_string());
        // Enforce max size
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// Navigate to the previous (older) history entry.
    /// On first call, stashes `current_input` so it can be restored.
    /// Returns `None` if already at the oldest entry.
    pub fn navigate_up(&mut self, current_input: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        let new_index = match self.nav_index {
            None => {
                // Starting navigation — stash current input
                self.stash = current_input.to_string();
                self.entries.len() - 1
            }
            Some(0) => return None, // Already at oldest
            Some(i) => i - 1,
        };

        self.nav_index = Some(new_index);
        Some(&self.entries[new_index])
    }

    /// Navigate to the next (newer) history entry.
    /// Returns `None` and restores stash when moving past the newest entry.
    pub fn navigate_down(&mut self) -> Option<&str> {
        match self.nav_index {
            None => None, // Not navigating
            Some(i) => {
                if i + 1 < self.entries.len() {
                    self.nav_index = Some(i + 1);
                    Some(&self.entries[i + 1])
                } else {
                    // Past newest — restore stash
                    self.nav_index = None;
                    Some(&self.stash)
                }
            }
        }
    }

    /// Reset navigation state. Called when the user edits the line.
    pub fn reset_navigation(&mut self) {
        self.nav_index = None;
        self.stash.clear();
    }

    /// Number of entries in history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_history() -> History {
        History::new(PathBuf::from("/dev/null"))
    }

    #[test]
    fn add_and_len() {
        let mut h = mem_history();
        h.add("first");
        h.add("second");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn skip_empty() {
        let mut h = mem_history();
        h.add("");
        h.add("   ");
        assert!(h.is_empty());
    }

    #[test]
    fn skip_consecutive_duplicates() {
        let mut h = mem_history();
        h.add("same");
        h.add("same");
        h.add("same");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn non_consecutive_duplicates_kept() {
        let mut h = mem_history();
        h.add("a");
        h.add("b");
        h.add("a");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn navigate_up_and_down() {
        let mut h = mem_history();
        h.add("first");
        h.add("second");
        h.add("third");

        assert_eq!(h.navigate_up("current"), Some("third"));
        assert_eq!(h.navigate_up("current"), Some("second"));
        assert_eq!(h.navigate_up("current"), Some("first"));
        assert_eq!(h.navigate_up("current"), None); // at oldest

        assert_eq!(h.navigate_down(), Some("second"));
        assert_eq!(h.navigate_down(), Some("third"));
        assert_eq!(h.navigate_down(), Some("current")); // stash restored
        assert_eq!(h.navigate_down(), None); // not navigating
    }

    #[test]
    fn navigate_up_empty_history() {
        let mut h = mem_history();
        assert_eq!(h.navigate_up("input"), None);
    }

    #[test]
    fn reset_navigation() {
        let mut h = mem_history();
        h.add("entry");
        h.navigate_up("current");
        h.reset_navigation();
        // After reset, navigate_down should return None (not navigating)
        assert_eq!(h.navigate_down(), None);
    }

    #[test]
    fn max_entries_enforced() {
        let mut h = mem_history();
        for i in 0..1100 {
            h.add(&format!("entry {i}"));
        }
        assert_eq!(h.len(), MAX_ENTRIES);
        // Oldest entries should be dropped
        assert_eq!(h.entries[0], "entry 100");
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");

        let mut h = History::new(path.clone());
        h.add("line one");
        h.add("line two");
        h.save().unwrap();

        let mut h2 = History::new(path);
        h2.load();
        assert_eq!(h2.len(), 2);
        assert_eq!(h2.entries[0], "line one");
        assert_eq!(h2.entries[1], "line two");
    }
}
