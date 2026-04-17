//! Slash command dispatch and handlers.

use chet_permissions::PermissionEngine;
use chet_session::{MemoryManager, Session, SessionStore, compact};
use chet_terminal::StatusLine;
use chrono::Utc;
use std::sync::{Arc, Mutex};

use crate::context::CommandContext;
use crate::prompts::print_usage;

pub(crate) enum SlashResult {
    Continue,
    Break,
    Unknown,
}

pub(crate) async fn handle_slash_command(
    input: &str,
    ctx: CommandContext<'_>,
) -> Option<SlashResult> {
    let CommandContext {
        session,
        store,
        context_tracker,
        system_prompt,
        mcp_manager,
        memory_manager,
        project_id,
        status_line,
        hooks_engine,
    } = ctx;
    if !input.starts_with('/') {
        return None;
    }

    let (cmd, args) = match input.split_once(' ') {
        Some((c, a)) => (c, Some(a.trim())),
        None => (input, None),
    };

    match cmd {
        "/quit" | "/exit" => Some(SlashResult::Break),
        "/clear" => {
            *session = Session::new(session.metadata.model.clone(), session.metadata.cwd.clone());
            eprintln!("Conversation cleared. New session: {}", session.short_id());
            Some(SlashResult::Continue)
        }
        "/cost" => {
            print_usage(&session.total_usage);
            Some(SlashResult::Continue)
        }
        "/help" => {
            print_help();
            Some(SlashResult::Continue)
        }
        "/mcp" => {
            handle_mcp_command(args, mcp_manager).await;
            Some(SlashResult::Continue)
        }
        "/memory" => {
            handle_memory_command(args, memory_manager, project_id, status_line).await;
            Some(SlashResult::Continue)
        }
        "/model" => {
            let short = chet_terminal::statusline::shorten_model_name(&session.metadata.model);
            eprintln!("Current model: {} ({})", short, session.metadata.model);
            Some(SlashResult::Continue)
        }
        "/context" => {
            let info = context_tracker.estimate(&session.messages, Some(system_prompt));
            eprintln!("{}", context_tracker.format_detailed(&info));
            Some(SlashResult::Continue)
        }
        "/copy" => {
            handle_copy(&session.messages);
            Some(SlashResult::Continue)
        }
        "/compact" => {
            handle_compact(session, store, hooks_engine).await;
            Some(SlashResult::Continue)
        }
        "/sessions" => {
            handle_sessions_list(store).await;
            Some(SlashResult::Continue)
        }
        "/resume" => {
            if let Some(prefix) = args {
                handle_resume(session, store, prefix).await;
            } else {
                eprintln!("Usage: /resume <session-id-prefix>");
            }
            Some(SlashResult::Continue)
        }
        _ if input.starts_with('/') => Some(SlashResult::Unknown),
        _ => None,
    }
}

async fn handle_compact(
    session: &mut Session,
    store: &SessionStore,
    hooks_engine: &Arc<PermissionEngine>,
) {
    match compact(&session.messages, session.metadata.label.as_deref()) {
        Some(result) => {
            session.compaction_count += 1;
            let archive_path = match store
                .write_compaction_archive(
                    session.id,
                    session.compaction_count,
                    &result.archive_markdown,
                )
                .await
            {
                Ok(path) => path,
                Err(e) => {
                    eprintln!("Failed to write archive: {e}");
                    return;
                }
            };

            let removed = result.messages_removed;
            session.messages = result.new_messages;
            session.updated_at = Utc::now();

            if let Err(e) = store.save(session).await {
                eprintln!("Warning: failed to save session: {e}");
            }

            eprintln!(
                "Compacted: removed {removed} messages, {} remaining.",
                session.messages.len()
            );
            eprintln!("Archive saved to: {}", archive_path.display());

            // Fire PostCompact hook
            let hook_input = chet_permissions::HookInput {
                event: chet_permissions::HookEvent::PostCompact,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                is_error: None,
                worktree_path: None,
                worktree_source: None,
                messages_removed: Some(removed),
                messages_remaining: Some(session.messages.len()),
                config_path: None,
            };
            if let Err(msg) = hooks_engine
                .run_hooks(&chet_permissions::HookEvent::PostCompact, &hook_input)
                .await
            {
                eprintln!("Warning: post_compact hook error: {msg}");
            }
        }
        None => {
            eprintln!("Nothing to compact: conversation is too short.");
        }
    }
}

async fn handle_sessions_list(store: &SessionStore) {
    match store.list().await {
        Ok(summaries) => {
            if summaries.is_empty() {
                eprintln!("No saved sessions.");
                return;
            }
            eprintln!("Saved sessions:");
            for s in &summaries {
                let label = s.label.as_deref().unwrap_or("");
                let label_str = if label.is_empty() {
                    String::new()
                } else {
                    format!(" [{label}]")
                };
                let model_short = chet_terminal::statusline::shorten_model_name(&s.model);
                eprintln!(
                    "  {} {:>8}  {:>3} msgs  {}{}  {}",
                    s.short_id(),
                    s.age(),
                    s.message_count,
                    model_short,
                    label_str,
                    if s.preview.is_empty() {
                        "(empty)"
                    } else {
                        &s.preview
                    }
                );
            }
        }
        Err(e) => {
            eprintln!("Failed to list sessions: {e}");
        }
    }
}

async fn handle_resume(session: &mut Session, store: &SessionStore, prefix: &str) {
    match store.load_by_prefix(prefix).await {
        Ok(loaded) => {
            eprintln!(
                "Resumed session {} ({} messages)",
                loaded.short_id(),
                loaded.messages.len()
            );
            *session = loaded;
        }
        Err(e) => {
            eprintln!("Failed to resume: {e}");
        }
    }
}

async fn handle_mcp_command(args: Option<&str>, mcp_manager: &mut Option<chet_mcp::McpManager>) {
    match args {
        Some(sub) if sub.starts_with("reconnect") => {
            let server_name = sub.strip_prefix("reconnect").unwrap().trim();
            let server_name = if server_name.is_empty() {
                None
            } else {
                Some(server_name)
            };
            if let Some(manager) = mcp_manager {
                let count = manager.reconnect(server_name).await;
                eprintln!("Reconnected {count} server(s).");
            } else {
                eprintln!("No MCP servers configured.");
            }
        }
        _ => {
            // Default: show status
            match mcp_manager {
                Some(manager) if manager.client_count() > 0 => {
                    eprintln!("MCP servers ({} connected):", manager.client_count());
                    for (name, tool_count) in manager.server_summary() {
                        eprintln!("  {name}: {tool_count} tools");
                    }
                    eprintln!("\nUse /mcp reconnect [name] to reconnect servers.");
                }
                _ => {
                    eprintln!("No MCP servers connected.");
                }
            }
        }
    }
}

async fn handle_memory_command(
    args: Option<&str>,
    memory_manager: &MemoryManager,
    project_id: Option<&str>,
    status_line: &Option<Arc<Mutex<StatusLine>>>,
) {
    match args {
        None => {
            // /memory — show both
            let global = memory_manager.load_global().await;
            let project = match project_id {
                Some(id) => memory_manager.load_project(id).await,
                None => String::new(),
            };
            if global.is_empty() && project.is_empty() {
                eprintln!("No memory saved yet.");
                eprintln!("Tip: Ask the model to remember something, or use /memory edit.");
                return;
            }
            if !global.is_empty() {
                eprintln!("=== Global Memory ===");
                eprintln!("{global}");
            }
            if !project.is_empty() {
                eprintln!("=== Project Memory ===");
                eprintln!("{project}");
            }
        }
        Some(sub) if sub == "edit" || sub == "edit global" => {
            let path = memory_manager.global_memory_path();
            open_editor(&path, status_line).await;
        }
        Some("edit project") => {
            if let Some(id) = project_id {
                let path = memory_manager.project_memory_path(id);
                open_editor(&path, status_line).await;
            } else {
                eprintln!("No project context available.");
            }
        }
        Some("reset") => {
            if let Err(e) = memory_manager.reset_global().await {
                eprintln!("Failed to reset global memory: {e}");
            }
            if let Some(id) = project_id {
                if let Err(e) = memory_manager.reset_project(id).await {
                    eprintln!("Failed to reset project memory: {e}");
                }
            }
            eprintln!("Memory cleared.");
        }
        Some("reset global") => {
            if let Err(e) = memory_manager.reset_global().await {
                eprintln!("Failed to reset global memory: {e}");
            } else {
                eprintln!("Global memory cleared.");
            }
        }
        Some("reset project") => {
            if let Some(id) = project_id {
                if let Err(e) = memory_manager.reset_project(id).await {
                    eprintln!("Failed to reset project memory: {e}");
                } else {
                    eprintln!("Project memory cleared.");
                }
            } else {
                eprintln!("No project context available.");
            }
        }
        Some(other) => {
            eprintln!("Unknown /memory subcommand: {other}");
            eprintln!("Usage: /memory [edit [global|project] | reset [global|project]]");
        }
    }
}

async fn open_editor(path: &std::path::Path, status_line: &Option<Arc<Mutex<StatusLine>>>) {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            eprintln!("Failed to create directory: {e}");
            return;
        }
    }
    // Create the file if it doesn't exist
    if !path.exists() {
        if let Err(e) = tokio::fs::write(path, "").await {
            eprintln!("Failed to create file: {e}");
            return;
        }
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    // Suspend status line while editor is open
    if let Some(sl) = status_line {
        sl.lock().unwrap().suspend();
    }

    let status = std::process::Command::new(&editor).arg(path).status();

    // Resume status line
    if let Some(sl) = status_line {
        sl.lock().unwrap().resume();
    }

    match status {
        Ok(s) if s.success() => {
            eprintln!("Memory file updated: {}", path.display());
        }
        Ok(s) => {
            eprintln!("Editor exited with status: {s}");
        }
        Err(e) => {
            eprintln!("Failed to open editor '{editor}': {e}");
        }
    }
}

fn handle_copy(messages: &[chet_types::Message]) {
    // Find the last assistant text
    let text = messages.iter().rev().find_map(|m| {
        if m.role == chet_types::Role::Assistant {
            let parts: Vec<&str> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    chet_types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        } else {
            None
        }
    });

    let Some(text) = text else {
        eprintln!("No assistant response to copy.");
        return;
    };

    // Try to copy to system clipboard
    if copy_to_clipboard(&text) {
        eprintln!("Copied {} chars to clipboard.", text.len());
    } else {
        // Fallback: print to stdout so user can pipe it
        println!("{text}");
        eprintln!("(clipboard not available — printed to stdout)");
    }
}

/// Try to copy text to the system clipboard. Returns true on success.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Detect platform clipboard command
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else if cfg!(target_os = "linux") {
        // Try xclip first, then xsel
        if Command::new("xclip").arg("--version").output().is_ok() {
            ("xclip", &["-selection", "clipboard"])
        } else if Command::new("xsel").arg("--version").output().is_ok() {
            ("xsel", &["--clipboard", "--input"])
        } else {
            return false;
        }
    } else if cfg!(target_os = "windows") {
        ("clip", &[])
    } else {
        return false;
    };

    let mut child = match Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn print_help() {
    eprintln!("Available commands:");
    eprintln!("  /help     — Show this help");
    eprintln!("  /effort   — Show or set effort level (low, medium, high, auto)");
    eprintln!("  /plan     — Toggle plan mode (read-only exploration)");
    eprintln!("  /mcp      — Show connected MCP servers and tools");
    eprintln!("  /memory   — View/edit/reset persistent memory");
    eprintln!("  /copy     — Copy last response to clipboard");
    eprintln!("  /model    — Show current model");
    eprintln!("  /cost     — Show token usage");
    eprintln!("  /context  — Show detailed context window usage");
    eprintln!("  /compact  — Compact conversation (archive + summarize)");
    eprintln!("  /sessions — List saved sessions");
    eprintln!("  /resume   — Resume a saved session by ID prefix");
    eprintln!("  /clear    — Clear conversation (starts new session)");
    eprintln!("  /quit     — Exit");
    eprintln!();
    eprintln!("Flags:");
    eprintln!("  --effort <level>       — Set effort level (low, medium, high)");
    eprintln!("  --thinking-budget <N>  — Enable extended thinking (token budget)");
}
