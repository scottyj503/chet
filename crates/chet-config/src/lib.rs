//! Multi-tier TOML configuration for Chet.
//!
//! Reads configuration from multiple sources with precedence:
//! env vars > project > global > defaults

use chet_api::RetryConfig;
use chet_types::{AuthCredential, Effort};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The default Anthropic API base URL.
pub const DEFAULT_API_BASE_URL: &str = "https://api.anthropic.com";

/// The default model to use.
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";

/// The default max tokens for a response.
pub const DEFAULT_MAX_TOKENS: u32 = 65536;

/// Resolved configuration for a Chet session.
#[derive(Debug, Clone)]
pub struct ChetConfig {
    pub credential: AuthCredential,
    pub model: String,
    pub max_tokens: u32,
    pub api_base_url: String,
    pub thinking_budget: Option<u32>,
    pub effort: Option<Effort>,
    pub retry: RetryConfig,
    pub config_dir: PathBuf,
    /// Directory for persistent memory files. Defaults to `<config_dir>/memory/`.
    pub memory_dir: PathBuf,
    pub permission_rules: Vec<chet_permissions::PermissionRule>,
    pub hooks: Vec<chet_permissions::HookConfig>,
    pub mcp: chet_mcp::McpConfig,
    /// Per-agent configuration profiles.
    pub agents: std::collections::HashMap<String, AgentConfig>,
}

/// Settings that can be read from a TOML config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub api: ApiSettings,
    #[serde(default)]
    pub permissions: PermissionsSettings,
    #[serde(default)]
    pub hooks: Vec<chet_permissions::HookConfig>,
    #[serde(default)]
    pub mcp: chet_mcp::McpConfig,
    /// Custom directory for persistent memory files (default: `<config_dir>/memory/`).
    pub memory_dir: Option<String>,
    /// Per-agent configuration profiles.
    #[serde(default)]
    pub agents: std::collections::HashMap<String, AgentConfig>,
    /// Custom model aliases (e.g. { "fast" = "claude-haiku-4-5-20251001" }).
    #[serde(default)]
    pub models: std::collections::HashMap<String, String>,
}

/// Per-agent configuration profile (used by SubagentTool and named agents).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Effort level override for this agent.
    pub effort: Option<Effort>,
    /// Maximum tool-use turns before stopping.
    pub max_turns: Option<usize>,
    /// Tools that this agent is not allowed to use.
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// Custom system prompt for this agent.
    pub system_prompt: Option<String>,
}

/// Permission rules section of the config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsSettings {
    #[serde(default)]
    pub rules: Vec<chet_permissions::PermissionRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiSettings {
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub base_url: Option<String>,
    pub thinking_budget: Option<u32>,
    pub effort: Option<Effort>,
    #[serde(default)]
    pub retry: RetrySettings,
}

/// Optional retry settings from the `[api.retry]` config section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrySettings {
    pub max_retries: Option<u32>,
    pub initial_delay_ms: Option<u64>,
    pub max_delay_ms: Option<u64>,
}

/// CLI overrides that take highest precedence.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<u32>,
    pub effort: Option<Effort>,
}

impl ChetConfig {
    /// Load configuration from all sources, applying precedence rules.
    ///
    /// Precedence (highest to lowest):
    /// 1. CLI flags
    /// 2. Environment variables
    /// 3. Project config (~/.chet/projects/<hash>/config.toml)
    /// 4. Global config (~/.chet/config.toml)
    /// 5. Defaults
    pub fn load(overrides: CliOverrides) -> Result<Self, chet_types::ConfigError> {
        Self::load_with_project_dir(overrides, None)
    }

    /// Load configuration, optionally merging a project-level config.
    /// The project dir (e.g. a worktree or repo root) can contain `.chet/config.toml`
    /// with hooks and permission rules that supplement the global config.
    pub fn load_with_project_dir(
        overrides: CliOverrides,
        project_dir: Option<&std::path::Path>,
    ) -> Result<Self, chet_types::ConfigError> {
        let config_dir = config_dir();
        let global_settings = load_settings_file(&config_dir.join("config.toml"));

        // Load project-level config if available
        let project_settings =
            project_dir.map(|dir| load_settings_file(&dir.join(".chet").join("config.toml")));

        // Resolve auth credential: auth_token takes precedence over api_key at each tier.
        let credential = overrides
            .auth_token
            .map(AuthCredential::AuthToken)
            .or_else(|| overrides.api_key.map(AuthCredential::ApiKey))
            .or_else(|| {
                std::env::var("ANTHROPIC_AUTH_TOKEN")
                    .ok()
                    .map(AuthCredential::AuthToken)
            })
            .or_else(|| {
                std::env::var("ANTHROPIC_API_KEY")
                    .ok()
                    .map(AuthCredential::ApiKey)
            })
            .or_else(|| {
                global_settings
                    .api
                    .auth_token
                    .clone()
                    .map(AuthCredential::AuthToken)
            })
            .or_else(|| {
                global_settings
                    .api
                    .api_key
                    .clone()
                    .map(AuthCredential::ApiKey)
            })
            .ok_or_else(|| chet_types::ConfigError::MissingKey {
                key: "api credential (set ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, or add to ~/.chet/config.toml)"
                    .into(),
            })?;

        // Resolve model (with alias expansion from [models] section)
        let raw_model = overrides
            .model
            .or_else(|| std::env::var("CHET_MODEL").ok())
            .or(global_settings.api.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let model = global_settings
            .models
            .get(&raw_model)
            .cloned()
            .unwrap_or(raw_model);

        // Resolve max tokens
        let max_tokens = overrides
            .max_tokens
            .or(global_settings.api.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS);

        // Resolve API base URL
        let api_base_url = std::env::var("ANTHROPIC_API_BASE_URL")
            .ok()
            .or(global_settings.api.base_url)
            .unwrap_or_else(|| DEFAULT_API_BASE_URL.to_string());

        // Resolve thinking budget: CLI > config > None
        let thinking_budget = overrides
            .thinking_budget
            .or(global_settings.api.thinking_budget);

        // Resolve effort: CLI > config > None
        let effort = overrides.effort.or(global_settings.api.effort);

        // Resolve retry config: config > defaults
        let retry_defaults = RetryConfig::default();
        let retry = RetryConfig {
            max_retries: global_settings
                .api
                .retry
                .max_retries
                .unwrap_or(retry_defaults.max_retries),
            initial_delay_ms: global_settings
                .api
                .retry
                .initial_delay_ms
                .unwrap_or(retry_defaults.initial_delay_ms),
            max_delay_ms: global_settings
                .api
                .retry
                .max_delay_ms
                .unwrap_or(retry_defaults.max_delay_ms),
            backoff_factor: retry_defaults.backoff_factor,
        };

        let memory_dir = match global_settings.memory_dir {
            Some(ref dir) => PathBuf::from(dir),
            None => config_dir.join("memory"),
        };

        // Merge project-level hooks and permission rules (project supplements global)
        let mut permission_rules = global_settings.permissions.rules;
        let mut hooks = global_settings.hooks;
        if let Some(ref proj) = project_settings {
            permission_rules.extend(proj.permissions.rules.clone());
            hooks.extend(proj.hooks.clone());
        }

        Ok(ChetConfig {
            credential,
            model,
            max_tokens,
            api_base_url,
            thinking_budget,
            effort,
            retry,
            permission_rules,
            hooks,
            mcp: global_settings.mcp,
            agents: global_settings.agents,
            config_dir,
            memory_dir,
        })
    }
}

/// Get the Chet config directory path (~/.chet/).
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CHET_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".chet")
}

/// Load and parse a TOML settings file, returning defaults on any error.
fn load_settings_file(path: &std::path::Path) -> SettingsFile {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse {}: {}", path.display(), e);
            SettingsFile::default()
        }),
        Err(_) => SettingsFile::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = SettingsFile::default();
        assert!(settings.api.api_key.is_none());
        assert!(settings.api.model.is_none());
    }

    #[test]
    fn test_settings_toml_parse() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
max_tokens = 8192
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.api.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(settings.api.max_tokens, Some(8192));
    }

    #[test]
    fn test_settings_with_permissions() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"

[[permissions.rules]]
tool = "Read"
level = "permit"

[[permissions.rules]]
tool = "Bash"
args = "command:rm *"
level = "block"

[[hooks]]
event = "before_tool"
command = "/usr/local/bin/audit.sh"
timeout_ms = 5000
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.permissions.rules.len(), 2);
        assert_eq!(settings.permissions.rules[0].tool, "Read");
        assert_eq!(
            settings.permissions.rules[0].level,
            chet_permissions::PermissionLevel::Permit
        );
        assert_eq!(settings.permissions.rules[1].tool, "Bash");
        assert_eq!(
            settings.permissions.rules[1].args.as_deref(),
            Some("command:rm *")
        );
        assert_eq!(
            settings.permissions.rules[1].level,
            chet_permissions::PermissionLevel::Block
        );
        assert_eq!(settings.hooks.len(), 1);
        assert_eq!(
            settings.hooks[0].event,
            chet_permissions::HookEvent::BeforeTool
        );
        assert_eq!(settings.hooks[0].timeout_ms, 5000);
    }

    #[test]
    fn test_settings_with_retry() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"

[api.retry]
max_retries = 5
initial_delay_ms = 500
max_delay_ms = 30000
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.api.retry.max_retries, Some(5));
        assert_eq!(settings.api.retry.initial_delay_ms, Some(500));
        assert_eq!(settings.api.retry.max_delay_ms, Some(30000));
    }

    #[test]
    fn test_settings_retry_defaults_when_missing() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.api.retry.max_retries.is_none());
        assert!(settings.api.retry.initial_delay_ms.is_none());
        assert!(settings.api.retry.max_delay_ms.is_none());
    }

    #[test]
    fn test_settings_missing_permissions_defaults_to_empty() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.permissions.rules.is_empty());
        assert!(settings.hooks.is_empty());
    }

    #[test]
    fn test_settings_with_mcp_section() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"

[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_xxxx" }
timeout_ms = 60000
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.mcp.servers.len(), 2);
        let fs = &settings.mcp.servers["filesystem"];
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args.len(), 3);
        assert_eq!(fs.timeout_ms, 30000); // default
        let gh = &settings.mcp.servers["github"];
        assert_eq!(gh.env["GITHUB_TOKEN"], "ghp_xxxx");
        assert_eq!(gh.timeout_ms, 60000);
    }

    #[test]
    fn test_settings_with_effort() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
effort = "high"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.api.effort, Some(Effort::High));
    }

    #[test]
    fn test_settings_effort_defaults_to_none() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.api.effort, None);
    }

    #[test]
    fn test_settings_missing_mcp_defaults_to_empty() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.mcp.servers.is_empty());
    }

    #[test]
    fn test_default_max_tokens_is_64k() {
        assert_eq!(DEFAULT_MAX_TOKENS, 65536);
    }

    #[test]
    fn test_settings_with_memory_dir() {
        let toml_str = r#"
memory_dir = "/custom/memory/path"

[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.memory_dir.as_deref(), Some("/custom/memory/path"));
    }

    #[test]
    fn test_settings_memory_dir_defaults_to_none() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.memory_dir.is_none());
    }

    #[test]
    fn test_settings_with_agents() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"

[agents.reviewer]
effort = "high"
max_turns = 10
disallowed_tools = ["Write", "Edit"]
system_prompt = "You are a code reviewer."

[agents.fast]
effort = "low"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.agents.len(), 2);
        let reviewer = &settings.agents["reviewer"];
        assert_eq!(reviewer.effort, Some(Effort::High));
        assert_eq!(reviewer.max_turns, Some(10));
        assert_eq!(reviewer.disallowed_tools, vec!["Write", "Edit"]);
        assert!(
            reviewer
                .system_prompt
                .as_deref()
                .unwrap()
                .contains("code reviewer")
        );
        let fast = &settings.agents["fast"];
        assert_eq!(fast.effort, Some(Effort::Low));
        assert!(fast.disallowed_tools.is_empty());
    }

    #[test]
    fn test_settings_agents_defaults_to_empty() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.agents.is_empty());
    }

    #[test]
    fn test_settings_with_model_aliases() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"

[models]
fast = "claude-haiku-4-5-20251001"
smart = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.models.len(), 2);
        assert_eq!(settings.models["fast"], "claude-haiku-4-5-20251001");
    }

    #[test]
    fn test_settings_with_auth_token() {
        let toml_str = r#"
[api]
auth_token = "my-bearer-token"
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.api.auth_token.as_deref(), Some("my-bearer-token"));
        assert!(settings.api.api_key.is_none());
    }

    #[test]
    fn test_settings_auth_token_defaults_to_none() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.api.auth_token.is_none());
    }

    #[test]
    fn test_settings_models_defaults_to_empty() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.models.is_empty());
    }
}
