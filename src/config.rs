//! Configuration management for Chatify client.
//!
//! Provides persistent settings storage with sensible defaults and cross-platform support.

use crate::ui::theme::{CustomTheme, OwnedTheme};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Main configuration structure for Chatify client
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub connection: ConnectionConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub completion: CompletionConfig,
    #[serde(default)]
    pub session: SessionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    #[serde(default = "default_host")]
    pub default_host: String,
    #[serde(default = "default_port")]
    pub default_port: u16,
    #[serde(default)]
    pub use_tls: bool,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub enable_markdown: bool,
    #[serde(default = "default_true")]
    pub enable_syntax_highlighting: bool,
    #[serde(default = "default_true")]
    pub enable_emoji: bool,
    #[serde(default)]
    pub compact_mode: bool,
    #[serde(default)]
    pub custom_themes: Vec<CustomThemeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomThemeConfig {
    pub name: String,
    #[serde(default = "default_custom_header")]
    pub header: String,
    #[serde(default = "default_custom_subtitle")]
    pub subtitle: String,
    #[serde(default = "default_custom_feed_text")]
    pub feed_text: String,
    #[serde(default = "default_custom_sidebar_text")]
    pub sidebar_text: String,
    #[serde(default = "default_custom_hint")]
    pub hint: String,
    #[serde(default = "default_custom_dim")]
    pub dim: String,
    #[serde(default = "default_custom_accent")]
    pub accent: String,
    #[serde(default = "default_custom_border")]
    pub border: String,
    #[serde(default = "default_custom_error")]
    pub error: String,
    #[serde(default = "default_custom_success")]
    pub success: String,
}

fn default_custom_header() -> String {
    "147".into()
}
fn default_custom_subtitle() -> String {
    "245".into()
}
fn default_custom_feed_text() -> String {
    "117".into()
}
fn default_custom_sidebar_text() -> String {
    "120".into()
}
fn default_custom_hint() -> String {
    "220".into()
}
fn default_custom_dim() -> String {
    "245".into()
}
fn default_custom_accent() -> String {
    "147".into()
}
fn default_custom_border() -> String {
    "240".into()
}
fn default_custom_error() -> String {
    "196".into()
}
fn default_custom_success() -> String {
    "82".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub on_dm: bool,
    #[serde(default = "default_true")]
    pub on_mention: bool,
    #[serde(default)]
    pub on_all_messages: bool,
    #[serde(default)]
    pub sound_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub show_inline_hints: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub remember_username: bool,
    #[serde(default)]
    pub last_username: String,
    #[serde(default = "default_true")]
    pub remember_channel: bool,
    #[serde(default = "default_channel")]
    pub last_channel: String,
}

// Default value functions
fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8765
}

fn default_theme() -> String {
    "retro-grid".to_string()
}

fn default_channel() -> String {
    "general".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            default_host: default_host(),
            default_port: default_port(),
            use_tls: false,
            auto_reconnect: true,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            enable_markdown: true,
            enable_syntax_highlighting: true,
            enable_emoji: true,
            compact_mode: false,
            custom_themes: Vec::new(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_dm: true,
            on_mention: true,
            on_all_messages: false,
            sound_enabled: false,
        }
    }
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_inline_hints: true,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            remember_username: true,
            last_username: String::new(),
            remember_channel: true,
            last_channel: default_channel(),
        }
    }
}

impl Config {
    /// Returns the platform-specific configuration directory path
    /// Windows: %APPDATA%\Chatify
    /// Unix: ~/.chatify
    pub fn config_dir() -> Option<PathBuf> {
        if cfg!(windows) {
            dirs::data_dir().map(|p| p.join("Chatify"))
        } else {
            dirs::home_dir().map(|p| p.join(".chatify"))
        }
    }

    /// Returns the full path to the configuration file
    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|p| p.join("config.toml"))
    }

    /// Loads configuration from file, falling back to defaults on error
    pub fn load() -> Self {
        match Self::config_path() {
            Some(path) => {
                if path.exists() {
                    match fs::read_to_string(&path) {
                        Ok(contents) => match toml::from_str(&contents) {
                            Ok(config) => {
                                log::info!("Loaded configuration from {}", path.display());
                                config
                            }
                            Err(e) => {
                                log::warn!(
                                    "Failed to parse config file {}: {}. Using defaults.",
                                    path.display(),
                                    e
                                );
                                Self::default()
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "Failed to read config file {}: {}. Using defaults.",
                                path.display(),
                                e
                            );
                            Self::default()
                        }
                    }
                } else {
                    log::info!(
                        "No config file found at {}. Using defaults.",
                        path.display()
                    );
                    Self::default()
                }
            }
            None => {
                log::warn!("Could not determine config directory. Using defaults.");
                Self::default()
            }
        }
    }

    /// Saves the current configuration to file
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path()
            .ok_or_else(|| "Could not determine config file path".to_string())?;

        // Ensure config directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        let contents = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        fs::write(&path, contents).map_err(|e| format!("Failed to write config file: {}", e))?;

        log::info!("Saved configuration to {}", path.display());
        Ok(())
    }

    /// Updates a configuration value by key path (e.g., "ui.theme", "notifications.enabled")
    pub fn set_value(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            // Connection
            "connection.default_host" => {
                self.connection.default_host = value.to_string();
            }
            "connection.default_port" => {
                self.connection.default_port = value
                    .parse()
                    .map_err(|_| format!("Invalid port number: {}", value))?;
            }
            "connection.use_tls" => {
                self.connection.use_tls = parse_bool(value)?;
            }
            "connection.auto_reconnect" => {
                self.connection.auto_reconnect = parse_bool(value)?;
            }

            // UI
            "ui.theme" => {
                self.ui.theme = value.to_string();
            }
            "ui.enable_markdown" => {
                self.ui.enable_markdown = parse_bool(value)?;
            }
            "ui.enable_syntax_highlighting" => {
                self.ui.enable_syntax_highlighting = parse_bool(value)?;
            }
            "ui.enable_emoji" => {
                self.ui.enable_emoji = parse_bool(value)?;
            }
            "ui.compact_mode" => {
                self.ui.compact_mode = parse_bool(value)?;
            }

            // Notifications
            "notifications.enabled" => {
                self.notifications.enabled = parse_bool(value)?;
            }
            "notifications.on_dm" => {
                self.notifications.on_dm = parse_bool(value)?;
            }
            "notifications.on_mention" => {
                self.notifications.on_mention = parse_bool(value)?;
            }
            "notifications.on_all_messages" => {
                self.notifications.on_all_messages = parse_bool(value)?;
            }
            "notifications.sound_enabled" => {
                self.notifications.sound_enabled = parse_bool(value)?;
            }

            // Completion
            "completion.enabled" => {
                self.completion.enabled = parse_bool(value)?;
            }
            "completion.show_inline_hints" => {
                self.completion.show_inline_hints = parse_bool(value)?;
            }

            // Session
            "session.remember_username" => {
                self.session.remember_username = parse_bool(value)?;
            }
            "session.last_username" => {
                self.session.last_username = value.to_string();
            }
            "session.remember_channel" => {
                self.session.remember_channel = parse_bool(value)?;
            }
            "session.last_channel" => {
                self.session.last_channel = value.to_string();
            }

            _ => return Err(format!("Unknown configuration key: {}", key)),
        }

        Ok(())
    }

    /// Returns a human-readable summary of the current configuration
    pub fn summary(&self) -> String {
        let custom_theme_names: Vec<&str> = self
            .ui
            .custom_themes
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        format!(
            r#"Current Configuration:

[connection]
  default_host = "{}"
  default_port = {}
  use_tls = {}
  auto_reconnect = {}

[ui]
  theme = "{}"
  enable_markdown = {}
  enable_syntax_highlighting = {}
  enable_emoji = {}
  compact_mode = {}
  custom_themes = [{}]

[notifications]
  enabled = {}
  on_dm = {}
  on_mention = {}
  on_all_messages = {}
  sound_enabled = {}

[completion]
  enabled = {}
  show_inline_hints = {}

[session]
  remember_username = {}
  last_username = "{}"
  remember_channel = {}
  last_channel = "{}"

Config file: {}"#,
            self.connection.default_host,
            self.connection.default_port,
            self.connection.use_tls,
            self.connection.auto_reconnect,
            self.ui.theme,
            self.ui.enable_markdown,
            self.ui.enable_syntax_highlighting,
            self.ui.enable_emoji,
            self.ui.compact_mode,
            custom_theme_names.join(", "),
            self.notifications.enabled,
            self.notifications.on_dm,
            self.notifications.on_mention,
            self.notifications.on_all_messages,
            self.notifications.sound_enabled,
            self.completion.enabled,
            self.completion.show_inline_hints,
            self.session.remember_username,
            self.session.last_username,
            self.session.remember_channel,
            self.session.last_channel,
            Self::config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(unknown)".to_string())
        )
    }

    /// Resolves the active theme from config, checking custom themes first.
    pub fn resolve_theme(&self) -> OwnedTheme {
        let custom: Vec<CustomTheme> = self
            .ui
            .custom_themes
            .iter()
            .map(|ct| CustomTheme {
                name: ct.name.clone(),
                header: ct.header.clone(),
                subtitle: ct.subtitle.clone(),
                feed_text: ct.feed_text.clone(),
                sidebar_text: ct.sidebar_text.clone(),
                hint: ct.hint.clone(),
                dim: ct.dim.clone(),
                accent: ct.accent.clone(),
                border: ct.border.clone(),
                error: ct.error.clone(),
                success: ct.success.clone(),
            })
            .collect();
        OwnedTheme::resolve(&self.ui.theme, &custom)
    }
}

/// Helper to parse boolean values from strings
fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" | "enabled" => Ok(true),
        "false" | "no" | "0" | "off" | "disabled" => Ok(false),
        _ => Err(format!("Invalid boolean value: {}", value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.connection.default_host, "127.0.0.1");
        assert_eq!(config.connection.default_port, 8765);
        assert!(config.ui.enable_markdown);
        assert!(config.notifications.enabled);
    }

    #[test]
    fn test_parse_bool() {
        assert!(parse_bool("true").unwrap());
        assert!(parse_bool("yes").unwrap());
        assert!(parse_bool("1").unwrap());
        assert!(!parse_bool("false").unwrap());
        assert!(!parse_bool("no").unwrap());
        assert!(parse_bool("invalid").is_err());
    }

    #[test]
    fn test_set_value() {
        let mut config = Config::default();
        config.set_value("ui.theme", "dark").unwrap();
        assert_eq!(config.ui.theme, "dark");

        config.set_value("notifications.enabled", "false").unwrap();
        assert!(!config.notifications.enabled);

        assert!(config.set_value("invalid.key", "value").is_err());
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = Config::default();
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(
            config.connection.default_host,
            deserialized.connection.default_host
        );
        assert_eq!(config.ui.theme, deserialized.ui.theme);
    }

    #[test]
    fn test_custom_theme_serialization() {
        let config_str = r#"
[ui]
theme = "my-theme"

[[ui.custom_themes]]
name = "my-theme"
header = "99"
subtitle = "100"
feed_text = "101"
sidebar_text = "102"
hint = "103"
dim = "104"
accent = "105"
border = "106"
error = "107"
success = "108"
"#;
        let config: Config = toml::from_str(config_str).unwrap();
        assert_eq!(config.ui.theme, "my-theme");
        assert_eq!(config.ui.custom_themes.len(), 1);
        assert_eq!(config.ui.custom_themes[0].name, "my-theme");
        assert_eq!(config.ui.custom_themes[0].header, "99");
    }

    #[test]
    fn test_resolve_theme_builtin() {
        let config = Config::default();
        let theme = config.resolve_theme();
        assert_eq!(theme.name, "retro-grid");
    }

    #[test]
    fn test_resolve_theme_custom() {
        let config_str = r#"
[ui]
theme = "my-theme"

[[ui.custom_themes]]
name = "my-theme"
header = "200"
subtitle = "201"
feed_text = "202"
sidebar_text = "203"
hint = "204"
dim = "205"
accent = "206"
border = "207"
error = "208"
success = "209"
"#;
        let config: Config = toml::from_str(config_str).unwrap();
        let theme = config.resolve_theme();
        assert_eq!(theme.name, "my-theme");
        assert_eq!(theme.header, "\x1b[38;5;200m");
    }
}
