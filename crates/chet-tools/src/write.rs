//! Write tool — creates or overwrites files.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;
use std::path::Path;

/// Tool for creating or overwriting files.
pub struct WriteTool;

#[derive(Deserialize)]
struct WriteInput {
    file_path: String,
    content: String,
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Write".to_string(),
            description: "Write content to a file. Creates the file and any parent directories \
                          if they don't exist. Overwrites existing files."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["file_path", "content"],
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
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
            let input: WriteInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "Write".into(),
                    message: e.to_string(),
                })?;

            let path = Path::new(&input.file_path);

            // Create parent directories if needed
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed(format!("Failed to create dirs: {e}"))
                })?;
            }

            // Atomic write: tmp file + rename to prevent corruption
            let tmp = path.with_extension("chet-tmp");
            tokio::fs::write(&tmp, &input.content)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("{}: {e}", input.file_path)))?;
            tokio::fs::rename(&tmp, path)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("{}: {e}", input.file_path)))?;

            Ok(ToolOutput::text(format!(
                "Successfully wrote {} bytes to {}",
                input.content.len(),
                input.file_path
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            env: HashMap::new(),
            sandboxed: false,
        }
    }

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");

        let output = WriteTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "content": "hello world"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_write_creates_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c/deep.txt");

        WriteTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "content": "deep content"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep content");
    }

    #[tokio::test]
    async fn test_write_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        std::fs::write(&path, "old content").unwrap();

        WriteTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "content": "new content"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[tokio::test]
    async fn test_write_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");

        let output = WriteTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "content": ""
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[tokio::test]
    async fn test_write_content_verification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("verify.txt");
        let content = "line 1\nline 2\nline 3\n";

        WriteTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "content": content
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, content.as_bytes());
    }
}
