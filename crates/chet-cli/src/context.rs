//! Context structs that bundle related parameters for CLI entry points.

use chet_config::ChetConfig;
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, MemoryManager, Session, SessionStore};
use chet_terminal::StatusLine;
use chet_types::provider::Provider;
use std::sync::{Arc, Mutex};

/// Terminal output context for `run_agent()`.
pub(crate) struct UIContext {
    pub stdout_is_tty: bool,
    pub stderr_is_tty: bool,
    pub status_line: Option<Arc<Mutex<StatusLine>>>,
}

/// Bundled parameters for `handle_slash_command()`.
pub(crate) struct CommandContext<'a> {
    pub session: &'a mut Session,
    pub store: &'a SessionStore,
    pub context_tracker: &'a ContextTracker,
    pub system_prompt: &'a str,
    pub mcp_manager: &'a mut Option<chet_mcp::McpManager>,
    pub memory_manager: &'a MemoryManager,
    pub project_id: Option<&'a str>,
    pub status_line: &'a Option<Arc<Mutex<StatusLine>>>,
    pub hooks_engine: &'a Arc<PermissionEngine>,
}

/// Long-lived state for the REPL loop.
pub(crate) struct ReplContext<'a> {
    pub provider: Arc<dyn Provider>,
    pub permissions: Arc<PermissionEngine>,
    pub config: &'a ChetConfig,
    pub cwd: &'a std::path::Path,
    pub original_cwd: Option<std::path::PathBuf>,
    pub mcp_manager: Option<chet_mcp::McpManager>,
    pub memory_manager: MemoryManager,
    pub stderr_is_tty: bool,
    pub project_id: Option<String>,
}

/// One-time startup options for REPL session initialization.
pub(crate) struct ReplStartup {
    pub resume_id: Option<String>,
    pub session_name: Option<String>,
}
