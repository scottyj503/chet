//! Rule matcher — evaluates permission rules against tool calls.

use crate::types::{PermissionLevel, PermissionRule};

/// Result of evaluating rules: the winning level + a human-readable description.
#[derive(Debug, Clone)]
pub struct EvaluateResult {
    pub level: PermissionLevel,
    pub description: String,
}

/// Evaluates whether a `PermissionRule` matches a given tool call.
pub struct RuleMatcher;

impl RuleMatcher {
    /// Check if a rule matches the given tool name and input.
    ///
    /// Tool name matching: exact string or glob (e.g., `*`, `Bash`).
    /// Argument matching: `field_name:glob_pattern` format
    /// (e.g., `command:git *`, `file_path:/etc/*`).
    pub fn matches(rule: &PermissionRule, tool_name: &str, tool_input: &serde_json::Value) -> bool {
        // Match tool name
        if !Self::matches_tool_name(&rule.tool, tool_name) {
            return false;
        }

        // Match args pattern (if specified)
        if let Some(ref args_pattern) = rule.args {
            if !Self::matches_args(args_pattern, tool_input) {
                return false;
            }
        }

        true
    }

    /// Find the highest-priority matching rule from a list of rules.
    ///
    /// Priority: most specific rule wins (rules with args > rules without),
    /// then within same specificity: block > permit > prompt.
    /// This allows `permit` rules with `file_path:/src/*` to override
    /// a broader `block` rule on the same tool.
    pub fn evaluate(
        rules: &[PermissionRule],
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Option<EvaluateResult> {
        let matching: Vec<&PermissionRule> = rules
            .iter()
            .filter(|r| Self::matches(r, tool_name, tool_input))
            .collect();

        if matching.is_empty() {
            return None;
        }

        // Partition into specific (has args) and general (no args) rules
        let specific: Vec<&&PermissionRule> =
            matching.iter().filter(|r| r.args.is_some()).collect();
        let general: Vec<&&PermissionRule> = matching.iter().filter(|r| r.args.is_none()).collect();

        // Specific rules win over general rules. Within each group: block > permit > prompt.
        let candidates = if !specific.is_empty() {
            specific
        } else {
            general
        };

        let winner = if let Some(r) = candidates
            .iter()
            .find(|r| r.level == PermissionLevel::Block)
        {
            r
        } else if let Some(r) = candidates
            .iter()
            .find(|r| r.level == PermissionLevel::Permit)
        {
            r
        } else {
            candidates.first().unwrap()
        };

        Some(EvaluateResult {
            level: winner.level.clone(),
            description: Self::describe_rule(winner),
        })
    }

    /// Human-readable description of a matched rule.
    fn describe_rule(rule: &PermissionRule) -> String {
        match &rule.args {
            Some(args) => format!("rule: {} [{}] -> {}", rule.tool, args, rule.level.as_str()),
            None => format!("rule: {} -> {}", rule.tool, rule.level.as_str()),
        }
    }

    fn matches_tool_name(pattern: &str, tool_name: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        // Try glob matching
        match globset::GlobBuilder::new(pattern)
            .case_insensitive(false)
            .build()
        {
            Ok(glob) => glob.compile_matcher().is_match(tool_name),
            // If glob fails to parse, fall back to exact match
            Err(_) => pattern == tool_name,
        }
    }

    fn matches_args(args_pattern: &str, tool_input: &serde_json::Value) -> bool {
        // Format: "field_name:glob_pattern"
        let Some((field_name, glob_pattern)) = args_pattern.split_once(':') else {
            return false;
        };

        let field_value = match tool_input.get(field_name) {
            Some(serde_json::Value::String(s)) => s.as_str(),
            _ => return false,
        };

        // For Bash command field, split compound commands and match each subcommand.
        // A rule matches if ANY subcommand matches the glob pattern.
        if field_name == "command" {
            return split_subcommands(field_value)
                .iter()
                .any(|sub| Self::glob_matches(glob_pattern, sub));
        }

        Self::glob_matches(glob_pattern, field_value)
    }

    fn glob_matches(pattern: &str, value: &str) -> bool {
        match globset::GlobBuilder::new(pattern)
            .case_insensitive(false)
            .build()
        {
            Ok(glob) => glob.compile_matcher().is_match(value),
            Err(_) => pattern == value,
        }
    }
}

/// Split a shell command into subcommands on `&&`, `||`, `;`, and `|`.
/// Each subcommand is trimmed. Preserves quoted strings (single/double).
#[allow(clippy::collapsible_match)]
fn split_subcommands(command: &str) -> Vec<&str> {
    let mut results = Vec::new();
    let mut start = 0;
    let mut chars = command.char_indices().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some((i, c)) = chars.next() {
        match c {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '&' if !in_single_quote && !in_double_quote => {
                if chars.peek().map(|(_, c)| *c) == Some('&') {
                    let sub = command[start..i].trim();
                    if !sub.is_empty() {
                        results.push(sub);
                    }
                    chars.next(); // consume second '&'
                    start = chars.peek().map(|(i, _)| *i).unwrap_or(command.len());
                }
            }
            '|' if !in_single_quote && !in_double_quote => {
                if chars.peek().map(|(_, c)| *c) == Some('|') {
                    // ||
                    let sub = command[start..i].trim();
                    if !sub.is_empty() {
                        results.push(sub);
                    }
                    chars.next(); // consume second '|'
                    start = chars.peek().map(|(i, _)| *i).unwrap_or(command.len());
                } else {
                    // pipe |
                    let sub = command[start..i].trim();
                    if !sub.is_empty() {
                        results.push(sub);
                    }
                    start = chars.peek().map(|(i, _)| *i).unwrap_or(command.len());
                }
            }
            ';' if !in_single_quote && !in_double_quote => {
                let sub = command[start..i].trim();
                if !sub.is_empty() {
                    results.push(sub);
                }
                start = chars.peek().map(|(i, _)| *i).unwrap_or(command.len());
            }
            _ => {}
        }
    }

    // Remaining tail
    let tail = command[start..].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(tool: &str, args: Option<&str>, level: PermissionLevel) -> PermissionRule {
        PermissionRule {
            tool: tool.to_string(),
            args: args.map(|s| s.to_string()),
            level,
        }
    }

    #[test]
    fn test_exact_tool_match() {
        let r = rule("Bash", None, PermissionLevel::Permit);
        assert!(RuleMatcher::matches(&r, "Bash", &json!({})));
    }

    #[test]
    fn test_exact_tool_no_match() {
        let r = rule("Bash", None, PermissionLevel::Permit);
        assert!(!RuleMatcher::matches(&r, "Read", &json!({})));
    }

    #[test]
    fn test_wildcard_tool_match() {
        let r = rule("*", None, PermissionLevel::Permit);
        assert!(RuleMatcher::matches(&r, "Bash", &json!({})));
        assert!(RuleMatcher::matches(&r, "Read", &json!({})));
    }

    #[test]
    fn test_arg_pattern_match() {
        let r = rule("Bash", Some("command:git *"), PermissionLevel::Permit);
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "git status"})
        ));
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "git push origin main"})
        ));
    }

    #[test]
    fn test_arg_pattern_no_match() {
        let r = rule("Bash", Some("command:git *"), PermissionLevel::Permit);
        assert!(!RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "rm -rf /"})
        ));
    }

    #[test]
    fn test_arg_pattern_missing_field() {
        let r = rule("Bash", Some("command:git *"), PermissionLevel::Permit);
        assert!(!RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"file_path": "/tmp/test"})
        ));
    }

    #[test]
    fn test_evaluate_block_wins() {
        let rules = vec![
            rule("Bash", None, PermissionLevel::Permit),
            rule("Bash", Some("command:rm *"), PermissionLevel::Block),
        ];
        let result = RuleMatcher::evaluate(&rules, "Bash", &json!({"command": "rm -rf /"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Block);
        assert!(result.description.contains("command:rm *"));
    }

    #[test]
    fn test_evaluate_permit_over_prompt() {
        let rules = vec![
            rule("Bash", None, PermissionLevel::Prompt),
            rule("Bash", Some("command:git *"), PermissionLevel::Permit),
        ];
        let result = RuleMatcher::evaluate(&rules, "Bash", &json!({"command": "git status"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Permit);
    }

    #[test]
    fn test_evaluate_no_match() {
        let rules = vec![rule("Write", None, PermissionLevel::Block)];
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({}));
        assert!(result.is_none());
    }

    #[test]
    fn test_evaluate_description_includes_args() {
        let rules = vec![rule("Bash", Some("command:git *"), PermissionLevel::Permit)];
        let result = RuleMatcher::evaluate(&rules, "Bash", &json!({"command": "git push"}));
        let result = result.unwrap();
        assert!(result.description.contains("command:git *"));
        assert!(result.description.contains("permit"));
    }

    #[test]
    fn test_evaluate_description_no_args() {
        let rules = vec![rule("Bash", None, PermissionLevel::Prompt)];
        let result = RuleMatcher::evaluate(&rules, "Bash", &json!({}));
        let result = result.unwrap();
        assert_eq!(result.description, "rule: Bash -> prompt");
    }

    #[test]
    fn test_compound_command_block_matches_subcommand() {
        let r = rule("Bash", Some("command:rm *"), PermissionLevel::Block);
        // "rm" is a subcommand in a compound command
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "cd /tmp && rm -rf /"})
        ));
    }

    #[test]
    fn test_compound_command_permit_matches_subcommand() {
        let r = rule("Bash", Some("command:git *"), PermissionLevel::Permit);
        // "git fetch" is a subcommand
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "cd /repo && git fetch && git push"})
        ));
    }

    #[test]
    fn test_compound_command_no_match() {
        let r = rule("Bash", Some("command:git *"), PermissionLevel::Permit);
        assert!(!RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "cd /tmp && ls -la"})
        ));
    }

    #[test]
    fn test_compound_command_semicolon_split() {
        let r = rule("Bash", Some("command:rm *"), PermissionLevel::Block);
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "echo hello; rm -rf /"})
        ));
    }

    #[test]
    fn test_compound_command_pipe_split() {
        let r = rule("Bash", Some("command:grep *"), PermissionLevel::Permit);
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "cat file.txt | grep pattern"})
        ));
    }

    #[test]
    fn test_compound_command_or_split() {
        let r = rule("Bash", Some("command:rm *"), PermissionLevel::Block);
        assert!(RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "test -f /tmp/x || rm -rf /"})
        ));
    }

    #[test]
    fn test_compound_command_quoted_separators_ignored() {
        let r = rule("Bash", Some("command:rm *"), PermissionLevel::Block);
        // The && is inside quotes, so it's not a separator — whole command is one unit
        assert!(!RuleMatcher::matches(
            &r,
            "Bash",
            &json!({"command": "echo 'cd /tmp && rm -rf /'"})
        ));
    }

    #[test]
    fn test_split_subcommands_simple() {
        assert_eq!(split_subcommands("git status"), vec!["git status"]);
    }

    #[test]
    fn test_split_subcommands_and() {
        assert_eq!(
            split_subcommands("cd /tmp && git fetch && git push"),
            vec!["cd /tmp", "git fetch", "git push"]
        );
    }

    #[test]
    fn test_split_subcommands_mixed() {
        assert_eq!(
            split_subcommands("echo a; echo b | cat && echo c || echo d"),
            vec!["echo a", "echo b", "cat", "echo c", "echo d"]
        );
    }

    #[test]
    fn test_split_subcommands_preserves_quotes() {
        assert_eq!(
            split_subcommands("echo 'a && b' && echo c"),
            vec!["echo 'a && b'", "echo c"]
        );
    }

    #[test]
    fn test_arg_pattern_file_path() {
        let r = rule("Read", Some("file_path:/etc/*"), PermissionLevel::Block);
        assert!(RuleMatcher::matches(
            &r,
            "Read",
            &json!({"file_path": "/etc/passwd"})
        ));
        assert!(!RuleMatcher::matches(
            &r,
            "Read",
            &json!({"file_path": "/home/user/file.txt"})
        ));
    }

    #[test]
    fn test_allow_read_overrides_broad_block() {
        // Block Read globally, but permit Read for /src/*
        let rules = vec![
            rule("Read", None, PermissionLevel::Block),
            rule("Read", Some("file_path:/src/*"), PermissionLevel::Permit),
        ];
        // /src/main.rs matches the specific permit rule — should be Permit
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/src/main.rs"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Permit);

        // /etc/passwd only matches the general block rule — should be Block
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/etc/passwd"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Block);
    }

    #[test]
    fn test_specific_block_beats_general_permit() {
        // Permit Read globally, but block Read for /etc/*
        let rules = vec![
            rule("Read", None, PermissionLevel::Permit),
            rule("Read", Some("file_path:/etc/*"), PermissionLevel::Block),
        ];
        // /etc/shadow matches the specific block rule — should be Block
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/etc/shadow"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Block);

        // /src/lib.rs only matches the general permit — should be Permit
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/src/lib.rs"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Permit);
    }

    #[test]
    fn test_multiple_specific_rules_block_wins() {
        // Two specific rules: permit /src/*, block /src/secret/*
        let rules = vec![
            rule("Read", Some("file_path:/src/*"), PermissionLevel::Permit),
            rule(
                "Read",
                Some("file_path:/src/secret/*"),
                PermissionLevel::Block,
            ),
        ];
        // /src/secret/key.rs matches both specific rules — block wins
        let result =
            RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/src/secret/key.rs"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Block);

        // /src/main.rs matches only the permit rule
        let result = RuleMatcher::evaluate(&rules, "Read", &json!({"file_path": "/src/main.rs"}));
        let result = result.unwrap();
        assert_eq!(result.level, PermissionLevel::Permit);
    }
}
