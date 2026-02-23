//! Git worktree isolation for parallel agent execution.
//!
//! Provides `ManagedWorktree` — a RAII wrapper around `git worktree add/remove`
//! that ensures cleanup on drop. Used for `--worktree` CLI flag (session-level)
//! and `isolation: "worktree"` in SubagentTool (per-task).

use chet_permissions::{HookEvent, HookInput, PermissionEngine};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Errors that can occur during worktree operations.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("not a git repository: {path}")]
    NotGitRepo { path: String },
    #[error("git not found on PATH")]
    GitNotFound,
    #[error("failed to create worktree: {message}")]
    CreateFailed { message: String },
    #[error("failed to remove worktree: {message}")]
    RemoveFailed { message: String },
}

/// A managed git worktree that cleans up on drop.
///
/// Prefer calling `cleanup()` explicitly for async hook support.
/// The `Drop` impl is a synchronous safety net (no hooks).
pub struct ManagedWorktree {
    path: PathBuf,
    source: PathBuf,
    permissions: Option<Arc<PermissionEngine>>,
    cleaned_up: bool,
}

impl std::fmt::Debug for ManagedWorktree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedWorktree")
            .field("path", &self.path)
            .field("source", &self.source)
            .field("cleaned_up", &self.cleaned_up)
            .finish()
    }
}

impl ManagedWorktree {
    /// Path to the worktree directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path to the source repository.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Explicitly clean up the worktree (async, runs hooks).
    ///
    /// Runs `WorktreeRemove` hooks (best-effort), then `git worktree remove --force`.
    /// Sets `cleaned_up = true` to prevent the `Drop` safety net from firing.
    pub async fn cleanup(&mut self) -> Result<(), WorktreeError> {
        if self.cleaned_up {
            return Ok(());
        }
        self.cleaned_up = true;

        // Run WorktreeRemove hooks (best-effort)
        if let Some(ref permissions) = self.permissions {
            let hook_input = HookInput {
                event: HookEvent::WorktreeRemove,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                is_error: None,
                worktree_path: Some(self.path.display().to_string()),
                worktree_source: Some(self.source.display().to_string()),
            };
            if let Err(msg) = permissions
                .run_hooks(&HookEvent::WorktreeRemove, &hook_input)
                .await
            {
                tracing::warn!("WorktreeRemove hook error: {msg}");
            }
        }

        // Remove the worktree
        let output = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.source)
            .output()
            .await
            .map_err(|e| WorktreeError::RemoveFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WorktreeError::RemoveFailed {
                message: stderr.trim().to_string(),
            });
        }

        Ok(())
    }

    /// Synchronous best-effort cleanup for Drop/panic contexts.
    /// Does NOT run hooks (can't run async in Drop).
    fn cleanup_sync(&self) {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.source)
            .output();
    }
}

impl Drop for ManagedWorktree {
    fn drop(&mut self) {
        if !self.cleaned_up {
            self.cleanup_sync();
        }
    }
}

/// Check if the given path is inside a git repository.
pub async fn is_git_repo(path: &Path) -> bool {
    tokio::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get the root directory of the git repository containing `path`.
pub async fn git_repo_root(path: &Path) -> Result<PathBuf, WorktreeError> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .await
        .map_err(|_| WorktreeError::GitNotFound)?;

    if !output.status.success() {
        return Err(WorktreeError::NotGitRepo {
            path: path.display().to_string(),
        });
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

/// Create a new git worktree from the given source repository.
///
/// - `source`: path inside a git repository (will resolve to repo root)
/// - `branch`: optional branch name to create (uses `--detach` if None)
/// - `permissions`: optional permission engine for running hooks
///
/// Returns a `ManagedWorktree` that cleans up on drop.
pub async fn create_worktree(
    source: &Path,
    branch: Option<&str>,
    permissions: Option<Arc<PermissionEngine>>,
) -> Result<ManagedWorktree, WorktreeError> {
    // Validate git is available
    if tokio::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_err()
    {
        return Err(WorktreeError::GitNotFound);
    }

    // Validate source is a git repo
    if !is_git_repo(source).await {
        return Err(WorktreeError::NotGitRepo {
            path: source.display().to_string(),
        });
    }

    let repo_root = git_repo_root(source).await?;

    // Generate unique worktree path in temp dir
    let id = &uuid::Uuid::new_v4().to_string()[..8];
    let worktree_path = std::env::temp_dir().join(format!("chet-worktree-{id}"));

    // Create the worktree
    let output = if let Some(branch) = branch {
        tokio::process::Command::new("git")
            .args(["worktree", "add", "-b", branch])
            .arg(&worktree_path)
            .current_dir(&repo_root)
            .output()
            .await
    } else {
        tokio::process::Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&worktree_path)
            .current_dir(&repo_root)
            .output()
            .await
    };

    let output = output.map_err(|e| WorktreeError::CreateFailed {
        message: e.to_string(),
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::CreateFailed {
            message: stderr.trim().to_string(),
        });
    }

    let managed = ManagedWorktree {
        path: worktree_path,
        source: repo_root.clone(),
        permissions: permissions.clone(),
        cleaned_up: false,
    };

    // Run WorktreeCreate hooks
    if let Some(ref permissions) = permissions {
        let hook_input = HookInput {
            event: HookEvent::WorktreeCreate,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            is_error: None,
            worktree_path: Some(managed.path.display().to_string()),
            worktree_source: Some(repo_root.display().to_string()),
        };
        if let Err(reason) = permissions
            .run_hooks(&HookEvent::WorktreeCreate, &hook_input)
            .await
        {
            // Hook denied — clean up the worktree we just created
            drop(managed);
            return Err(WorktreeError::CreateFailed {
                message: format!("WorktreeCreate hook denied: {reason}"),
            });
        }
    }

    Ok(managed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn is_git_repo_true_for_this_repo() {
        // This test runs from the workspace root which is a git repo
        let cwd = std::env::current_dir().unwrap();
        assert!(is_git_repo(&cwd).await);
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn is_git_repo_false_for_tmp() {
        assert!(!is_git_repo(&std::env::temp_dir()).await);
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn create_and_cleanup_worktree() {
        let cwd = std::env::current_dir().unwrap();
        let mut wt = create_worktree(&cwd, None, None).await.unwrap();
        let wt_path = wt.path().to_path_buf();

        // Worktree directory should exist
        assert!(wt_path.exists());
        assert!(wt_path.join(".git").exists());

        // Cleanup should remove it
        wt.cleanup().await.unwrap();
        assert!(!wt_path.exists());
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn create_worktree_non_git_dir_fails() {
        let result = create_worktree(&std::env::temp_dir(), None, None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            WorktreeError::NotGitRepo { .. } => {}
            other => panic!("expected NotGitRepo, got: {other}"),
        }
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn create_worktree_with_branch() {
        let cwd = std::env::current_dir().unwrap();
        let branch_name = format!("test-wt-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let mut wt = create_worktree(&cwd, Some(&branch_name), None)
            .await
            .unwrap();
        let wt_path = wt.path().to_path_buf();

        assert!(wt_path.exists());

        // Verify the branch was created
        let output = tokio::process::Command::new("git")
            .args(["branch", "--list", &branch_name])
            .current_dir(&cwd)
            .output()
            .await
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(branches.contains(&branch_name));

        wt.cleanup().await.unwrap();
        assert!(!wt_path.exists());

        // Clean up the branch
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", &branch_name])
            .current_dir(&cwd)
            .output()
            .await;
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn drop_triggers_cleanup() {
        let cwd = std::env::current_dir().unwrap();
        let wt_path;
        {
            let wt = create_worktree(&cwd, None, None).await.unwrap();
            wt_path = wt.path().to_path_buf();
            assert!(wt_path.exists());
            // wt is dropped here
        }
        // Give a moment for the sync cleanup in Drop
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!wt_path.exists());
    }

    #[tokio::test]
    #[ignore] // requires git + filesystem
    async fn git_repo_root_returns_toplevel() {
        let cwd = std::env::current_dir().unwrap();
        let root = git_repo_root(&cwd).await.unwrap();
        // Root should be a parent of or equal to cwd
        assert!(cwd.starts_with(&root) || cwd == root);
    }

    #[test]
    fn worktree_error_display() {
        let e = WorktreeError::NotGitRepo {
            path: "/tmp".to_string(),
        };
        assert_eq!(e.to_string(), "not a git repository: /tmp");

        let e = WorktreeError::GitNotFound;
        assert_eq!(e.to_string(), "git not found on PATH");

        let e = WorktreeError::CreateFailed {
            message: "oops".to_string(),
        };
        assert_eq!(e.to_string(), "failed to create worktree: oops");

        let e = WorktreeError::RemoveFailed {
            message: "nope".to_string(),
        };
        assert_eq!(e.to_string(), "failed to remove worktree: nope");
    }
}
