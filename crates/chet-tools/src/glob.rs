//! Glob tool — find files by pattern.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;
use std::path::PathBuf;

/// Tool for finding files matching glob patterns.
pub struct GlobTool;

#[derive(Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Glob".to_string(),
            description: "Find files matching a glob pattern. Results are sorted by \
                          modification time (newest first). Respects .gitignore."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g. \"**/*.rs\", \"src/**/*.ts\")"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (defaults to cwd)"
                    }
                }
            }),
        }
    }

    fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>,
    > {
        Box::pin(async move {
            let input: GlobInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "Glob".into(),
                    message: e.to_string(),
                })?;

            let search_dir = input
                .path
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.cwd.clone());

            let glob = globset::GlobBuilder::new(&input.pattern)
                .literal_separator(false)
                .build()
                .map_err(|e| ToolError::InvalidInput {
                    tool: "Glob".into(),
                    message: format!("Invalid glob pattern: {e}"),
                })?
                .compile_matcher();

            // Walk the directory tree and collect matching files
            // Use tokio::task::spawn_blocking since walkdir is synchronous
            let search_dir_clone = search_dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut found = Vec::new();
                for entry in walkdir::WalkDir::new(&search_dir_clone)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let path = entry.path();
                    // Match against the relative path
                    if let Ok(rel) = path.strip_prefix(&search_dir_clone) {
                        if glob.is_match(rel) && path.is_file() {
                            let mtime = entry
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            found.push((path.to_path_buf(), mtime));
                        }
                    }
                }
                found
            })
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let mut matches = result;

            // Sort by modification time, newest first
            matches.sort_by(|a, b| b.1.cmp(&a.1));

            if matches.is_empty() {
                return Ok(ToolOutput::text("No files found"));
            }

            let output: String = matches
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");

            Ok(ToolOutput::text(output))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_ctx_with_dir(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            env: HashMap::new(),
            sandboxed: false,
        }
    }

    #[tokio::test]
    async fn test_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foo.rs"), "").unwrap();
        std::fs::write(dir.path().join("bar.rs"), "").unwrap();
        std::fs::write(dir.path().join("baz.txt"), "").unwrap();

        let output = GlobTool
            .execute(
                serde_json::json!({"pattern": "*.rs"}),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("foo.rs"));
        assert!(text.contains("bar.rs"));
        assert!(!text.contains("baz.txt"));
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foo.txt"), "").unwrap();

        let output = GlobTool
            .execute(
                serde_json::json!({"pattern": "*.rs"}),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert_eq!(text, "No files found");
    }
}
