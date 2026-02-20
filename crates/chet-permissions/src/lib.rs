//! Permission system and hooks for Chet.
//!
//! Permission levels: permit / block / prompt
//! Hook events: before_tool / after_tool / before_input / on_exit / on_session_start / on_session_end

pub mod engine;
pub mod hooks;
pub mod matcher;
pub mod prompt;
pub mod types;

pub use engine::PermissionEngine;
pub use hooks::run_hooks;
pub use matcher::{EvaluateResult, RuleMatcher};
pub use prompt::PromptHandler;
pub use types::*;
