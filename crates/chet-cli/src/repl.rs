//! Interactive REPL loop.

use anyhow::Result;
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, Session};
use chet_terminal::{
    LineEditor, ReadLineResult, SlashCommandCompleter, StatusLine, StatusLineData,
};
use chet_types::Effort;
use chrono::Utc;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::commands::{self, SlashResult};
use crate::context::{CommandContext, ReplContext, ReplStartup, UIContext};
use crate::plan::{self, PlanApproval};
use crate::prompts::{plan_system_prompt, print_usage, system_prompt, user_message};
use crate::runner::{self, create_agent};

pub(crate) async fn repl(ctx: ReplContext<'_>, startup: ReplStartup) -> Result<()> {
    let ReplContext {
        provider,
        permissions,
        config,
        cwd,
        original_cwd,
        mut mcp_manager,
        memory_manager,
        stderr_is_tty,
        project_id,
    } = ctx;
    let hooks_engine = Arc::clone(&permissions);
    let mut agent = create_agent(
        provider,
        permissions,
        config,
        cwd,
        &mcp_manager,
        project_id.clone(),
    );
    let store = chet_session::SessionStore::new(config.config_dir.clone())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let context_tracker = ContextTracker::new(&config.model);
    let mut memory_section = memory_manager.load_combined(project_id.as_deref()).await;
    let mut system = system_prompt(cwd, &memory_section);
    agent.set_system_prompt(system.clone());

    // Fire InstructionsLoaded hook
    let _ = hooks_engine
        .run_hooks(
            &chet_permissions::HookEvent::InstructionsLoaded,
            &chet_permissions::HookInput {
                event: chet_permissions::HookEvent::InstructionsLoaded,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                is_error: None,
                worktree_path: None,
                worktree_source: None,
                messages_removed: None,
                messages_remaining: None,
                config_path: None,
            },
        )
        .await;

    // Spawn background config file watcher (fires ConfigChange hook on mtime changes)
    let config_watcher_handle = spawn_config_watcher(
        Arc::clone(&hooks_engine),
        config.config_dir.join("config.toml"),
        cwd.join(".chet").join("config.toml"),
    );

    // Load or create session
    let mut session = match &startup.resume_id {
        Some(prefix) => {
            let s = store
                .load_by_prefix(prefix)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eprintln!("Resumed session {}", s.short_id());
            s
        }
        None => Session::new(config.model.clone(), cwd.display().to_string()),
    };

    // Apply --name flag (overrides auto-label, even on resumed sessions)
    if let Some(name) = startup.session_name {
        session.metadata.label = Some(name);
    }

    let mut editor = LineEditor::new(config.config_dir.join("history"));
    editor.set_completer(Box::new(SlashCommandCompleter::new(vec![
        "/quit",
        "/exit",
        "/clear",
        "/copy",
        "/cost",
        "/effort",
        "/help",
        "/mcp",
        "/memory",
        "/model",
        "/context",
        "/compact",
        "/sessions",
        "/resume",
        "/plan",
    ])));

    let thinking_info = match (config.effort, config.thinking_budget) {
        (_, Some(budget)) => format!(", thinking: {budget} tokens"),
        (Some(effort), None) => format!(", effort: {effort}"),
        (None, None) => String::new(),
    };
    let mcp_info = match &mcp_manager {
        Some(m) if m.client_count() > 0 => {
            let tool_count: usize = m.server_summary().iter().map(|(_, c)| c).sum();
            format!(", mcp: {} servers/{} tools", m.client_count(), tool_count)
        }
        _ => String::new(),
    };
    eprintln!(
        "chet v{} (model: {}{}{}, session: {})",
        env!("CARGO_PKG_VERSION"),
        config.model,
        thinking_info,
        mcp_info,
        session.short_id()
    );
    eprintln!("Type your message. Press Ctrl+D to exit.\n");

    // Set terminal title (TTY only)
    if stderr_is_tty {
        set_terminal_title(&format!("chet — {}", session.short_id()));
    }

    // Create status line (TTY only)
    let status_line: Option<Arc<Mutex<StatusLine>>> = if stderr_is_tty {
        let context_window_k = ContextTracker::new(&config.model)
            .estimate(&[], None)
            .context_window as f64
            / 1000.0;
        let data = StatusLineData {
            model: config.model.clone(),
            session_id: session.short_id(),
            context_tokens_k: 0.0,
            context_window_k,
            context_percent: 0.0,
            input_tokens: session.total_usage.input_tokens,
            output_tokens: session.total_usage.output_tokens,
            effort: config.effort,
            plan_mode: false,
            active_tool: None,
        };
        let mut sl = StatusLine::new(data);
        sl.install();
        Some(Arc::new(Mutex::new(sl)))
    } else {
        None
    };

    let mut plan_mode = false;
    let mut auto_compact_failures: u32 = 0;
    const AUTO_COMPACT_THRESHOLD: f64 = 80.0;
    const AUTO_COMPACT_MAX_FAILURES: u32 = 3;

    loop {
        let prompt = if plan_mode { "plan> " } else { "> " };

        // Suspend status line before line editor
        if let Some(sl) = &status_line {
            sl.lock().unwrap().suspend();
        }

        let input = match editor.read_line(prompt).await? {
            ReadLineResult::Line(line) => {
                // Resume status line after line editor returns
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().resume();
                }
                line
            }
            ReadLineResult::Eof => {
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().resume();
                }
                eprintln!();
                break;
            }
            ReadLineResult::Interrupted => {
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().resume();
                }
                continue;
            }
        };

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Handle /plan toggle before other slash commands
        if input == "/plan" || input.starts_with("/plan ") {
            let plan_desc = input.strip_prefix("/plan").unwrap().trim();
            if plan_mode && plan_desc.is_empty() {
                // Exit plan mode
                plan_mode = false;
                agent.set_read_only_mode(false);
                agent.set_system_prompt(system_prompt(cwd, &memory_section));
                eprintln!("Exited plan mode.");
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().update_field(|d| d.plan_mode = false);
                }
                continue;
            }
            if !plan_mode {
                // Enter plan mode
                plan_mode = true;
                agent.set_read_only_mode(true);
                agent.set_system_prompt(plan_system_prompt(cwd, &memory_section));
                eprintln!("{}", chet_terminal::style::plan_mode_banner(stderr_is_tty));
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().update_field(|d| d.plan_mode = true);
                }
            }
            if plan_desc.is_empty() {
                continue;
            }
            // Fall through to send plan_desc as a message
            session.messages.push(user_message(plan_desc));
        }

        // Handle /effort inline (needs mutable access to agent)
        if input.starts_with("/effort") {
            let arg = input.strip_prefix("/effort").unwrap().trim();
            if arg.is_empty() {
                match agent.effort() {
                    Some(e) => eprintln!("Current effort: {e}"),
                    None => eprintln!("No effort level set (using default)."),
                }
            } else if arg == "auto" {
                agent.set_effort(None);
                eprintln!("Effort reset to default (auto).");
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().update_field(|d| d.effort = None);
                }
            } else {
                match arg.parse::<Effort>() {
                    Ok(e) => {
                        agent.set_effort(Some(e));
                        eprintln!("Effort set to: {e}");
                        if let Some(sl) = &status_line {
                            sl.lock().unwrap().update_field(|d| d.effort = Some(e));
                        }
                    }
                    Err(msg) => eprintln!("{msg}"),
                }
            }
            continue;
        }

        // Handle /worktree exit
        if input == "/worktree exit" || input == "/worktree" {
            if let Some(ref orig) = original_cwd {
                agent.set_cwd(orig.clone());
                eprintln!("Exited worktree. CWD restored to: {}", orig.display());
            } else {
                eprintln!("Not running in a worktree.");
            }
            continue;
        }

        // Handle slash commands
        if let Some(handled) = commands::handle_slash_command(
            input,
            CommandContext {
                session: &mut session,
                store: &store,
                context_tracker: &context_tracker,
                system_prompt: &system,
                mcp_manager: &mut mcp_manager,
                memory_manager: &memory_manager,
                project_id: project_id.as_deref(),
                status_line: &status_line,
                hooks_engine: &hooks_engine,
            },
        )
        .await
        {
            // Update status line for commands that change session state
            if input == "/clear" || input.starts_with("/resume") {
                if let Some(sl) = &status_line {
                    let info = context_tracker.estimate(&session.messages, Some(&system));
                    sl.lock().unwrap().update_field(|d| {
                        d.session_id = session.short_id();
                        d.context_tokens_k = info.estimated_tokens as f64 / 1000.0;
                        d.context_percent = info.usage_percent();
                        d.input_tokens = session.total_usage.input_tokens;
                        d.output_tokens = session.total_usage.output_tokens;
                    });
                }
            }
            match handled {
                SlashResult::Continue => continue,
                SlashResult::Break => break,
                SlashResult::Unknown => {
                    eprintln!("Unknown command: {input}. Type /help for available commands.");
                    continue;
                }
            }
        }

        // Push user message if not already pushed by /plan <desc>
        if !input.starts_with("/plan ") {
            session.messages.push(user_message(input));
        }

        match runner::run_agent(
            &agent,
            &mut session.messages,
            UIContext {
                stdout_is_tty: true,
                stderr_is_tty,
                status_line: status_line.clone(),
            },
        )
        .await
        {
            Ok(usage) => {
                session.total_usage.add(&usage);
                session.updated_at = Utc::now();
                session.auto_label();

                // Update terminal title with session label
                if stderr_is_tty {
                    if let Some(label) = &session.metadata.label {
                        set_terminal_title(&format!("chet — {label}"));
                    }
                }

                // In plan mode, save plan to file and prompt for approval
                if plan_mode {
                    if let Some(plan_text) = plan::extract_last_assistant_text(&session.messages) {
                        let plan_path =
                            plan::save_plan_file(&config.config_dir, &session, &plan_text).await;
                        if let Some(path) = plan_path {
                            eprintln!("\nPlan saved to {}", path.display());
                        }
                    }

                    eprintln!();
                    eprintln!("Plan complete. What would you like to do?");
                    eprintln!("  [a]pprove — exit plan mode, keep plan as context");
                    eprintln!("  [r]efine  — stay in plan mode, provide refinements");
                    eprintln!("  [d]iscard — discard plan and exit plan mode");

                    match plan::prompt_plan_approval().await {
                        PlanApproval::Approve => {
                            // Name session from plan content if not already named
                            if session.metadata.label.is_none() {
                                if let Some(plan_text) =
                                    plan::extract_last_assistant_text(&session.messages)
                                {
                                    if let Some(label) = plan::label_from_plan(&plan_text) {
                                        session.metadata.label = Some(label);
                                    }
                                }
                            }
                            plan_mode = false;
                            agent.set_read_only_mode(false);
                            agent.set_system_prompt(system_prompt(cwd, &memory_section));
                            eprintln!("Plan approved. Exiting plan mode.");
                        }
                        PlanApproval::Refine => {
                            eprintln!("Staying in plan mode. Provide your refinements.");
                        }
                        PlanApproval::Discard => {
                            plan::pop_last_turn(&mut session.messages);
                            plan_mode = false;
                            agent.set_read_only_mode(false);
                            agent.set_system_prompt(system_prompt(cwd, &memory_section));
                            eprintln!("Plan discarded. Exiting plan mode.");
                        }
                    }
                    if let Some(sl) = &status_line {
                        sl.lock().unwrap().update_field(|d| d.plan_mode = plan_mode);
                    }
                }

                // Auto-save
                if let Err(e) = store.save(&session).await {
                    eprintln!("Warning: failed to save session: {e}");
                }

                // Refresh memory if it changed (model may have called MemoryWrite)
                let new_memory = memory_manager.load_combined(project_id.as_deref()).await;
                if new_memory != memory_section {
                    memory_section = new_memory;
                    system = if plan_mode {
                        plan_system_prompt(cwd, &memory_section)
                    } else {
                        system_prompt(cwd, &memory_section)
                    };
                    agent.set_system_prompt(system.clone());
                }

                // Update status line with latest context info
                let info = context_tracker.estimate(&session.messages, Some(&system));
                if let Some(sl) = &status_line {
                    sl.lock().unwrap().update_field(|d| {
                        d.context_tokens_k = info.estimated_tokens as f64 / 1000.0;
                        d.context_percent = info.usage_percent();
                        d.input_tokens = session.total_usage.input_tokens;
                        d.output_tokens = session.total_usage.output_tokens;
                        d.plan_mode = plan_mode;
                        d.effort = agent.effort();
                    });
                } else {
                    // Non-TTY: print brief context line
                    eprintln!("{}", context_tracker.format_brief(&info));
                }

                // Auto-compact when context usage is high
                if info.usage_percent() > AUTO_COMPACT_THRESHOLD
                    && auto_compact_failures < AUTO_COMPACT_MAX_FAILURES
                {
                    if let Some(result) =
                        chet_session::compact(&session.messages, session.metadata.label.as_deref())
                    {
                        let removed = result.messages_removed;
                        session.compaction_count += 1;
                        session.messages = result.new_messages;
                        session.updated_at = Utc::now();
                        if let Err(e) = store
                            .write_compaction_archive(
                                session.id,
                                session.compaction_count,
                                &result.archive_markdown,
                            )
                            .await
                        {
                            tracing::warn!("Failed to write compaction archive: {e}");
                        }
                        if let Err(e) = store.save(&session).await {
                            tracing::warn!("Failed to save after auto-compact: {e}");
                        }
                        eprintln!(
                            "(auto-compacted: removed {removed} messages, {} remaining)",
                            session.messages.len()
                        );
                        auto_compact_failures = 0;
                    } else {
                        auto_compact_failures += 1;
                        if auto_compact_failures >= AUTO_COMPACT_MAX_FAILURES {
                            eprintln!(
                                "Warning: auto-compaction disabled after {AUTO_COMPACT_MAX_FAILURES} consecutive failures."
                            );
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("\nError: {e}");
                // Remove the failed user message
                session.messages.pop();
            }
        }

        println!();
    }

    // Tear down status line before exiting
    if let Some(sl) = &status_line {
        sl.lock().unwrap().teardown();
    }

    editor.save_history()?;

    // Final save
    session.updated_at = Utc::now();
    if let Err(e) = store.save(&session).await {
        eprintln!("Warning: failed to save session: {e}");
    }

    // Shut down MCP servers
    if let Some(manager) = mcp_manager {
        manager.shutdown().await;
    }

    // Stop config watcher
    config_watcher_handle.abort();

    // Reset terminal title
    if stderr_is_tty {
        set_terminal_title("Terminal");
    }

    print_usage(&session.total_usage);
    Ok(())
}

/// Set the terminal title via OSC escape sequence.
fn set_terminal_title(title: &str) {
    let _ = write!(std::io::stderr(), "\x1b]0;{title}\x07");
    let _ = std::io::stderr().flush();
}

/// Spawn a background task that polls config file mtimes every 5 seconds
/// and fires ConfigChange hook when one of them changes.
fn spawn_config_watcher(
    hooks_engine: Arc<PermissionEngine>,
    global_path: std::path::PathBuf,
    project_path: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut global_mtime = mtime_of(&global_path);
        let mut project_mtime = mtime_of(&project_path);
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        interval.tick().await; // skip the initial tick

        loop {
            interval.tick().await;

            let new_global = mtime_of(&global_path);
            if new_global != global_mtime {
                global_mtime = new_global;
                fire_config_change(&hooks_engine, &global_path).await;
            }

            let new_project = mtime_of(&project_path);
            if new_project != project_mtime {
                project_mtime = new_project;
                fire_config_change(&hooks_engine, &project_path).await;
            }
        }
    })
}

/// Get file mtime as `Option<SystemTime>`. Returns None if file doesn't exist.
fn mtime_of(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Fire ConfigChange hook with the changed file path.
async fn fire_config_change(hooks_engine: &Arc<PermissionEngine>, path: &std::path::Path) {
    let hook_input = chet_permissions::HookInput {
        event: chet_permissions::HookEvent::ConfigChange,
        tool_name: None,
        tool_input: None,
        tool_output: None,
        is_error: None,
        worktree_path: None,
        worktree_source: None,
        messages_removed: None,
        messages_remaining: None,
        config_path: Some(path.display().to_string()),
    };
    if let Err(msg) = hooks_engine
        .run_hooks(&chet_permissions::HookEvent::ConfigChange, &hook_input)
        .await
    {
        tracing::warn!("config_change hook error: {msg}");
    }
}
