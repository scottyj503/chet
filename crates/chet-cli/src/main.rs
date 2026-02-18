//! Chet CLI — an AI-powered coding assistant.

mod prompt;

use anyhow::{Context, Result};
use chet_api::ApiClient;
use chet_config::{ChetConfig, CliOverrides};
use chet_core::{Agent, AgentEvent};
use chet_permissions::PermissionEngine;
use chet_session::{ContextTracker, Session, SessionStore, compact};
use chet_terminal::{LineEditor, ReadLineResult, SlashCommandCompleter, StreamingMarkdownRenderer};
use chet_tools::ToolRegistry;
use chet_types::{ContentBlock, Message, Role, Usage};
use chrono::Utc;
use clap::Parser;
use std::io::{self, Write};
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
        .context("Failed to create API client")?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let is_interactive = cli.print.is_none() && !cli.ludicrous;

    if let Some(prompt) = cli.print {
        // Print mode: single prompt, no session persistence
        let engine = if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            PermissionEngine::new(config.permission_rules.clone(), config.hooks.clone(), None)
        };
        let agent = create_agent(client, engine, &config, &cwd);
        let mut messages = vec![user_message(&prompt)];
        let usage = run_agent(&agent, &mut messages).await?;
        print_usage(&usage);
        return Ok(());
    }

    // Interactive REPL mode
    let engine = if cli.ludicrous {
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
    };

    repl(client, engine, &config, &cwd, cli.resume).await
}

fn create_agent(
    client: ApiClient,
    permissions: PermissionEngine,
    config: &ChetConfig,
    cwd: &std::path::Path,
) -> Agent {
    let registry = ToolRegistry::with_builtins();
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
    permissions: PermissionEngine,
    config: &ChetConfig,
    cwd: &std::path::Path,
    resume_id: Option<String>,
) -> Result<()> {
    let agent = create_agent(client, permissions, config, cwd);
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

    loop {
        let input = match editor.read_line("> ").await? {
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
