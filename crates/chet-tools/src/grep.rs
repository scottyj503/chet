//! Grep tool — search file contents with regex.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{SearcherBuilder, Sink, SinkContext, SinkMatch};
use serde::Deserialize;
use std::path::PathBuf;

/// Tool for searching file contents using regex patterns.
pub struct GrepTool;

#[derive(Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "default_output_mode")]
    output_mode: String,
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

            let matcher = RegexMatcherBuilder::new()
                .case_insensitive(input.case_insensitive)
                .build(&input.pattern)
                .map_err(|e| ToolError::InvalidInput {
                    tool: "Grep".into(),
                    message: format!("Invalid regex: {e}"),
                })?;

            let head_limit = input.head_limit.unwrap_or(0);
            let context_lines = input.context.unwrap_or(0);

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
                    context_lines,
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
    matcher: &grep_regex::RegexMatcher,
    path: &std::path::Path,
    output_mode: &str,
    glob_filter: Option<&str>,
    head_limit: usize,
    context_lines: usize,
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
        search_single_file(matcher, path, output_mode, context_lines, &mut results);
    } else {
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_vcs_dir(e))
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

            search_single_file(
                matcher,
                entry.path(),
                output_mode,
                context_lines,
                &mut results,
            );

            if head_limit > 0 && results.len() >= head_limit {
                results.truncate(head_limit);
                break;
            }
        }
    }

    Ok(results.join("\n"))
}

/// Sink that captures both matching and context lines for content mode.
struct ContentSink<'a> {
    results: &'a mut Vec<String>,
    path: &'a std::path::Path,
}

impl Sink for ContentSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        let line = std::str::from_utf8(mat.bytes()).unwrap_or("");
        let line_num = mat.line_number().unwrap_or(0);
        self.results.push(format!(
            "{}:{line_num}:{}",
            self.path.display(),
            line.trim_end()
        ));
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, std::io::Error> {
        let line = std::str::from_utf8(ctx.bytes()).unwrap_or("");
        let line_num = ctx.line_number().unwrap_or(0);
        self.results.push(format!(
            "{}:{line_num}-{}",
            self.path.display(),
            line.trim_end()
        ));
        Ok(true)
    }

    fn context_break(
        &mut self,
        _searcher: &grep_searcher::Searcher,
    ) -> Result<bool, std::io::Error> {
        self.results.push("--".to_string());
        Ok(true)
    }
}

fn is_vcs_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir() && matches!(entry.file_name().to_str(), Some(".git" | ".jj" | ".sl"))
}

fn search_single_file(
    matcher: &grep_regex::RegexMatcher,
    path: &std::path::Path,
    output_mode: &str,
    context_lines: usize,
    results: &mut Vec<String>,
) {
    match output_mode {
        "files_with_matches" => {
            let mut searcher = SearcherBuilder::new().build();
            let mut found = false;
            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|_line_num, _line| {
                    found = true;
                    Ok(false)
                }),
            );
            if found {
                results.push(path.display().to_string());
            }
        }
        "count" => {
            let mut searcher = SearcherBuilder::new().build();
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
            let mut searcher = SearcherBuilder::new()
                .before_context(context_lines)
                .after_context(context_lines)
                .line_number(true)
                .build();
            let _ = searcher.search_path(matcher, path, ContentSink { results, path });
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

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "Hello World\nhello world\n").unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "HELLO",
                    "-i": true,
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

    #[tokio::test]
    async fn test_grep_context_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "before\nmatch_line\nafter\n").unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "match_line",
                    "path": dir.path().join("test.txt").to_str().unwrap(),
                    "output_mode": "content",
                    "context": 1
                }),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("before"));
        assert!(text.contains("match_line"));
        assert!(text.contains("after"));
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "content\n").unwrap();

        let result = GrepTool
            .execute(
                serde_json::json!({"pattern": "[invalid"}),
                test_ctx_with_dir(dir.path()),
            )
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let output = GrepTool
            .execute(
                serde_json::json!({"pattern": "zzz_no_match"}),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        assert_eq!(text, "No matches found");
    }

    #[tokio::test]
    async fn test_grep_head_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "match_me\n").unwrap();
        }

        let output = GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "match_me",
                    "head_limit": 3
                }),
                test_ctx_with_dir(dir.path()),
            )
            .await
            .unwrap();

        let text = match &output.content[0] {
            chet_types::ToolOutputContent::Text { text } => text,
            _ => panic!("expected text"),
        };
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
    }
}
