//! Suggests corrected paths when a file is not found.
//!
//! Walks from the repo root (or a bounded ancestor) looking for files
//! whose basename matches the missing path's basename. Used by Read,
//! Write, and Edit tools to improve error messages when the model
//! drops the repo prefix or uses a relative path.

use std::path::Path;
use walkdir::WalkDir;

/// Maximum number of suggestions to return.
const MAX_SUGGESTIONS: usize = 5;
/// Cap directory traversal depth to avoid scanning the whole filesystem.
const MAX_DEPTH: usize = 8;

/// Try to find candidate paths for a missing file by basename match.
/// Starts from `start_dir` and walks up to find the repo root (a `.git` dir),
/// then walks down from there matching basenames.
/// Returns up to `MAX_SUGGESTIONS` paths, excluding `.git` and common build dirs.
pub fn suggest_paths(missing_path: &str, start_dir: &Path) -> Vec<String> {
    let missing = Path::new(missing_path);
    let Some(basename) = missing.file_name().and_then(|s| s.to_str()) else {
        return Vec::new();
    };

    let root = find_repo_root(start_dir).unwrap_or(start_dir);

    let mut suggestions = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| !is_excluded_dir(e.path()))
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().file_name().and_then(|s| s.to_str()) == Some(basename) {
            if let Some(path_str) = entry.path().to_str() {
                // Skip the missing path itself (already known not to exist, but be safe)
                if path_str == missing_path {
                    continue;
                }
                suggestions.push(path_str.to_string());
                if suggestions.len() >= MAX_SUGGESTIONS {
                    break;
                }
            }
        }
    }
    suggestions
}

/// Format an error message with suggestions appended.
pub fn format_not_found_error(missing_path: &str, start_dir: &Path, io_err: &str) -> String {
    let suggestions = suggest_paths(missing_path, start_dir);
    if suggestions.is_empty() {
        format!("{missing_path}: {io_err}")
    } else {
        let list = suggestions
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{missing_path}: {io_err}\n\nDid you mean:\n{list}")
    }
}

/// Walk up from `start` looking for a `.git` directory. Returns the parent of `.git`.
fn find_repo_root(start: &Path) -> Option<&Path> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        current = dir.parent();
    }
    None
}

/// Skip directories that would waste walk time.
fn is_excluded_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__" | "dist" | "build"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn suggest_finds_basename_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/foo")).unwrap();
        fs::write(root.join("src/foo/main.rs"), "").unwrap();

        let suggestions = suggest_paths("main.rs", root);
        assert_eq!(suggestions.len(), 1);
        assert!(suggestions[0].ends_with("src/foo/main.rs"));
    }

    #[test]
    fn suggest_skips_excluded_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("target/debug/foo.rs"), "").unwrap();
        fs::write(root.join("src/foo.rs"), "").unwrap();

        let suggestions = suggest_paths("foo.rs", root);
        // Only the src version, target/ is excluded
        assert_eq!(suggestions.len(), 1);
        assert!(!suggestions[0].contains("target"));
    }

    #[test]
    fn suggest_returns_empty_when_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("existing.rs"), "").unwrap();

        let suggestions = suggest_paths("nonexistent.rs", root);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn suggest_caps_at_max() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        for i in 0..10 {
            let sub = root.join(format!("dir{i}"));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join("target.rs"), "").unwrap();
        }

        let suggestions = suggest_paths("target.rs", root);
        assert_eq!(suggestions.len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn format_not_found_error_without_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let msg = format_not_found_error("missing.rs", dir.path(), "No such file");
        assert_eq!(msg, "missing.rs: No such file");
    }

    #[test]
    fn format_not_found_error_with_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/foo.rs"), "").unwrap();

        let msg = format_not_found_error("foo.rs", root, "No such file");
        assert!(msg.contains("Did you mean:"));
        assert!(msg.contains("src/foo.rs"));
    }
}
