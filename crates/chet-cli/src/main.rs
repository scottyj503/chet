//! Chet CLI — an AI-powered coding assistant.

mod prompt;

use anyhow::{Context, Result};
use chet_api::ApiClient;
use chet_config::{ChetConfig, CliOverrides};
use chet_core::{Agent, AgentEvent};
use chet_permissions::PermissionEngine;
use chet_tools::ToolRegistry;
use chet_types::{ContentBlock, Message, Role, Usage};
use clap::Parser;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "chet", version, about = "An AI-powered coding assistant")]
struct Cli {
    /// Send a single prompt and print the response (non-interactive)
    #[arg(short, long)]
    print: Option<String>,

    /// Model to use
    #[arg(long)]
    model: Option<String>,

    /// Maximum tokens in the response
    #[arg(long)]
    max_tokens: Option<u32>,

    /// API key (overrides ANTHROPIC_API_KEY)
    #[arg(long)]
    api_key: Option<String>,

    /// Enable verbose/debug logging
    #[arg(long)]
    verbose: bool,

    /// Skip all permission checks — auto-permit every tool call
    #[arg(long)]
    ludicrous: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
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
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let client = ApiClient::new(&config.api_key, &config.api_base_url)
        .context("Failed to create API client")?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let is_interactive = cli.print.is_none() && !cli.ludicrous;

    if let Some(prompt) = cli.print {
        // Print mode: single prompt, no prompt handler (Prompt → Block)
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

    repl(client, engine, &config, &cwd).await
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
    agent
}

async fn repl(
    client: ApiClient,
    permissions: PermissionEngine,
    config: &ChetConfig,
    cwd: &std::path::Path,
) -> Result<()> {
    let agent = create_agent(client, permissions, config, cwd);
    let mut messages: Vec<Message> = Vec::new();
    let mut total_usage = Usage::default();
    let stdin = io::stdin();

    eprintln!(
        "chet v{} (model: {})",
        env!("CARGO_PKG_VERSION"),
        config.model
    );
    eprintln!("Type your message. Press Ctrl+D to exit.\n");

    loop {
        eprint!("> ");
        io::stderr().flush()?;

        let mut input = String::new();
        let bytes_read = stdin.lock().read_line(&mut input)?;
        if bytes_read == 0 {
            eprintln!();
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Handle slash commands
        match input {
            "/quit" | "/exit" => break,
            "/clear" => {
                messages.clear();
                eprintln!("Conversation cleared.");
                continue;
            }
            "/cost" => {
                print_usage(&total_usage);
                continue;
            }
            "/help" => {
                print_help();
                continue;
            }
            "/model" => {
                eprintln!("Current model: {}", config.model);
                continue;
            }
            _ if input.starts_with('/') => {
                eprintln!("Unknown command: {input}. Type /help for available commands.");
                continue;
            }
            _ => {}
        }

        messages.push(user_message(input));

        match run_agent(&agent, &mut messages).await {
            Ok(usage) => {
                total_usage.add(&usage);
            }
            Err(e) => {
                eprintln!("\nError: {e}");
                // Remove the failed user message
                messages.pop();
            }
        }

        println!();
    }

    print_usage(&total_usage);
    Ok(())
}

/// Run the agent loop and stream output to stdout.
async fn run_agent(agent: &Agent, messages: &mut Vec<Message>) -> Result<Usage> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let usage = agent
        .run(messages, |event| match event {
            AgentEvent::TextDelta(text) => {
                let _ = write!(out, "{text}");
                let _ = out.flush();
            }
            AgentEvent::ThinkingDelta(_) => {}
            AgentEvent::ToolStart { name, .. } => {
                let _ = writeln!(out);
                let _ = writeln!(out, "  [tool: {name}]");
            }
            AgentEvent::ToolEnd {
                name,
                output,
                is_error,
            } => {
                if is_error {
                    let _ = writeln!(out, "  [tool {name} error: {output}]");
                } else {
                    let _ = writeln!(out, "  [tool {name} done: {output}]");
                }
            }
            AgentEvent::ToolBlocked { name, reason } => {
                let _ = writeln!(out, "  [tool {name} blocked: {reason}]");
            }
            AgentEvent::Usage(_) => {}
            AgentEvent::Done => {
                let _ = writeln!(out);
            }
            AgentEvent::Error(e) => {
                let _ = writeln!(io::stderr(), "Error: {e}");
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(usage)
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
    eprintln!("  /help    — Show this help");
    eprintln!("  /model   — Show current model");
    eprintln!("  /cost    — Show token usage");
    eprintln!("  /clear   — Clear conversation history");
    eprintln!("  /quit    — Exit");
}
