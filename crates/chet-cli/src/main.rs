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

    /// Provider to use: anthropic (default), bedrock, vertex
    #[arg(long)]
    provider: Option<String>,

    /// AWS region for Bedrock (overrides AWS_REGION)
    #[arg(long)]
    aws_region: Option<String>,

    /// Google Cloud project ID for Vertex AI
    #[arg(long)]
    vertex_project: Option<String>,

    /// Google Cloud region for Vertex AI (default: us-east5)
    #[arg(long)]
    vertex_region: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// List configured agent profiles
    Agents,
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

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let config = ChetConfig::load_with_project_dir(
        CliOverrides {
            api_key: cli.api_key.clone(),
            model: cli.model.clone(),
            max_tokens: cli.max_tokens,
            thinking_budget: cli.thinking_budget,
            effort: cli.effort,
        },
        Some(&cwd),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Handle subcommands that don't need the full agent stack
    if let Some(Commands::Agents) = cli.command {
        print_agents(&config);
        return Ok(());
    }

    let provider: Arc<dyn Provider> = create_provider(&cli, &config).await?;

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

        let original_cwd = if worktree_requested {
            Some(cwd.clone())
        } else {
            None
        };
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
            original_cwd,
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

/// Print the configured agent profiles from `[agents.<name>]` config sections.
fn print_agents(config: &ChetConfig) {
    if config.agents.is_empty() {
        println!("No agent profiles configured.");
        println!("Add profiles in ~/.chet/config.toml or .chet/config.toml:");
        println!();
        println!("  [agents.reviewer]");
        println!("  effort = \"high\"");
        println!("  disallowed_tools = [\"Write\", \"Edit\"]");
        return;
    }

    let mut names: Vec<&String> = config.agents.keys().collect();
    names.sort();

    println!("Configured agent profiles:\n");
    for name in names {
        let agent = &config.agents[name];
        println!("  {name}");
        if let Some(e) = agent.effort {
            println!("    effort: {e}");
        }
        if let Some(n) = agent.max_turns {
            println!("    max_turns: {n}");
        }
        if !agent.disallowed_tools.is_empty() {
            println!(
                "    disallowed_tools: {}",
                agent.disallowed_tools.join(", ")
            );
        }
        if let Some(prompt) = &agent.system_prompt {
            let preview = chet_types::truncate_str(prompt, 60);
            let ellipsis = if prompt.len() > 60 { "..." } else { "" };
            println!("    system_prompt: {preview}{ellipsis}");
        }
        println!();
    }
}

/// Resolve which provider to use and construct it.
///
/// Priority: --provider flag > CLAUDE_CODE_USE_BEDROCK/VERTEX env > CHET_USE_BEDROCK/VERTEX env > "anthropic"
async fn create_provider(cli: &Cli, config: &ChetConfig) -> Result<Arc<dyn Provider>> {
    let provider_name = resolve_provider_name(cli);

    match provider_name.as_str() {
        "anthropic" => {
            let provider = AnthropicProvider::new(&config.api_key, &config.api_base_url)
                .context("Failed to create Anthropic provider")?
                .with_retry_config(config.retry.clone());
            Ok(Arc::new(provider))
        }
        #[cfg(feature = "bedrock")]
        "bedrock" => {
            let region = cli
                .aws_region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let provider = chet_bedrock::BedrockProvider::new(&region);
            Ok(Arc::new(provider))
        }
        #[cfg(feature = "vertex")]
        "vertex" => {
            let project = cli
                .vertex_project
                .clone()
                .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Vertex AI requires --vertex-project or GOOGLE_CLOUD_PROJECT env var"
                    )
                })?;
            let region = cli
                .vertex_region
                .clone()
                .or_else(|| std::env::var("GOOGLE_CLOUD_REGION").ok())
                .unwrap_or_else(|| "us-east5".to_string());
            let provider = chet_vertex::VertexProvider::new(&project, &region);
            Ok(Arc::new(provider))
        }
        #[cfg(not(feature = "bedrock"))]
        "bedrock" => Err(anyhow::anyhow!(
            "Bedrock support not compiled. Rebuild with: cargo install --features bedrock"
        )),
        #[cfg(not(feature = "vertex"))]
        "vertex" => Err(anyhow::anyhow!(
            "Vertex AI support not compiled. Rebuild with: cargo install --features vertex"
        )),
        other => Err(anyhow::anyhow!(
            "Unknown provider: {other}. Use: anthropic, bedrock, or vertex"
        )),
    }
}

/// Resolve provider name from CLI flag and env vars.
fn resolve_provider_name(cli: &Cli) -> String {
    if let Some(ref p) = cli.provider {
        return p.clone();
    }
    // Claude Code compatible env vars
    if env_is_truthy("CLAUDE_CODE_USE_BEDROCK") || env_is_truthy("CHET_USE_BEDROCK") {
        return "bedrock".to_string();
    }
    if env_is_truthy("CLAUDE_CODE_USE_VERTEX") || env_is_truthy("CHET_USE_VERTEX") {
        return "vertex".to_string();
    }
    "anthropic".to_string()
}

/// Check if an env var is set to a truthy value (not empty, not "0", not "false").
fn env_is_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(val) => !val.is_empty() && val != "0" && val.to_lowercase() != "false",
        Err(_) => false,
    }
}
