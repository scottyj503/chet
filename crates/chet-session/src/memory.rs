//! Persistent memory storage for Chet — global and per-project markdown files.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// Manages persistent memory files (global + per-project).
pub struct MemoryManager {
    memory_dir: PathBuf,
}

impl MemoryManager {
    /// Create a new MemoryManager with the given memory directory.
    /// The directory should be the resolved memory path (e.g. `~/.chet/memory/`
    /// or a custom path from the `memory_dir` config setting).
    pub fn new(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }

    /// Compute a deterministic 16-char hex project ID from a path.
    pub fn project_id(path: &Path) -> String {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Path to the global memory file.
    pub fn global_memory_path(&self) -> PathBuf {
        self.memory_dir.join("MEMORY.md")
    }

    /// Path to the project-specific memory file.
    pub fn project_memory_path(&self, project_id: &str) -> PathBuf {
        self.memory_dir
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
    /// Includes last-modified timestamps when files exist.
    pub async fn load_combined(&self, project_id: Option<&str>) -> String {
        let global = self.load_global().await;
        let global_modified = file_modified_label(&self.global_memory_path()).await;
        let project = match project_id {
            Some(id) => self.load_project(id).await,
            None => String::new(),
        };
        let project_modified = match project_id {
            Some(id) => file_modified_label(&self.project_memory_path(id)).await,
            None => None,
        };
        format_memory_section(
            &global,
            global_modified.as_deref(),
            &project,
            project_modified.as_deref(),
        )
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
/// Optional timestamps are shown as "(last updated: ...)" after section headings.
pub fn format_memory_section(
    global: &str,
    global_modified: Option<&str>,
    project: &str,
    project_modified: Option<&str>,
) -> String {
    let global = global.trim();
    let project = project.trim();

    if global.is_empty() && project.is_empty() {
        return String::new();
    }

    let mut out = String::from("# Memory\n\n");

    if !global.is_empty() {
        match global_modified {
            Some(ts) => out.push_str(&format!("## Global Memory (last updated: {ts})\n\n")),
            None => out.push_str("## Global Memory\n\n"),
        }
        out.push_str(global);
        out.push_str("\n\n");
    }

    if !project.is_empty() {
        match project_modified {
            Some(ts) => out.push_str(&format!("## Project Memory (last updated: {ts})\n\n")),
            None => out.push_str("## Project Memory\n\n"),
        }
        out.push_str(project);
        out.push('\n');
    }

    out
}

/// Get a human-readable last-modified label for a file, or None if the file doesn't exist.
async fn file_modified_label(path: &Path) -> Option<String> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let modified = meta.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = modified.into();
    Some(dt.format("%Y-%m-%d %H:%M UTC").to_string())
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
        let result = format_memory_section("global stuff", None, "project stuff", None);
        assert!(result.contains("# Memory"));
        assert!(result.contains("## Global Memory"));
        assert!(result.contains("global stuff"));
        assert!(result.contains("## Project Memory"));
        assert!(result.contains("project stuff"));
    }

    #[test]
    fn format_memory_section_global_only() {
        let result = format_memory_section("global stuff", None, "", None);
        assert!(result.contains("## Global Memory"));
        assert!(!result.contains("## Project Memory"));
    }

    #[test]
    fn format_memory_section_project_only() {
        let result = format_memory_section("", None, "project stuff", None);
        assert!(!result.contains("## Global Memory"));
        assert!(result.contains("## Project Memory"));
    }

    #[test]
    fn format_memory_section_empty() {
        let result = format_memory_section("", None, "", None);
        assert!(result.is_empty());
    }

    #[test]
    fn format_memory_section_with_timestamps() {
        let result = format_memory_section(
            "global",
            Some("2026-03-16 12:00 UTC"),
            "project",
            Some("2026-03-15 08:30 UTC"),
        );
        assert!(result.contains("last updated: 2026-03-16 12:00 UTC"));
        assert!(result.contains("last updated: 2026-03-15 08:30 UTC"));
    }

    #[tokio::test]
    async fn load_combined_includes_timestamps() {
        let dir = TempDir::new().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());
        mgr.write_global("some memory").await.unwrap();
        let combined = mgr.load_combined(None).await;
        assert!(combined.contains("last updated:"));
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
