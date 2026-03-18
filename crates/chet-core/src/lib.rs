//! Agent loop orchestration and conversation management for Chet.

mod agent;
mod subagent;
mod util;
pub mod worktree;

pub use agent::{Agent, AgentEvent};
pub use subagent::SubagentTool;
pub use worktree::{ManagedWorktree, WorktreeError, create_worktree, is_git_repo};
