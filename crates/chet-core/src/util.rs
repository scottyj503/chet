//! Internal utility functions for the agent loop.

use std::path::{Path, PathBuf};

/// Truncate a string for display, adding "..." if truncated.
pub(crate) fn truncate_for_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", chet_types::truncate_str(s, max_len))
    }
}

/// Persist a large tool result to a temp file under the CWD.
/// Returns the path on success, None on failure.
pub(crate) fn persist_tool_result(
    cwd: &Path,
    tool_name: &str,
    tool_id: &str,
    text: &str,
) -> Option<PathBuf> {
    let dir = cwd.join(".chet-tool-output");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Failed to create tool output dir: {e}");
        return None;
    }
    // Short ID to avoid long filenames
    let short_id = if tool_id.len() > 8 {
        &tool_id[..8]
    } else {
        tool_id
    };
    let filename = format!("{tool_name}-{short_id}.txt");
    let path = dir.join(&filename);
    match std::fs::write(&path, text) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!("Failed to persist tool result: {e}");
            None
        }
    }
}
