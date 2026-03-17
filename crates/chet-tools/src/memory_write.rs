//! MemoryWrite tool — writes to persistent memory.

use chet_session::MemoryManager;
use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use serde::Deserialize;
use std::path::PathBuf;

/// Tool for writing to persistent memory (global or project).
pub struct MemoryWriteTool {
    memory_dir: PathBuf,
    project_id: Option<String>,
}

impl MemoryWriteTool {
    pub fn new(memory_dir: PathBuf, project_id: Option<String>) -> Self {
        Self {
            memory_dir,
            project_id,
        }
    }
}

#[derive(Deserialize)]
struct MemoryWriteInput {
    scope: String,
    content: String,
}

impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "MemoryWrite"
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "MemoryWrite".to_string(),
            description: "Write to persistent memory. Saves notes, preferences, and context \
                          that persist across sessions. Use MemoryRead first to see existing \
                          content, then provide the complete updated content."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["scope", "content"],
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["global", "project"],
                        "description": "Which memory to write: 'global' for cross-project, 'project' for current project only"
                    },
                    "content": {
                        "type": "string",
                        "description": "The complete memory content (replaces existing content for this scope)"
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
            let input: MemoryWriteInput =
                serde_json::from_value(input).map_err(|e| ToolError::InvalidInput {
                    tool: "MemoryWrite".into(),
                    message: e.to_string(),
                })?;

            let mgr = MemoryManager::new(self.memory_dir.clone());
            let bytes = input.content.len();

            match input.scope.as_str() {
                "global" => {
                    mgr.write_global(&input.content)
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                    Ok(ToolOutput::text(format!(
                        "Saved {bytes} bytes to global memory."
                    )))
                }
                "project" => {
                    let project_id =
                        self.project_id
                            .as_deref()
                            .ok_or(ToolError::ExecutionFailed(
                            "No project context available (not in a git repo or known directory)."
                                .to_string(),
                        ))?;
                    mgr.write_project(project_id, &input.content)
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                    Ok(ToolOutput::text(format!(
                        "Saved {bytes} bytes to project memory."
                    )))
                }
                other => Err(ToolError::InvalidInput {
                    tool: "MemoryWrite".into(),
                    message: format!("Invalid scope '{other}'. Must be 'global' or 'project'."),
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ctx(dir: &TempDir) -> ToolContext {
        ToolContext {
            cwd: dir.path().to_path_buf(),
            env: Default::default(),
            sandboxed: false,
        }
    }

    #[test]
    fn name_and_not_read_only() {
        let tool = MemoryWriteTool::new(PathBuf::from("/tmp"), None);
        assert_eq!(tool.name(), "MemoryWrite");
        assert!(!tool.is_read_only());
    }

    #[tokio::test]
    async fn write_global() {
        let dir = TempDir::new().unwrap();
        let tool = MemoryWriteTool::new(dir.path().to_path_buf(), None);
        let result = tool
            .execute(
                serde_json::json!({"scope": "global", "content": "remember this"}),
                make_ctx(&dir),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        // Verify file was written
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        assert_eq!(mgr.load_global().await, "remember this");
    }

    #[tokio::test]
    async fn write_project() {
        let dir = TempDir::new().unwrap();
        let project_id = "abcdef0123456789".to_string();
        let tool = MemoryWriteTool::new(dir.path().to_path_buf(), Some(project_id.clone()));
        let result = tool
            .execute(
                serde_json::json!({"scope": "project", "content": "project note"}),
                make_ctx(&dir),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let mgr = MemoryManager::new(dir.path().to_path_buf());
        assert_eq!(mgr.load_project(&project_id).await, "project note");
    }

    #[tokio::test]
    async fn invalid_scope() {
        let dir = TempDir::new().unwrap();
        let tool = MemoryWriteTool::new(dir.path().to_path_buf(), None);
        let result = tool
            .execute(
                serde_json::json!({"scope": "bad", "content": "x"}),
                make_ctx(&dir),
            )
            .await;
        match result {
            Err(ToolError::InvalidInput { tool, .. }) => assert_eq!(tool, "MemoryWrite"),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_project_id() {
        let dir = TempDir::new().unwrap();
        let tool = MemoryWriteTool::new(dir.path().to_path_buf(), None);
        let result = tool
            .execute(
                serde_json::json!({"scope": "project", "content": "x"}),
                make_ctx(&dir),
            )
            .await;
        match result {
            Err(ToolError::ExecutionFailed(msg)) => {
                assert!(msg.contains("No project context"));
            }
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }
}
