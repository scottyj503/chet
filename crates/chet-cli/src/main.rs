//! Chet CLI — an AI-powered coding assistant.

mod prompt;

use anyhow::{Context, Result};
use chet_api::ApiClient;
use chet_config::{ChetConfig, CliOverrides};
use chet_core::{Agent, AgentEvent, SubagentTool};
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, Session, SessionStore, compact};
use chet_terminal::{LineEditor, ReadLineResult, SlashCommandCompleter, StreamingMarkdownRenderer};
use chet_tools::ToolRegistry;
use chet_types::{ContentBlock, Message, Role, Usage};
use chrono::Utc;
use clap::Parser;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
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

    /// Enable verbose/debug logging
    #[arg(long)]
    verbose: bool,

    /// Skip all permission checks — auto-permit every tool call
    #[arg(long)]
    ludicrous: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Safety: ensure raw mode is disabled if we panic
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        default_panic(info);
    }));

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
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let client = ApiClient::new(&config.api_key, &config.api_base_url)
        .context("Failed to create API client")?
        .with_retry_config(config.retry.clone());

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let is_interactive = cli.print.is_none() && !cli.ludicrous;

    if let Some(prompt) = cli.print {
        // Print mode: single prompt, no session persistence
        let engine = Arc::new(if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            PermissionEngine::new(config.permission_rules.clone(), config.hooks.clone(), None)
        });
        let agent = create_agent(client, engine, &config, &cwd);
        let mut messages = vec![user_message(&prompt)];
        let usage = run_agent(&agent, &mut messages).await?;
        print_usage(&usage);
        return Ok(());
    }

    // Interactive REPL mode
    let engine = Arc::new(if cli.ludicrous {
        PermissionEngine::ludicrous()
    } else {
        let prompt_handler: Option<Arc<dyn chet_permissions::PromptHandler>> = if is_interactive {
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

    repl(client, engine, &config, &cwd, cli.resume).await
}

fn create_agent(
    client: ApiClient,
    permissions: Arc<PermissionEngine>,
    config: &ChetConfig,
    cwd: &std::path::Path,
) -> Agent {
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(SubagentTool::new(
        client.clone(),
        Arc::clone(&permissions),
        config.model.clone(),
        config.max_tokens,
        cwd.to_path_buf(),
    )));
    let mut agent = Agent::new(
        client,
        registry,
        permissions,
        config.model.clone(),
        config.max_tokens,
        cwd.to_path_buf(),
    );
    agent.set_system_prompt(system_prompt(cwd));
    if let Some(budget) = config.thinking_budget {
        agent.set_thinking_budget(budget);
    }
    agent
}

async fn repl(
    client: ApiClient,
    permissions: Arc<PermissionEngine>,
    config: &ChetConfig,
    cwd: &std::path::Path,
    resume_id: Option<String>,
) -> Result<()> {
    let mut agent = create_agent(client, permissions, config, cwd);
    let store = SessionStore::new(config.config_dir.clone())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let context_tracker = ContextTracker::new(&config.model);
    let system = system_prompt(cwd);

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

    let mut editor = LineEditor::new(config.config_dir.join("history"));
    editor.set_completer(Box::new(SlashCommandCompleter::new(vec![
        "/quit",
        "/exit",
        "/clear",
        "/cost",
        "/help",
        "/model",
        "/context",
        "/compact",
        "/sessions",
        "/resume",
        "/plan",
    ])));

    let thinking_info = match config.thinking_budget {
        Some(budget) => format!(", thinking: {budget} tokens"),
        None => String::new(),
    };
    eprintln!(
        "chet v{} (model: {}{}, session: {})",
        env!("CARGO_PKG_VERSION"),
        config.model,
        thinking_info,
        session.short_id()
    );
    eprintln!("Type your message. Press Ctrl+D to exit.\n");

    let mut plan_mode = false;

    loop {
        let prompt = if plan_mode { "plan> " } else { "> " };
        let input = match editor.read_line(prompt).await? {
            ReadLineResult::Line(line) => line,
            ReadLineResult::Eof => {
                eprintln!();
                break;
            }
            ReadLineResult::Interrupted => continue,
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
                agent.set_system_prompt(system_prompt(cwd));
                eprintln!("Exited plan mode.");
            } else {
                // Enter plan mode
                plan_mode = true;
                agent.set_read_only_mode(true);
                agent.set_system_prompt(plan_system_prompt(cwd));
                eprintln!("{}", chet_terminal::style::plan_mode_banner());
            }
            continue;
        }

        // Handle slash commands
        if let Some(handled) =
            handle_slash_command(input, &mut session, &store, &context_tracker, &system).await
        {
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

        match run_agent(&agent, &mut session.messages).await {
            Ok(usage) => {
                session.total_usage.add(&usage);
                session.updated_at = Utc::now();

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
                            agent.set_system_prompt(system_prompt(cwd));
                            eprintln!("Plan approved. Exiting plan mode.");
                        }
                        PlanApproval::Refine => {
                            eprintln!("Staying in plan mode. Provide your refinements.");
                        }
                        PlanApproval::Discard => {
                            pop_last_turn(&mut session.messages);
                            plan_mode = false;
                            agent.set_read_only_mode(false);
                            agent.set_system_prompt(system_prompt(cwd));
                            eprintln!("Plan discarded. Exiting plan mode.");
                        }
                    }
                }

                // Auto-save
                if let Err(e) = store.save(&session).await {
                    eprintln!("Warning: failed to save session: {e}");
                }

                // Print brief context line
                let info = context_tracker.estimate(&session.messages, Some(&system));
                eprintln!("{}", context_tracker.format_brief(&info));
            }
            Err(e) => {
                eprintln!("\nError: {e}");
                // Remove the failed user message
                session.messages.pop();
            }
        }

        println!();
    }

    editor.save_history()?;

    // Final save
    session.updated_at = Utc::now();
    if let Err(e) = store.save(&session).await {
        eprintln!("Warning: failed to save session: {e}");
    }

    print_usage(&session.total_usage);
    Ok(())
}

enum SlashResult {
    Continue,
    Break,
    Unknown,
}

async fn handle_slash_command(
    input: &str,
    session: &mut Session,
    store: &SessionStore,
    context_tracker: &ContextTracker,
    system_prompt: &str,
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
    match compact(&session.messages) {
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
async fn run_agent(agent: &Agent, messages: &mut Vec<Message>) -> Result<Usage> {
    let stdout = io::stdout();
    let mut renderer = StreamingMarkdownRenderer::new(Box::new(stdout.lock()));

    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_for_signal.cancel();
    });

    let spinner = chet_terminal::spinner::Spinner::new("Thinking...");
    let mut first_text = true;

    let result = agent
        .run(messages, cancel, |event| match event {
            AgentEvent::TextDelta(text) => {
                if first_text {
                    spinner.set_active(false);
                    chet_terminal::spinner::clear_line();
                    first_text = false;
                }
                renderer.push(&text);
            }
            AgentEvent::ThinkingDelta(text) => {
                if first_text {
                    spinner.set_active(false);
                    chet_terminal::spinner::clear_line();
                    first_text = false;
                }
                let _ = write!(io::stderr(), "\x1b[2m{text}\x1b[0m");
                let _ = io::stderr().flush();
            }
            AgentEvent::ToolStart { name, .. } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                renderer.finish(); // flush any pending markdown
                let _ = writeln!(io::stderr(), "{}", chet_terminal::style::tool_start(&name));
                spinner.set_message(&format!("Running {name}..."));
                spinner.set_active(true);
            }
            AgentEvent::ToolEnd {
                name,
                output,
                is_error,
            } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                if is_error {
                    let _ = writeln!(
                        io::stderr(),
                        "{}",
                        chet_terminal::style::tool_error(&name, &output)
                    );
                } else {
                    let _ = writeln!(
                        io::stderr(),
                        "{}",
                        chet_terminal::style::tool_success(&name, &output)
                    );
                }
                spinner.set_message("Thinking...");
                spinner.set_active(true);
                first_text = true;
            }
            AgentEvent::ToolBlocked { name, reason } => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                let _ = writeln!(
                    io::stderr(),
                    "{}",
                    chet_terminal::style::tool_blocked(&name, &reason)
                );
            }
            AgentEvent::Cancelled => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                renderer.finish();
                let _ = writeln!(io::stderr(), "\nCancelled.");
            }
            AgentEvent::Usage(_) => {}
            AgentEvent::Done => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                renderer.finish();
                let _ = writeln!(io::stdout());
            }
            AgentEvent::Error(e) => {
                spinner.set_active(false);
                chet_terminal::spinner::clear_line();
                let _ = writeln!(io::stderr(), "Error: {e}");
            }
        })
        .await;

    signal_task.abort(); // clean up signal listener
    spinner.stop().await;

    match result {
        Ok(usage) => Ok(usage),
        Err(chet_types::ChetError::Cancelled) => Ok(Usage::default()),
        Err(e) => Err(anyhow::anyhow!("{e}")),
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

fn system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "You are Chet, an AI coding assistant running in a terminal. \
         You help users with software engineering tasks by reading, writing, \
         and editing code files, running commands, and searching codebases.\n\n\
         Current working directory: {}\n\n\
         Use the available tools to assist the user. Be concise and helpful.",
        cwd.display()
    )
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
    eprintln!("  /plan     — Toggle plan mode (read-only exploration)");
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
    eprintln!("  --thinking-budget <N>  — Enable extended thinking (token budget)");
}

fn plan_system_prompt(cwd: &std::path::Path) -> String {
    format!(
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
    )
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
        let prompt = plan_system_prompt(std::path::Path::new("/tmp"));
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("read-only"));
        assert!(prompt.contains("/tmp"));
        assert!(prompt.contains("Read"));
        assert!(prompt.contains("Glob"));
        assert!(prompt.contains("Grep"));
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
