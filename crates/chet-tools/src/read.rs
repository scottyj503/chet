//! Read tool — reads files with line numbers.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;

/// Tool for reading files with line numbers, offset, and limit support.
pub struct ReadTool;

#[derive(Deserialize)]
struct ReadInput {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Read".to_string(),
            description: "Read a file from the filesystem. Returns content with line numbers. \
                          Supports offset and limit for reading portions of large files."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["file_path"],
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                }
            }),
        }
    }

    fn execute(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>> {
        Box::pin(async move {
        let input: ReadInput = serde_json::from_value(input).map_err(|e| {
            ToolError::InvalidInput {
                tool: "Read".into(),
                message: e.to_string(),
            }
        })?;

        let content = tokio::fs::read_to_string(&input.file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("{}: {e}", input.file_path)))?;

        if content.is_empty() {
            return Ok(ToolOutput::text("(empty file)"));
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let offset = input.offset.unwrap_or(1).max(1) - 1; // Convert to 0-based
        let limit = input.limit.unwrap_or(2000);

        let end = (offset + limit).min(total_lines);
        let selected = &lines[offset.min(total_lines)..end];

        let max_line_num_width = format!("{}", end).len();
        let mut output = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = offset + i + 1;
            // Truncate lines longer than 2000 chars
            let display_line = if line.len() > 2000 {
                &line[..2000]
            } else {
                line
            };
            output.push_str(&format!(
                "{:>width$}\t{}\n",
                line_num,
                display_line,
                width = max_line_num_width
            ));
        }

        if end < total_lines {
            output.push_str(&format!(
                "\n(showing lines {}-{} of {total_lines})",
                offset + 1,
                end
            ));
        }

        Ok(ToolOutput::text(output))
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
    async fn test_read_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "line one\nline two\nline three\n").unwrap();

        let output = ReadTool
            .execute(
                serde_json::json!({"file_path": path.to_str().unwrap()}),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("line one"));
        assert!(text.contains("line two"));
        assert!(text.contains("line three"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let result = ReadTool
            .execute(
                serde_json::json!({"file_path": "/tmp/nonexistent_chet_test_file.txt"}),
                test_ctx(),
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_with_offset_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let content: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &content).unwrap();

        let output = ReadTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "offset": 50,
                    "limit": 10
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("line 50"));
        assert!(text.contains("line 59"));
        assert!(!text.contains("line 60\n")); // limit=10 from line 50
    }
}
