//! Multi-tier TOML configuration for Chet.
//!
//! Reads configuration from multiple sources with precedence:
//! env vars > project > global > defaults

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The default Anthropic API base URL.
pub const DEFAULT_API_BASE_URL: &str = "https://api.anthropic.com";

/// The default model to use.
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";

/// The default max tokens for a response.
pub const DEFAULT_MAX_TOKENS: u32 = 16384;

/// Resolved configuration for a Chet session.
#[derive(Debug, Clone)]
pub struct ChetConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub api_base_url: String,
    pub thinking_budget: Option<u32>,
    pub config_dir: PathBuf,
    pub permission_rules: Vec<chet_permissions::PermissionRule>,
    pub hooks: Vec<chet_permissions::HookConfig>,
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
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub base_url: Option<String>,
    pub thinking_budget: Option<u32>,
}

/// CLI overrides that take highest precedence.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<u32>,
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
        let config_dir = config_dir();
        let global_settings = load_settings_file(&config_dir.join("config.toml"));

        // Resolve API key: CLI > env > config file
        let api_key = overrides
            .api_key
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .or(global_settings.api.api_key)
            .ok_or_else(|| chet_types::ConfigError::MissingKey {
                key: "api_key (set ANTHROPIC_API_KEY or add to ~/.chet/config.toml)".into(),
            })?;

        // Resolve model
        let model = overrides
            .model
            .or_else(|| std::env::var("CHET_MODEL").ok())
            .or(global_settings.api.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

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

        Ok(ChetConfig {
            api_key,
            model,
            max_tokens,
            api_base_url,
            thinking_budget,
            permission_rules: global_settings.permissions.rules,
            hooks: global_settings.hooks,
            config_dir,
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
    fn test_settings_missing_permissions_defaults_to_empty() {
        let toml_str = r#"
[api]
model = "claude-opus-4-6"
"#;
        let settings: SettingsFile = toml::from_str(toml_str).unwrap();
        assert!(settings.permissions.rules.is_empty());
        assert!(settings.hooks.is_empty());
    }
}
