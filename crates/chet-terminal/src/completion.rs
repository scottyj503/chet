//! Tab completion support.

/// Trait for providing tab completions.
pub trait Completer: Send {
    /// Return possible completions for the given line and cursor position.
    /// Each completion is the full replacement text for the line.
    fn complete(&self, line: &str, cursor: usize) -> Vec<String>;
}

/// Completes slash commands (e.g., `/he` → `/help`).
pub struct SlashCommandCompleter {
    commands: Vec<String>,
}

impl SlashCommandCompleter {
    pub fn new(commands: Vec<&str>) -> Self {
        Self {
            commands: commands.into_iter().map(String::from).collect(),
        }
    }
}

impl Completer for SlashCommandCompleter {
    fn complete(&self, line: &str, _cursor: usize) -> Vec<String> {
        let trimmed = line.trim();
        if !trimmed.starts_with('/') {
            return Vec::new();
        }
        // Only complete if the line is just the command (no args)
        if trimmed.contains(' ') {
            return Vec::new();
        }
        self.commands
            .iter()
            .filter(|cmd| cmd.starts_with(trimmed) && *cmd != trimmed)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completer() -> SlashCommandCompleter {
        SlashCommandCompleter::new(vec![
            "/quit", "/exit", "/clear", "/cost", "/help", "/context", "/compact",
        ])
    }

    #[test]
    fn complete_prefix() {
        let c = completer();
        let results = c.complete("/he", 3);
        assert_eq!(results, vec!["/help"]);
    }

    #[test]
    fn complete_multiple_matches() {
        let c = completer();
        let mut results = c.complete("/co", 3);
        results.sort();
        assert_eq!(results, vec!["/compact", "/context", "/cost"]);
    }

    #[test]
    fn no_match() {
        let c = completer();
        let results = c.complete("/xyz", 4);
        assert!(results.is_empty());
    }

    #[test]
    fn exact_match_returns_empty() {
        let c = completer();
        let results = c.complete("/help", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn non_slash_returns_empty() {
        let c = completer();
        let results = c.complete("hello", 5);
        assert!(results.is_empty());
    }
}
