//! Chet CLI — an AI-powered coding assistant.

mod prompt;

use anyhow::{Context, Result};
use chet_api::AnthropicProvider;
use chet_config::{ChetConfig, CliOverrides};
use chet_core::{Agent, AgentEvent, ManagedWorktree, SubagentTool};
use chet_mcp::{McpManager, McpTool};
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, MemoryManager, Session, SessionStore, compact};
use chet_terminal::{
    LineEditor, ReadLineResult, SlashCommandCompleter, StatusLine, StatusLineData,
    StreamingMarkdownRenderer,
};
use chet_tools::ToolRegistry;
use chet_types::{ContentBlock, Effort, Message, Role, Usage, provider::Provider};
use chrono::Utc;
use clap::Parser;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "chet", version, about = "An AI-powered coding assistant")]
struct Cli {
    /// Send a single prompt and print the response (non-interactive)
    #[arg(short, long)]
    print: Option<String>,

    /// Resume a previous session by ID or prefix
    #[arg(long)]
    resume: Option<String>,

    /// Model to use
    #[arg(long)]
    model: Option<String>,

    /// Maximum tokens in the response
    #[arg(long)]
    max_tokens: Option<u32>,

    /// API key (overrides ANTHROPIC_API_KEY)
    #[arg(long)]
    api_key: Option<String>,

    /// Enable extended thinking with the given token budget
    #[arg(long)]
    thinking_budget: Option<u32>,

    /// Set effort level for extended thinking (low, medium, high)
    #[arg(long)]
    effort: Option<Effort>,

    /// Enable verbose/debug logging
    #[arg(long)]
    verbose: bool,

    /// Skip all permission checks — auto-permit every tool call
    #[arg(long)]
    ludicrous: bool,

    /// Run in an isolated git worktree
    #[arg(long)]
    worktree: bool,

    /// Branch name for the worktree (implies --worktree)
    #[arg(long)]
    worktree_branch: Option<String>,

    /// Name for the session (overrides auto-labeling)
    #[arg(short = 'n', long)]
    name: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    use std::io::IsTerminal;
    let stdout_is_tty = std::io::stdout().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();

    // Safety: ensure raw mode is disabled if we panic (only relevant when TTY attached)
    if stdout_is_tty || stderr_is_tty {
        let default_panic = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::terminal::disable_raw_mode();
            default_panic(info);
        }));
    }

    let cli = Cli::parse();

    // Set up logging
    let log_level = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .with_writer(io::stderr)
        .init();

    let config = ChetConfig::load(CliOverrides {
        api_key: cli.api_key,
        model: cli.model,
        max_tokens: cli.max_tokens,
        thinking_budget: cli.thinking_budget,
        effort: cli.effort,
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let provider: Arc<dyn Provider> = Arc::new(
        AnthropicProvider::new(&config.api_key, &config.api_base_url)
            .context("Failed to create API provider")?
            .with_retry_config(config.retry.clone()),
    );

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let is_interactive = cli.print.is_none() && !cli.ludicrous;

    // Set up worktree isolation if requested
    let worktree_requested = cli.worktree || cli.worktree_branch.is_some();
    let mut managed_worktree: Option<ManagedWorktree> = None;
    let effective_cwd = if worktree_requested {
        // Create a temporary permission engine for hooks during worktree setup
        let setup_engine = Arc::new(if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            PermissionEngine::new(config.permission_rules.clone(), config.hooks.clone(), None)
        });
        match chet_core::create_worktree(&cwd, cli.worktree_branch.as_deref(), Some(setup_engine))
            .await
        {
            Ok(wt) => {
                let effective = wt.path().to_path_buf();
                eprintln!("Created worktree: {}", effective.display());
                managed_worktree = Some(wt);
                effective
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        cwd.clone()
    };

    // Start MCP servers if configured
    let mcp_manager = start_mcp_servers(&config).await;

    // Compute project_id from original cwd (not worktree) so all worktrees share memory
    let project_id: Option<String> = match chet_core::worktree::git_repo_root(&cwd).await {
        Ok(ref root) => Some(MemoryManager::project_id(root)),
        Err(_) => Some(MemoryManager::project_id(&cwd)),
    };

    let memory_manager = MemoryManager::new(config.config_dir.clone());

    let result = if let Some(prompt) = cli.print {
        // Print mode: single prompt, no session persistence
        let engine = Arc::new(if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            PermissionEngine::new(config.permission_rules.clone(), config.hooks.clone(), None)
        });
        let memory_section = memory_manager.load_combined(project_id.as_deref()).await;
        let mut agent = create_agent(
            Arc::clone(&provider),
            engine,
            &config,
            &effective_cwd,
            &mcp_manager,
            project_id,
        );
        agent.set_system_prompt(system_prompt(&effective_cwd, &memory_section));
        let mut messages = vec![user_message(&prompt)];
        let usage = run_agent(&agent, &mut messages, stdout_is_tty, stderr_is_tty, None).await?;
        print_usage(&usage);
        if let Some(manager) = mcp_manager {
            manager.shutdown().await;
        }
        Ok(())
    } else {
        // Interactive REPL mode
        let engine = Arc::new(if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            let prompt_handler: Option<Arc<dyn chet_permissions::PromptHandler>> = if is_interactive
            {
                Some(Arc::new(prompt::TerminalPromptHandler))
            } else {
                None
            };
            PermissionEngine::new(
                config.permission_rules.clone(),
                config.hooks.clone(),
                prompt_handler,
            )
        });

        repl(
            provider,
            engine,
            &config,
            &effective_cwd,
            cli.resume,
            cli.name,
            mcp_manager,
            stderr_is_tty,
            project_id,
            memory_manager,
        )
        .await
    };

    // Always clean up worktree (Drop is the safety net for panics/signals)
    if let Some(mut wt) = managed_worktree {
        eprintln!("Removing worktree...");
        if let Err(e) = wt.cleanup().await {
            eprintln!("Warning: failed to remove worktree: {e}");
        }
    }

    result
}

fn create_agent(
    provider: Arc<dyn Provider>,
    permissions: Arc<PermissionEngine>,
    config: &ChetConfig,
    cwd: &std::path::Path,
    mcp_manager: &Option<McpManager>,
    project_id: Option<String>,
) -> Agent {
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(SubagentTool::new(
        Arc::clone(&provider),
        Arc::clone(&permissions),
        config.model.clone(),
        config.max_tokens,
        cwd.to_path_buf(),
    )));

    // Register memory tools
    registry.register(Arc::new(chet_tools::MemoryReadTool::new(
        config.config_dir.clone(),
        project_id.clone(),
    )));
    registry.register(Arc::new(chet_tools::MemoryWriteTool::new(
        config.config_dir.clone(),
        project_id,
    )));

    // Register MCP tools
    if let Some(manager) = mcp_manager {
        for (client, tool_info) in manager.tools() {
            let server_name = client.server_name().to_string();
            registry.register(Arc::new(McpTool::new(&server_name, tool_info, client)));
        }
    }

    let mut agent = Agent::new(
        provider,
        registry,
        permissions,
        config.model.clone(),
        config.max_tokens,
        cwd.to_path_buf(),
    );
    if let Some(budget) = config.thinking_budget {
        agent.set_thinking_budget(budget);
    }
    if let Some(effort) = config.effort {
        agent.set_effort(Some(effort));
    }
    agent
}

#[allow(clippy::too_many_arguments)]
async fn repl(
    provider: Arc<dyn Provider>,
    permissions: Arc<PermissionEngine>,
    config: &ChetConfig,
    cwd: &std::path::Path,
    resume_id: Option<String>,
    session_name: Option<String>,
    mcp_manager: Option<McpManager>,
    stderr_is_tty: bool,
    project_id: Option<String>,
    memory_manager: MemoryManager,
) -> Result<()> {
    let mut agent = create_agent(
        provider,
        permissions,
        config,
        cwd,
        &mcp_manager,
        project_id.clone(),
    );
    let store = SessionStore::new(config.config_dir.clone())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let context_tracker = ContextTracker::new(&config.model);
    let mut memory_section = memory_manager.load_combined(project_id.as_deref()).await;
    let mut system = system_prompt(cwd, &memory_section);
    agent.set_system_prompt(system.clone());

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

    // Create status line (TTY only)
    let status_line: Option<Arc<Mutex<StatusLine>>> = if stderr_is_tty {
        let context_window_k = chet_session::ContextTracker::new(&config.model)
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
        if input == "/plan" {
            if plan_mode {
                // Exit plan mode
                plan_mode = false;
                agent.set_read_only_mode(false);
                agent.set_system_prompt(system_prompt(cwd, &memory_section));
                eprintln!("Exited plan mode.");
            } else {
                // Enter plan mode
                plan_mode = true;
                agent.set_read_only_mode(true);
                agent.set_system_prompt(plan_system_prompt(cwd, &memory_section));
                eprintln!("{}", chet_terminal::style::plan_mode_banner(stderr_is_tty));
            }
            if let Some(sl) = &status_line {
                sl.lock().unwrap().update_field(|d| d.plan_mode = plan_mode);
            }
            continue;
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
        if let Some(handled) = handle_slash_command(
            input,
            &mut session,
            &store,
            &context_tracker,
            &system,
            &mcp_manager,
            &memory_manager,
            project_id.as_deref(),
            &status_line,
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

        session.messages.push(user_message(input));

        match run_agent(
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

                // In plan mode, save plan to file and prompt for approval
                if plan_mode {
                    if let Some(plan_text) = extract_last_assistant_text(&session.messages) {
                        let plan_path =
                            save_plan_file(&config.config_dir, &session, &plan_text).await;
                        if let Some(path) = plan_path {
                            eprintln!("\nPlan saved to {}", path.display());
                        }
                    }

                    eprintln!();
                    eprintln!("Plan complete. What would you like to do?");
                    eprintln!("  [a]pprove — exit plan mode, keep plan as context");
                    eprintln!("  [r]efine  — stay in plan mode, provide refinements");
                    eprintln!("  [d]iscard — discard plan and exit plan mode");

                    match prompt_plan_approval().await {
                        PlanApproval::Approve => {
                            plan_mode = false;
                            agent.set_read_only_mode(false);
                            agent.set_system_prompt(system_prompt(cwd, &memory_section));
                            eprintln!("Plan approved. Exiting plan mode.");
                        }
                        PlanApproval::Refine => {
                            eprintln!("Staying in plan mode. Provide your refinements.");
                        }
                        PlanApproval::Discard => {
                            pop_last_turn(&mut session.messages);
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

    print_usage(&session.total_usage);
    Ok(())
}

enum SlashResult {
    Continue,
    Break,
    Unknown,
}

#[allow(clippy::too_many_arguments)]
async fn handle_slash_command(
    input: &str,
    session: &mut Session,
    store: &SessionStore,
    context_tracker: &ContextTracker,
    system_prompt: &str,
    mcp_manager: &Option<McpManager>,
    memory_manager: &MemoryManager,
    project_id: Option<&str>,
    status_line: &Option<Arc<Mutex<StatusLine>>>,
) -> Option<SlashResult> {
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
            handle_mcp_status(mcp_manager);
            Some(SlashResult::Continue)
        }
        "/memory" => {
            handle_memory_command(args, memory_manager, project_id, status_line).await;
            Some(SlashResult::Continue)
        }
        "/model" => {
            eprintln!("Current model: {}", session.metadata.model);
            Some(SlashResult::Continue)
        }
        "/context" => {
            let info = context_tracker.estimate(&session.messages, Some(system_prompt));
            eprintln!("{}", context_tracker.format_detailed(&info));
            Some(SlashResult::Continue)
        }
        "/compact" => {
            handle_compact(session, store).await;
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

async fn handle_compact(session: &mut Session, store: &SessionStore) {
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
                eprintln!(
                    "  {} {:>8}  {:>3} msgs  {}{}  {}",
                    s.short_id(),
                    s.age(),
                    s.message_count,
                    s.model,
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

/// Run the agent loop and stream styled markdown output to stdout.
async fn run_agent(
    agent: &Agent,
    messages: &mut Vec<Message>,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
    status_line: Option<Arc<Mutex<StatusLine>>>,
) -> Result<Usage> {
    let stdout = io::stdout();
    let mut renderer = if stdout_is_tty {
        StreamingMarkdownRenderer::new(Box::new(stdout.lock()))
    } else {
        StreamingMarkdownRenderer::new_plain(Box::new(stdout.lock()))
    };

    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_for_signal.cancel();
    });

    let thinking_msg = match agent.effort() {
        Some(e) => format!("Thinking with {e} effort..."),
        None => "Thinking...".to_string(),
    };
    let spinner = chet_terminal::spinner::Spinner::new(&thinking_msg, !stderr_is_tty);
    let mut first_text = true;

    // Clone Arc for the event callback closure
    let sl_for_callback = status_line.clone();

    // Spawn SIGWINCH handler for terminal resize
    #[cfg(unix)]
    let resize_task = {
        let sl_for_resize = status_line.clone();
        tokio::spawn(async move {
            if let Some(sl) = sl_for_resize {
                let mut sig = match tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::window_change(),
                ) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                while sig.recv().await.is_some() {
                    if let Ok((w, h)) = crossterm::terminal::size() {
                        sl.lock().unwrap().resize(w, h);
                    }
                }
            }
        })
    };

    let result = agent
        .run(messages, cancel, |event| match event {
            AgentEvent::TextDelta(text) => {
                if first_text {
                    spinner.set_active(false);
                    chet_terminal::spinner::clear_line(stderr_is_tty);
                    first_text = false;
                }
                renderer.push(&text);
            }
            AgentEvent::ThinkingDelta(text) => {
                if first_text {
                    spinner.set_active(false);
                    chet_terminal::spinner::clear_line(stderr_is_tty);
                    first_text = false;
                }
                if stderr_is_tty {
                    let _ = write!(io::stderr(), "\x1b[2m{text}\x1b[0m");
                } else {
                    let _ = write!(io::stderr(), "{text}");
                }
                let _ = io::stderr().flush();
            }
            AgentEvent::ToolStart { name, .. } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                renderer.finish(); // flush any pending markdown
                let _ = writeln!(
                    io::stderr(),
                    "{}",
                    chet_terminal::style::tool_start(&name, stderr_is_tty)
                );
                if let Some(sl) = &sl_for_callback {
                    sl.lock().unwrap().update_field(|d| {
                        d.active_tool = Some(name.clone());
                    });
                }
                spinner.set_message(&format!("Running {name}..."));
                spinner.set_active(true);
            }
            AgentEvent::ToolEnd {
                name,
                output,
                is_error,
            } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                if is_error {
                    let _ = writeln!(
                        io::stderr(),
                        "{}",
                        chet_terminal::style::tool_error(&name, &output, stderr_is_tty)
                    );
                } else {
                    let _ = writeln!(
                        io::stderr(),
                        "{}",
                        chet_terminal::style::tool_success(&name, &output, stderr_is_tty)
                    );
                }
                if let Some(sl) = &sl_for_callback {
                    sl.lock().unwrap().update_field(|d| {
                        d.active_tool = None;
                    });
                }
                spinner.set_message(&thinking_msg);
                spinner.set_active(true);
                first_text = true;
            }
            AgentEvent::ToolBlocked { name, reason } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                let _ = writeln!(
                    io::stderr(),
                    "{}",
                    chet_terminal::style::tool_blocked(&name, &reason, stderr_is_tty)
                );
            }
            AgentEvent::Cancelled => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                renderer.finish();
                let _ = writeln!(io::stderr(), "\nCancelled.");
            }
            AgentEvent::Usage(_) => {}
            AgentEvent::Done => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                renderer.finish();
                if let Some(sl) = &sl_for_callback {
                    sl.lock().unwrap().update_field(|d| {
                        d.active_tool = None;
                    });
                }
                let _ = writeln!(io::stdout());
            }
            AgentEvent::Error(e) => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line(stderr_is_tty);
                let _ = writeln!(io::stderr(), "Error: {e}");
            }
        })
        .await;

    signal_task.abort(); // clean up signal listener
    #[cfg(unix)]
    resize_task.abort();
    spinner.stop(stderr_is_tty).await;

    match result {
        Ok(usage) => Ok(usage),
        Err(chet_types::ChetError::Cancelled) => Ok(Usage::default()),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

/// Start MCP servers from config. Returns None if no servers configured.
async fn start_mcp_servers(config: &ChetConfig) -> Option<McpManager> {
    if config.mcp.servers.is_empty() {
        return None;
    }
    let manager = McpManager::start(&config.mcp).await;
    if manager.client_count() > 0 {
        Some(manager)
    } else {
        None
    }
}

fn handle_mcp_status(mcp_manager: &Option<McpManager>) {
    match mcp_manager {
        Some(manager) if manager.client_count() > 0 => {
            eprintln!("MCP servers ({} connected):", manager.client_count());
            for (name, tool_count) in manager.server_summary() {
                eprintln!("  {name}: {tool_count} tools");
            }
        }
        _ => {
            eprintln!("No MCP servers connected.");
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

fn user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    }
}

fn append_memory_instructions(prompt: &mut String, memory: &str) {
    prompt.push_str(
        "\n\n# Persistent Memory\n\n\
        You have access to persistent memory that survives across sessions via the \
        MemoryRead and MemoryWrite tools.\n\
        - Use MemoryRead to check existing memory before writing.\n\
        - When writing, provide the complete updated content for the scope (global or project).\n\
        - Use 'global' scope for cross-project preferences, 'project' for project-specific notes.\n\
        - Save things worth remembering: user preferences, project conventions, key decisions.\n\
        - When the user explicitly asks you to remember something, save it immediately.",
    );
    if !memory.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(memory);
    }
}

fn system_prompt(cwd: &std::path::Path, memory: &str) -> String {
    let mut prompt = format!(
        "You are Chet, an AI coding assistant running in a terminal. \
         You help users with software engineering tasks by reading, writing, \
         and editing code files, running commands, and searching codebases.\n\n\
         Current working directory: {}\n\n\
         Use the available tools to assist the user. Be concise and helpful.",
        cwd.display()
    );
    append_memory_instructions(&mut prompt, memory);
    prompt
}

fn print_usage(usage: &Usage) {
    eprintln!(
        "Tokens — input: {}, output: {}, cache read: {}, cache write: {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens
    );
}

fn print_help() {
    eprintln!("Available commands:");
    eprintln!("  /help     — Show this help");
    eprintln!("  /effort   — Show or set effort level (low, medium, high, auto)");
    eprintln!("  /plan     — Toggle plan mode (read-only exploration)");
    eprintln!("  /mcp      — Show connected MCP servers and tools");
    eprintln!("  /memory   — View/edit/reset persistent memory");
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

fn plan_system_prompt(cwd: &std::path::Path, memory: &str) -> String {
    let mut prompt = format!(
        "You are Chet, an AI coding assistant running in PLAN MODE.\n\n\
         Current working directory: {}\n\n\
         In plan mode, you can ONLY use read-only tools (Read, Glob, Grep) to explore the codebase.\n\
         You CANNOT modify files, run commands, or make any changes.\n\n\
         Your task is to:\n\
         1. Explore the codebase using the available read-only tools\n\
         2. Understand the existing code structure and patterns\n\
         3. Produce a clear, structured implementation plan in markdown\n\n\
         Your plan should include:\n\
         - Summary of proposed changes\n\
         - Files to modify or create\n\
         - Step-by-step implementation approach\n\
         - Key considerations or risks\n\n\
         Be thorough in exploration but concise in your plan.",
        cwd.display()
    );
    append_memory_instructions(&mut prompt, memory);
    prompt
}

/// Extract text content from the last assistant message.
fn extract_last_assistant_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        if m.role == Role::Assistant {
            let text: String = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() { None } else { Some(text) }
        } else {
            None
        }
    })
}

/// Save plan text to ~/.chet/plans/<short-session-id>-<timestamp>.md
async fn save_plan_file(
    config_dir: &std::path::Path,
    session: &Session,
    plan_text: &str,
) -> Option<std::path::PathBuf> {
    let plans_dir = config_dir.join("plans");
    if let Err(e) = tokio::fs::create_dir_all(&plans_dir).await {
        eprintln!("Warning: failed to create plans directory: {e}");
        return None;
    }

    let timestamp = Utc::now().format("%Y%m%dT%H%M");
    let filename = format!("{}-{}.md", session.short_id(), timestamp);
    let path = plans_dir.join(&filename);

    match tokio::fs::write(&path, plan_text).await {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("Warning: failed to save plan file: {e}");
            None
        }
    }
}

enum PlanApproval {
    Approve,
    Refine,
    Discard,
}

async fn prompt_plan_approval() -> PlanApproval {
    eprint!("  > ");
    let _ = io::stderr().flush();

    let result = tokio::task::spawn_blocking(|| {
        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line).ok();
        line
    })
    .await
    .unwrap_or_default();

    match result.trim().to_lowercase().as_str() {
        "a" | "approve" => PlanApproval::Approve,
        "r" | "refine" => PlanApproval::Refine,
        "d" | "discard" => PlanApproval::Discard,
        _ => {
            eprintln!("Invalid choice, defaulting to refine.");
            PlanApproval::Refine
        }
    }
}

/// Remove the last turn (user message + assistant response + any tool-result messages).
/// Peels from the end: assistant messages, tool-result user messages, then the triggering user text.
fn pop_last_turn(messages: &mut Vec<Message>) {
    // Pop trailing assistant messages
    while let Some(last) = messages.last() {
        if last.role == Role::Assistant {
            messages.pop();
        } else {
            break;
        }
    }

    // Pop tool-result user messages (content is all ToolResult blocks)
    while let Some(last) = messages.last() {
        if last.role == Role::User && is_tool_result_message(last) {
            messages.pop();
        } else {
            break;
        }
    }

    // Pop assistant messages interleaved with tool results
    while let Some(last) = messages.last() {
        if last.role == Role::Assistant {
            messages.pop();
            // After popping assistant, check for more tool-result messages
            while let Some(last) = messages.last() {
                if last.role == Role::User && is_tool_result_message(last) {
                    messages.pop();
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }

    // Pop the triggering user text message
    if let Some(last) = messages.last() {
        if last.role == Role::User && !is_tool_result_message(last) {
            messages.pop();
        }
    }
}

fn is_tool_result_message(msg: &Message) -> bool {
    !msg.content.is_empty()
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chet_types::ToolResultContent;

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn tool_result_msg(tool_use_id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: vec![ToolResultContent::Text {
                    text: text.to_string(),
                }],
                is_error: None,
            }],
        }
    }

    #[test]
    fn pop_last_turn_simple_exchange() {
        let mut msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "hi there"),
        ];
        pop_last_turn(&mut msgs);
        assert!(msgs.is_empty());
    }

    #[test]
    fn pop_last_turn_with_tool_results() {
        let mut msgs = vec![
            text_msg(Role::User, "earlier question"),
            text_msg(Role::Assistant, "earlier answer"),
            text_msg(Role::User, "read my files"),
            text_msg(Role::Assistant, "let me read"), // assistant with tool_use
            tool_result_msg("t1", "file contents"),   // tool result
            text_msg(Role::Assistant, "here's the answer"), // final assistant
        ];
        pop_last_turn(&mut msgs);
        // Should preserve only the earlier exchange
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn pop_last_turn_preserves_earlier_messages() {
        let mut msgs = vec![
            text_msg(Role::User, "first"),
            text_msg(Role::Assistant, "first reply"),
            text_msg(Role::User, "second"),
            text_msg(Role::Assistant, "second reply"),
        ];
        pop_last_turn(&mut msgs);
        assert_eq!(msgs.len(), 2);
        if let ContentBlock::Text { text } = &msgs[0].content[0] {
            assert_eq!(text, "first");
        }
    }

    #[test]
    fn pop_last_turn_empty() {
        let mut msgs: Vec<Message> = vec![];
        pop_last_turn(&mut msgs);
        assert!(msgs.is_empty());
    }

    #[test]
    fn plan_system_prompt_contains_key_directives() {
        let prompt = plan_system_prompt(std::path::Path::new("/tmp"), "");
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("read-only"));
        assert!(prompt.contains("/tmp"));
        assert!(prompt.contains("Read"));
        assert!(prompt.contains("Glob"));
        assert!(prompt.contains("Grep"));
    }

    #[test]
    fn system_prompt_includes_memory() {
        let prompt = system_prompt(std::path::Path::new("/tmp"), "# Memory\n\nTest memory");
        assert!(prompt.contains("Test memory"));
        assert!(prompt.contains("MemoryRead"));
        assert!(prompt.contains("MemoryWrite"));
    }

    #[test]
    fn system_prompt_empty_memory() {
        let prompt = system_prompt(std::path::Path::new("/tmp"), "");
        // Should still have memory tool instructions
        assert!(prompt.contains("MemoryRead"));
        assert!(prompt.contains("MemoryWrite"));
        // But no memory content section
        assert!(!prompt.contains("# Memory"));
    }

    #[test]
    fn extract_last_assistant_text_finds_text() {
        let msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "the plan"),
        ];
        assert_eq!(
            extract_last_assistant_text(&msgs),
            Some("the plan".to_string())
        );
    }

    #[test]
    fn extract_last_assistant_text_skips_user() {
        let msgs = vec![text_msg(Role::User, "hello")];
        assert_eq!(extract_last_assistant_text(&msgs), None);
    }

    #[test]
    fn is_tool_result_message_true() {
        let msg = tool_result_msg("t1", "output");
        assert!(is_tool_result_message(&msg));
    }

    #[test]
    fn is_tool_result_message_false_for_text() {
        let msg = text_msg(Role::User, "hello");
        assert!(!is_tool_result_message(&msg));
    }
}
