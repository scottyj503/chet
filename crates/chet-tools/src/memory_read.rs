//! MemoryRead tool — reads persistent memory.

use chet_session::MemoryManager;
use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use std::path::PathBuf;

/// Tool for reading persistent memory (global + project).
pub struct MemoryReadTool {
    memory_dir: PathBuf,
    project_id: Option<String>,
}

impl MemoryReadTool {
    pub fn new(memory_dir: PathBuf, project_id: Option<String>) -> Self {
        Self {
            memory_dir,
            project_id,
        }
    }
}

impl Tool for MemoryReadTool {
    fn name(&self) -> &str {
        "MemoryRead"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "MemoryRead".to_string(),
            description: "Read persistent memory (global and project-specific). \
                          Returns saved notes, preferences, and context that persist across sessions."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            cache_control: None,
        }
    }

    fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: ToolContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolOutput, ToolError>> + Send + '_>,
    > {
        Box::pin(async move {
            let mgr = MemoryManager::new(self.memory_dir.clone());
            let combined = mgr.load_combined(self.project_id.as_deref()).await;
            if combined.is_empty() {
                Ok(ToolOutput::text("No memory saved yet."))
            } else {
                Ok(ToolOutput::text(combined))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn name_and_read_only() {
        let tool = MemoryReadTool::new(PathBuf::from("/tmp"), None);
        assert_eq!(tool.name(), "MemoryRead");
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn read_empty() {
        let dir = TempDir::new().unwrap();
        let tool = MemoryReadTool::new(dir.path().to_path_buf(), None);
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            env: Default::default(),
            sandboxed: false,
        };
        let result = tool.execute(serde_json::json!({}), ctx).await.unwrap();
        assert!(!result.is_error);
        match &result.content[0] {
            chet_types::ToolOutputContent::Text { text } => {
                assert_eq!(text, "No memory saved yet.");
            }
            _ => panic!("expected text output"),
        }
    }

    #[tokio::test]
    async fn read_with_content() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("remembered preference").await.unwrap();

        let tool = MemoryReadTool::new(dir.path().to_path_buf(), None);
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            env: Default::default(),
            sandboxed: false,
        };
        let result = tool.execute(serde_json::json!({}), ctx).await.unwrap();
        assert!(!result.is_error);
        match &result.content[0] {
            chet_types::ToolOutputContent::Text { text } => {
                assert!(text.contains("remembered preference"));
            }
            _ => panic!("expected text output"),
        }
    }
}
