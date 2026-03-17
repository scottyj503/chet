//! Persistent memory storage for Chet — global and per-project markdown files.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// Manages persistent memory files (global + per-project).
pub struct MemoryManager {
    config_dir: PathBuf,
}

impl MemoryManager {
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    /// Compute a deterministic 16-char hex project ID from a path.
    pub fn project_id(path: &Path) -> String {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Path to the global memory file.
    pub fn global_memory_path(&self) -> PathBuf {
        self.config_dir.join("memory").join("MEMORY.md")
    }

    /// Path to the project-specific memory file.
    pub fn project_memory_path(&self, project_id: &str) -> PathBuf {
        self.config_dir
            .join("memory")
            .join("projects")
            .join(format!("{project_id}.md"))
    }

    /// Load global memory, returning empty string if file is missing.
    pub async fn load_global(&self) -> String {
        let path = self.global_memory_path();
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    }

    /// Load project memory, returning empty string if file is missing.
    pub async fn load_project(&self, project_id: &str) -> String {
        let path = self.project_memory_path(project_id);
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    }

    /// Load and format both global and project memory into a single section.
    pub async fn load_combined(&self, project_id: Option<&str>) -> String {
        let global = self.load_global().await;
        let project = match project_id {
            Some(id) => self.load_project(id).await,
            None => String::new(),
        };
        format_memory_section(&global, &project)
    }

    /// Write global memory atomically (tmp file + rename).
    pub async fn write_global(&self, content: &str) -> io::Result<()> {
        let path = self.global_memory_path();
        atomic_write(&path, content).await
    }

    /// Write project memory atomically.
    pub async fn write_project(&self, project_id: &str, content: &str) -> io::Result<()> {
        let path = self.project_memory_path(project_id);
        atomic_write(&path, content).await
    }

    /// Delete the global memory file.
    pub async fn reset_global(&self) -> io::Result<()> {
        let path = self.global_memory_path();
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Delete a project memory file.
    pub async fn reset_project(&self, project_id: &str) -> io::Result<()> {
        let path = self.project_memory_path(project_id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Format global and project memory into a combined section.
/// Returns empty string if both are empty.
pub fn format_memory_section(global: &str, project: &str) -> String {
    let global = global.trim();
    let project = project.trim();

    if global.is_empty() && project.is_empty() {
        return String::new();
    }

    let mut out = String::from("# Memory\n\n");

    if !global.is_empty() {
        out.push_str("## Global Memory\n\n");
        out.push_str(global);
        out.push_str("\n\n");
    }

    if !project.is_empty() {
        out.push_str("## Project Memory\n\n");
        out.push_str(project);
        out.push('\n');
    }

    out
}

/// Write content to a file atomically via a temporary file + rename.
async fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn project_id_deterministic() {
        let p = Path::new("/home/user/myproject");
        assert_eq!(MemoryManager::project_id(p), MemoryManager::project_id(p));
    }

    #[test]
    fn project_id_different_paths() {
        let a = MemoryManager::project_id(Path::new("/home/user/a"));
        let b = MemoryManager::project_id(Path::new("/home/user/b"));
        assert_ne!(a, b);
    }

    #[test]
    fn format_memory_section_both() {
        let result = format_memory_section("global stuff", "project stuff");
        assert!(result.contains("# Memory"));
        assert!(result.contains("## Global Memory"));
        assert!(result.contains("global stuff"));
        assert!(result.contains("## Project Memory"));
        assert!(result.contains("project stuff"));
    }

    #[test]
    fn format_memory_section_global_only() {
        let result = format_memory_section("global stuff", "");
        assert!(result.contains("## Global Memory"));
        assert!(!result.contains("## Project Memory"));
    }

    #[test]
    fn format_memory_section_project_only() {
        let result = format_memory_section("", "project stuff");
        assert!(!result.contains("## Global Memory"));
        assert!(result.contains("## Project Memory"));
    }

    #[test]
    fn format_memory_section_empty() {
        let result = format_memory_section("", "");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn write_and_load_global() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("hello world").await.unwrap();
        let loaded = mgr.load_global().await;
        assert_eq!(loaded, "hello world");
    }

    #[tokio::test]
    async fn write_and_load_project() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_project("abc123", "project data").await.unwrap();
        let loaded = mgr.load_project("abc123").await;
        assert_eq!(loaded, "project data");
    }

    #[tokio::test]
    async fn load_missing_returns_empty() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        assert_eq!(mgr.load_global().await, "");
        assert_eq!(mgr.load_project("nonexistent").await, "");
    }

    #[tokio::test]
    async fn reset_global() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("data").await.unwrap();
        assert!(!mgr.load_global().await.is_empty());
        mgr.reset_global().await.unwrap();
        assert!(mgr.load_global().await.is_empty());
    }

    #[tokio::test]
    async fn reset_project() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_project("abc", "data").await.unwrap();
        assert!(!mgr.load_project("abc").await.is_empty());
        mgr.reset_project("abc").await.unwrap();
        assert!(mgr.load_project("abc").await.is_empty());
    }

    #[tokio::test]
    async fn reset_missing_is_ok() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        // Should not error when file doesn't exist
        mgr.reset_global().await.unwrap();
        mgr.reset_project("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn load_combined_both() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("global content").await.unwrap();
        mgr.write_project("proj1", "project content").await.unwrap();
        let combined = mgr.load_combined(Some("proj1")).await;
        assert!(combined.contains("global content"));
        assert!(combined.contains("project content"));
    }

    #[tokio::test]
    async fn load_combined_no_project_id() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("global only").await.unwrap();
        let combined = mgr.load_combined(None).await;
        assert!(combined.contains("global only"));
        assert!(!combined.contains("## Project Memory"));
    }
}
