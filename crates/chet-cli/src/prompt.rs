//! Terminal-based prompt handler for interactive permission prompts.

use chet_permissions::{PromptHandler, PromptResponse};
use std::future::Future;
use std::io::{self, BufRead, Write};
use std::pin::Pin;

/// Prompts the user in the terminal for permission decisions.
pub struct TerminalPromptHandler;

impl PromptHandler for TerminalPromptHandler {
    fn prompt_permission(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        _description: &str,
    ) -> Pin<Box<dyn Future<Output = PromptResponse> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let input_summary = summarize_input(tool_input);

        Box::pin(async move {
            // Use spawn_blocking since we read from stdin
            let result = tokio::task::spawn_blocking(move || {
                let stderr = io::stderr();
                let mut err = stderr.lock();

                let _ = writeln!(err);
                let _ = writeln!(err, "  Permission required: {tool_name}");
                if !input_summary.is_empty() {
                    let _ = writeln!(err, "  {input_summary}");
                }
                let _ = write!(
                    err,
                    "  [y] Allow once  [a] Always allow  [n] Deny  > "
                );
                let _ = err.flush();

                let mut input = String::new();
                let stdin = io::stdin();
                let _ = stdin.lock().read_line(&mut input);

                match input.trim().to_lowercase().as_str() {
                    "y" | "yes" | "" => PromptResponse::AllowOnce,
                    "a" | "always" => PromptResponse::AlwaysAllow,
                    _ => PromptResponse::Deny,
                }
            })
            .await;

            result.unwrap_or(PromptResponse::Deny)
        })
    }
}

/// Create a brief summary of tool input for display.
fn summarize_input(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(3)
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => {
                            if s.len() > 60 {
                                format!("{}...", &s[..60])
                            } else {
                                s.clone()
                            }
                        }
                        other => {
                            let s = other.to_string();
                            if s.len() > 60 {
                                format!("{}...", &s[..60])
                            } else {
                                s
                            }
                        }
                    };
                    format!("{k}: {val}")
                })
                .collect();
            parts.join(", ")
        }
        _ => String::new(),
    }
}
