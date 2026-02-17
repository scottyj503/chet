//! Edit tool — string replacement in files.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;

/// Tool for editing files via old_string/new_string replacement.
pub struct EditTool;

#[derive(Deserialize)]
struct EditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Edit".to_string(),
            description: "Perform exact string replacements in files. The old_string must be \
                          unique in the file unless replace_all is true. Use this for targeted \
                          edits to existing files."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["file_path", "old_string", "new_string"],
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences (default: false, requires unique match)"
                    }
                }
            }),
            cache_control: None,
        }
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>,
    > {
        Box::pin(async move {
            let input: EditInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "Edit".into(),
                    message: e.to_string(),
                })?;

            if input.old_string == input.new_string {
                return Ok(ToolOutput::error(
                    "old_string and new_string are identical — no change needed",
                ));
            }

            let content = tokio::fs::read_to_string(&input.file_path)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("{}: {e}", input.file_path)))?;

            let match_count = content.matches(&input.old_string).count();

            if match_count == 0 {
                return Ok(ToolOutput::error(format!(
                    "old_string not found in {}",
                    input.file_path
                )));
            }

            if !input.replace_all && match_count > 1 {
                return Ok(ToolOutput::error(format!(
                    "old_string found {match_count} times in {} — provide more context to make \
                 it unique, or set replace_all to true",
                    input.file_path
                )));
            }

            let new_content = if input.replace_all {
                content.replace(&input.old_string, &input.new_string)
            } else {
                // Replace only the first (and only) occurrence
                content.replacen(&input.old_string, &input.new_string, 1)
            };

            tokio::fs::write(&input.file_path, &new_content)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("{}: {e}", input.file_path)))?;

            let msg = if input.replace_all {
                format!("Replaced {match_count} occurrences in {}", input.file_path)
            } else {
                format!("Successfully edited {}", input.file_path)
            };

            Ok(ToolOutput::text(msg))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("/tmp"),
            env: HashMap::new(),
            sandboxed: false,
        }
    }

    #[tokio::test]
    async fn test_edit_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world\ngoodbye world\n").unwrap();

        let output = EditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "hello world",
                    "new_string": "hi world"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hi world\ngoodbye world\n");
    }

    #[tokio::test]
    async fn test_edit_non_unique_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "foo bar\nfoo baz\n").unwrap();

        let output = EditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "qux"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(output.is_error);
        // File should be unchanged
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "foo bar\nfoo baz\n");
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "foo bar\nfoo baz\nfoo qux\n").unwrap();

        let output = EditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "replaced",
                    "replace_all": true
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "replaced bar\nreplaced baz\nreplaced qux\n");
    }

    #[tokio::test]
    async fn test_edit_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world\n").unwrap();

        let output = EditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "nonexistent",
                    "new_string": "replacement"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(output.is_error);
    }
}
