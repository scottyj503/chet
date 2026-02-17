//! Grep tool — search file contents with regex.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use grep_regex::RegexMatcher;
use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use serde::Deserialize;
use std::path::PathBuf;

/// Tool for searching file contents using regex patterns.
pub struct GrepTool;

#[derive(Deserialize)]
#[allow(dead_code)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "default_output_mode")]
    output_mode: String,
    #[serde(default, rename = "type")]
    file_type: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    context: Option<usize>,
    #[serde(default, rename = "-i")]
    case_insensitive: bool,
}

fn default_output_mode() -> String {
    "files_with_matches".to_string()
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "Grep".to_string(),
            description: "Search file contents using regex patterns. Supports output modes: \
                          'content' (matching lines), 'files_with_matches' (file paths only), \
                          'count' (match counts). Defaults to 'files_with_matches'."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in (defaults to cwd)"
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"],
                        "description": "Output mode (default: files_with_matches)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g. \"*.rs\")"
                    },
                    "head_limit": {
                        "type": "integer",
                        "description": "Limit output to first N results"
                    },
                    "context": {
                        "type": "integer",
                        "description": "Lines of context around matches (for content mode)"
                    },
                    "-i": {
                        "type": "boolean",
                        "description": "Case-insensitive search"
                    }
                }
            }),
            cache_control: None,
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
            let input: GrepInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "Grep".into(),
                    message: e.to_string(),
                })?;

            let search_path = input
                .path
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.cwd.clone());

            let matcher = RegexMatcher::new_line_matcher(&input.pattern).map_err(|e| {
                ToolError::InvalidInput {
                    tool: "Grep".into(),
                    message: format!("Invalid regex: {e}"),
                }
            })?;

            let head_limit = input.head_limit.unwrap_or(0);

            // Use spawn_blocking since grep-searcher is synchronous
            let output_mode = input.output_mode.clone();
            let glob_filter = input.glob.clone();

            let result = tokio::task::spawn_blocking(move || {
                search_files(
                    &matcher,
                    &search_path,
                    &output_mode,
                    glob_filter.as_deref(),
                    head_limit,
                )
            })
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .map_err(ToolError::ExecutionFailed)?;

            if result.is_empty() {
                return Ok(ToolOutput::text("No matches found"));
            }

            Ok(ToolOutput::text(result))
        })
    }
}

fn search_files(
    matcher: &RegexMatcher,
    path: &std::path::Path,
    output_mode: &str,
    glob_filter: Option<&str>,
    head_limit: usize,
) -> Result<String, String> {
    let mut results = Vec::new();

    let file_glob = glob_filter
        .map(|g| {
            globset::GlobBuilder::new(g)
                .build()
                .map(|g| g.compile_matcher())
        })
        .transpose()
        .map_err(|e| format!("Invalid glob filter: {e}"))?;

    if path.is_file() {
        search_single_file(matcher, path, output_mode, &mut results);
    } else {
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }

            if let Some(ref glob) = file_glob {
                let name = entry.file_name().to_string_lossy();
                if !glob.is_match(name.as_ref()) {
                    continue;
                }
            }

            search_single_file(matcher, entry.path(), output_mode, &mut results);

            if head_limit > 0 && results.len() >= head_limit {
                results.truncate(head_limit);
                break;
            }
        }
    }

    Ok(results.join("\n"))
}

fn search_single_file(
    matcher: &RegexMatcher,
    path: &std::path::Path,
    output_mode: &str,
    results: &mut Vec<String>,
) {
    let mut searcher = Searcher::new();

    match output_mode {
        "files_with_matches" => {
            let mut found = false;
            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|_line_num, _line| {
                    found = true;
                    Ok(false) // Stop after first match
                }),
            );
            if found {
                results.push(path.display().to_string());
            }
        }
        "count" => {
            let mut count = 0u64;
            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|_line_num, _line| {
                    count += 1;
                    Ok(true)
                }),
            );
            if count > 0 {
                results.push(format!("{}:{count}", path.display()));
            }
        }
        _ => {
            // "content" mode
            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|line_num, line| {
                    results.push(format!("{}:{line_num}:{}", path.display(), line.trim_end()));
                    Ok(true)
                }),
            );
        }
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
    async fn test_grep_files_with_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("match.txt"), "hello world\n").unwrap();
        std::fs::write(dir.path().join("nomatch.txt"), "goodbye\n").unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({"pattern": "hello"}),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("match.txt"));
        assert!(!text.contains("nomatch.txt"));
    }

    #[tokio::test]
    async fn test_grep_content_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "line one\nline two\nline three\n",
        )
        .unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "two",
                    "path": dir.path().join("test.txt").to_str().unwrap(),
                    "output_mode": "content"
                }),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("line two"));
        assert!(!text.contains("line one"));
    }

    #[tokio::test]
    async fn test_grep_count_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "foo\nfoo\nbar\n").unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "foo",
                    "path": dir.path().join("test.txt").to_str().unwrap(),
                    "output_mode": "count"
                }),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains(":2"));
    }
}
