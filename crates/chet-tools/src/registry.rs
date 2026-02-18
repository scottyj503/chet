//! Tool registry for name-based dispatch.

use chet_types::{Tool, ToolContext, ToolDefinition, ToolError, ToolOutput};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of available tools, supporting name-based dispatch.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Create a registry with all built-in tools.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(super::ReadTool));
        registry.register(Arc::new(super::WriteTool));
        registry.register(Arc::new(super::EditTool));
        registry.register(Arc::new(super::BashTool::new()));
        registry.register(Arc::new(super::GlobTool));
        registry.register(Arc::new(super::GrepTool));
        registry
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get all tool definitions for sending to the API.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Get only read-only tool definitions (for plan mode).
    pub fn read_only_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|t| t.is_read_only())
            .map(|t| t.definition())
            .collect()
    }

    /// Execute a tool by name with the given input.
    pub async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| ToolError::UnknownTool {
            name: name.to_string(),
        })?;
        tool.execute(input, ctx).await
    }

    /// Check if a tool exists by name.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Query whether a tool is read-only by name.
    pub fn is_read_only(&self, name: &str) -> Option<bool> {
        self.tools.get(name).map(|t| t.is_read_only())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_definitions_returns_only_read_only_tools() {
        let registry = ToolRegistry::with_builtins();
        let defs = registry.read_only_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        // Read, Glob, Grep are read-only; Write, Edit, Bash are not
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"Glob"));
        assert!(names.contains(&"Grep"));
        assert!(!names.contains(&"Write"));
        assert!(!names.contains(&"Edit"));
        assert!(!names.contains(&"Bash"));
        assert_eq!(defs.len(), 3);
    }
}
