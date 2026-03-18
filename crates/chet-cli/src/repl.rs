//! Interactive REPL loop.

use anyhow::Result;
use chet_config::ChetConfig;
use chet_mcp::McpManager;
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, MemoryManager, Session};
use chet_terminal::{
    LineEditor, ReadLineResult, SlashCommandCompleter, StatusLine, StatusLineData,
};
use chet_types::{Effort, provider::Provider};
use chrono::Utc;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::commands::{self, SlashResult};
use crate::plan::{self, PlanApproval};
use crate::prompts::{plan_system_prompt, print_usage, system_prompt, user_message};
use crate::runner::{self, create_agent};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn repl(
    provider: Arc<dyn Provider>,
    permissions: Arc<PermissionEngine>,
    config: &ChetConfig,
    cwd: &std::path::Path,
    resume_id: Option<String>,
    session_name: Option<String>,
    mut mcp_manager: Option<McpManager>,
    stderr_is_tty: bool,
    project_id: Option<String>,
    memory_manager: MemoryManager,
) -> Result<()> {
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
            },
        )
        .await;

    // Load or create session
    let mut session = match &resume_id {
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
    if let Some(name) = session_name {
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

        // Handle slash commands
        if let Some(handled) = commands::handle_slash_command(
            input,
            &mut session,
            &store,
            &context_tracker,
            &system,
            &mut mcp_manager,
            &memory_manager,
            project_id.as_deref(),
            &status_line,
            &hooks_engine,
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
            true,
            stderr_is_tty,
            status_line.clone(),
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
