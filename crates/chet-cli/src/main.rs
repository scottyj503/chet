//! Chet CLI — an AI-powered coding assistant.

mod commands;
mod plan;
mod prompt;
mod prompts;
mod repl;
mod runner;

use anyhow::{Context, Result};
use chet_api::AnthropicProvider;
use chet_config::{ChetConfig, CliOverrides};
use chet_core::ManagedWorktree;
use chet_permissions::PermissionEngine;
use chet_session::MemoryManager;
use chet_types::{Effort, provider::Provider};
use clap::Parser;
use std::io;
use std::sync::Arc;

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
    let mcp_manager = runner::start_mcp_servers(&config).await;

    // Compute project_id from original cwd (not worktree) so all worktrees share memory
    let project_id: Option<String> = match chet_core::worktree::git_repo_root(&cwd).await {
        Ok(ref root) => Some(MemoryManager::project_id(root)),
        Err(_) => Some(MemoryManager::project_id(&cwd)),
    };

    let memory_manager = MemoryManager::new(config.memory_dir.clone());

    let result = if let Some(prompt) = cli.print {
        // Print mode: single prompt, no session persistence
        let engine = Arc::new(if cli.ludicrous {
            PermissionEngine::ludicrous()
        } else {
            PermissionEngine::new(config.permission_rules.clone(), config.hooks.clone(), None)
        });
        let memory_section = memory_manager.load_combined(project_id.as_deref()).await;
        let mut agent = runner::create_agent(
            Arc::clone(&provider),
            engine,
            &config,
            &effective_cwd,
            &mcp_manager,
            project_id,
        );
        agent.set_system_prompt(prompts::system_prompt(&effective_cwd, &memory_section));
        let mut messages = vec![prompts::user_message(&prompt)];
        let usage =
            runner::run_agent(&agent, &mut messages, stdout_is_tty, stderr_is_tty, None).await?;
        prompts::print_usage(&usage);
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

        repl::repl(
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
