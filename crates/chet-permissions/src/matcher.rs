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
    /// Priority: block > permit > prompt.
    /// Returns the permission level + description of the winning rule, or None if no rules match.
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

        // block > permit > prompt — find the winning rule for its description
        let winner = if let Some(r) = matching.iter().find(|r| r.level == PermissionLevel::Block) {
            r
        } else if let Some(r) = matching.iter().find(|r| r.level == PermissionLevel::Permit) {
            r
        } else {
            matching.first().unwrap()
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

        match globset::GlobBuilder::new(glob_pattern)
            .case_insensitive(false)
            .build()
        {
            Ok(glob) => glob.compile_matcher().is_match(field_value),
            Err(_) => glob_pattern == field_value,
        }
    }
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
}
