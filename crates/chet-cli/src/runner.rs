//! Agent execution: run_agent(), create_agent(), and MCP server startup.

use anyhow::Result;
use chet_config::ChetConfig;
use chet_core::{Agent, AgentEvent, SubagentTool};
use chet_mcp::{McpManager, McpTool};
use chet_permissions::PermissionEngine;
use chet_terminal::StreamingMarkdownRenderer;
use chet_tools::ToolRegistry;
use chet_types::{Message, Usage, provider::Provider};
use std::io::{self, Write};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::context::UIContext;

/// Create a fully-configured Agent with all tools registered.
pub(crate) fn create_agent(
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
        config.memory_dir.clone(),
        project_id.clone(),
    )));
    registry.register(Arc::new(chet_tools::MemoryWriteTool::new(
        config.memory_dir.clone(),
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

/// Run the agent loop and stream styled markdown output to stdout.
pub(crate) async fn run_agent(
    agent: &Agent,
    messages: &mut Vec<Message>,
    ui: UIContext,
) -> Result<Usage> {
    let stdout_is_tty = ui.stdout_is_tty;
    let stderr_is_tty = ui.stderr_is_tty;
    let status_line = ui.status_line;

    let stdout = io::stdout();
    let mut renderer = if stdout_is_tty {
        StreamingMarkdownRenderer::new(Box::new(stdout.lock()))
    } else {
        StreamingMarkdownRenderer::new_plain(Box::new(stdout.lock()))
    };

    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut sighup =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    if let Some(ref mut sig) = sighup { sig.recv().await; }
                    else { std::future::pending::<()>().await; }
                } => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
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
pub(crate) async fn start_mcp_servers(config: &ChetConfig) -> Option<McpManager> {
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
