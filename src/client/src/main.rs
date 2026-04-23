//! Terminal UI client for Chatify.
//!
//! This is the primary user-facing application—a full-featured, async terminal dashboard
//! that connects to the Chatify WebSocket server. It handles:
//!
//! - **Connection management**: WebSocket reconnection, auth, TLS if configured
//! - **Terminal UI**: Real-time message display, command input, presence updates
//! - **Encryption**: E2E DMs using received public keys, per-channel encryption keys
//! - **Voice**: Audio relay and spatial awareness in voice rooms
//! - **Media**: Inline rendering of shared images/videos with fallback to links
//! - **Search & history**: Full-text search and conversation replay
//! - **Plugins**: Runtime loading of Lua scripts for extensibility
//! - **Notifications**: Desktop alerts, sound, and text-to-speech support
//! - **2FA**: TOTP verification and backup code entry flows
//!
//! # Architecture
//!
//! The client uses a decoupled actor-like pattern:
//! - **UI layer** (`chatify_client::ui`): Terminal rendering, theme, markdown
//! - **State layer** (`ClientState`): Ephemeral session, message history, presence
//! - **Handlers** (`chatify_client::handlers`): Business logic for commands and protocol events
//! - **Protocol**: WebSocket communication with first-frame auth guarantee
//!
//! Session tokens are ephemeral—they don't survive server restarts, so all clients
//! must re-authenticate. See AGENTS.md for protocol constraints.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chatify::crypto::{new_keypair, pub_b64, pw_hash_client};
use chatify::notifications::NotificationService;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
#[allow(unused_imports)]
use log::info;
use rodio::{Decoder, Source};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{lookup_host, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, Message},
};

use chatify::config::Config;
use chatify::error::{ChatifyError, ChatifyResult};

use chatify_client::{
    args::Args,
    handlers,
    media::{guess_mime_from_path, MediaKind},
    state::{ClientState, OutgoingMediaMeta, SharedState},
    voice::{start_voice_session, VoiceEvent},
};

/// Emit a line of output to the UI (with newline).
///
/// Unlike std::println!, this respects the UI framework's line management and
/// integrates with the terminal dashboard. Avoids stdout contention.
macro_rules! println {
    ($($arg:tt)*) => {{
        chatify_client::ui::emit_output_line(format!($($arg)*), false);
    }};
}

/// Emit a line of error/diagnostic output to the UI.
///
/// Same as println! but semantically marked as an error stream for styling.
macro_rules! eprintln {
    ($($arg:tt)*) => {{
        chatify_client::ui::emit_output_line(format!($($arg)*), true);
    }};
}

const DEFAULT_HISTORY_LIMIT: usize = 50;
const MAX_HISTORY_LIMIT: usize = 500;
const DEFAULT_SEARCH_LIMIT: usize = 50;
const MAX_SEARCH_LIMIT: usize = 200;
const DEFAULT_REPLAY_LIMIT: usize = 1000;
const MAX_REPLAY_LIMIT: usize = 5000;
const DEFAULT_RECENT_LIMIT: usize = 10;
const MAX_RECENT_LIMIT: usize = 50;
const DEFAULT_REACTION_SYNC_LIMIT: usize = 500;
const DEFAULT_TRUST_AUDIT_LIMIT: usize = 20;
const MAX_TRUST_AUDIT_LIMIT: usize = 200;
const MAX_OFFLINE_OUTBOUND_QUEUE: usize = 256;
const RECONNECT_BASE_DELAY_SECS: u64 = 1;
const RECONNECT_MAX_DELAY_SECS: u64 = 30;

#[derive(Clone)]
struct ConnectionAuth {
    username: String,
    password_hash: String,
    public_key: String,
}

#[derive(Clone, Copy)]
struct CommandHelp {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    aliases: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PluginCommand {
    List,
    Install(String),
    Disable(String),
}

#[cfg(feature = "bridge-client")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeCommand {
    Status,
}

const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        name: "/commands",
        usage: "/commands [filter]",
        summary: "List commands, optionally filtered by keyword",
        aliases: &[],
    },
    CommandHelp {
        name: "/help",
        usage: "/help [command]",
        summary: "Show general help or detailed help for one command",
        aliases: &[],
    },
    CommandHelp {
        name: "/join",
        usage: "/join <channel>",
        summary: "Join or switch to a channel",
        aliases: &["/switch"],
    },
    CommandHelp {
        name: "/leave",
        usage: "/leave [channel]",
        summary: "Leave a channel (default: current channel)",
        aliases: &["/part"],
    },
    CommandHelp {
        name: "/history",
        usage: "/history [ch|dm:user] [limit]",
        summary: "Load channel or DM history",
        aliases: &[],
    },
    CommandHelp {
        name: "/search",
        usage: "/search [#ch|dm:user] <query> [limit=N]",
        summary: "Search timeline events",
        aliases: &[],
    },
    CommandHelp {
        name: "/replay",
        usage: "/replay <from_ts> [#ch|dm:user] [limit=N]",
        summary: "Replay events from a timestamp",
        aliases: &[],
    },
    CommandHelp {
        name: "/users",
        usage: "/users",
        summary: "Refresh online users and key directory",
        aliases: &[],
    },
    CommandHelp {
        name: "/metrics",
        usage: "/metrics",
        summary: "Show runtime and database metrics",
        aliases: &[],
    },
    CommandHelp {
        name: "/db-profile",
        usage: "/db-profile",
        summary: "Show focused database latency profile",
        aliases: &["/dbprofile"],
    },
    CommandHelp {
        name: "/doctor",
        usage: "/doctor [--json]",
        summary: "Run connection diagnostics (network, TLS, auth readiness, latency)",
        aliases: &[],
    },
    CommandHelp {
        name: "/typing",
        usage: "/typing [on|off] [#ch|dm:user]",
        summary: "Broadcast ephemeral typing state",
        aliases: &[],
    },
    CommandHelp {
        name: "/voice",
        usage: "/voice <on|off|mute|unmute|deafen|undeafen|status> [room]",
        summary: "Control voice session and signaling",
        aliases: &["/vc"],
    },
    CommandHelp {
        name: "/screen",
        usage: "/screen <start|stop|status> [room]",
        summary: "Control screen-share signaling",
        aliases: &["/ss"],
    },
    CommandHelp {
        name: "/notify",
        usage: "/notify [target] [on|off] | reset | export [--redact] [path|stdout] | doctor [--json] | test [sound] [level] [message]",
        summary: "Show or update desktop notification preferences",
        aliases: &[],
    },
    CommandHelp {
        name: "/plugin",
        usage: "/plugin [list|install <plugin>|disable <plugin>]",
        summary: "List, install, or disable server-side plugins",
        aliases: &[],
    },
    #[cfg(feature = "bridge-client")]
    CommandHelp {
        name: "/bridge",
        usage: "/bridge status",
        summary: "Show connected bridge instances and route health",
        aliases: &[],
    },
    CommandHelp {
        name: "/dm",
        usage: "/dm <user> <message>",
        summary: "Send direct message (trust-verified)",
        aliases: &[],
    },
    CommandHelp {
        name: "/fingerprint",
        usage: "/fingerprint [user]",
        summary: "Show key fingerprint(s) and trust state",
        aliases: &[],
    },
    CommandHelp {
        name: "/trust",
        usage: "/trust <user> <fingerprint>",
        summary: "Mark peer fingerprint as trusted",
        aliases: &[],
    },
    CommandHelp {
        name: "/trust-audit",
        usage: "/trust-audit [n]",
        summary: "Show recent trust audit entries",
        aliases: &[],
    },
    CommandHelp {
        name: "/trust-export",
        usage: "/trust-export [path]",
        summary: "Export deterministic trust audit JSON",
        aliases: &[],
    },
    CommandHelp {
        name: "/recent",
        usage: "/recent [n]",
        summary: "Show recent message IDs",
        aliases: &[],
    },
    CommandHelp {
        name: "/react",
        usage: "/react <msg_id|#index> <emoji>",
        summary: "React to a message",
        aliases: &[],
    },
    CommandHelp {
        name: "/sync",
        usage: "/sync",
        summary: "Request reaction sync for active channel",
        aliases: &[],
    },
    CommandHelp {
        name: "/image",
        usage: "/image \"<path>\"",
        summary: "Send an image file to the current channel",
        aliases: &[],
    },
    CommandHelp {
        name: "/video",
        usage: "/video \"<path>\"",
        summary: "Send a video file to the current channel",
        aliases: &[],
    },
    CommandHelp {
        name: "/audio",
        usage: "/audio \"<path>\"",
        summary: "Send a short audio note to the current channel",
        aliases: &[],
    },
    CommandHelp {
        name: "/quit",
        usage: "/quit",
        summary: "Exit client",
        aliases: &["/exit", "/q"],
    },
];

fn is_valid_reaction_emoji(emoji: &str) -> bool {
    let trimmed = emoji.trim();
    !trimmed.is_empty() && trimmed.len() <= 32
}

fn prompt_input(label: &str, default: Option<&str>) -> ChatifyResult<String> {
    if let Some(default_value) = default {
        print!("{} [{}]: ", label, default_value);
    } else {
        print!("{}: ", label);
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(value.to_string())
    }
}

fn sanitize_username(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}

fn resolve_known_username(users: &HashMap<String, String>, raw: &str) -> Result<String, String> {
    let query = raw.trim().trim_start_matches('@');
    if query.is_empty() {
        return Err("user cannot be empty".to_string());
    }

    if users.contains_key(query) {
        return Ok(query.to_string());
    }

    let needle = query.to_ascii_lowercase();
    let mut matches: Vec<String> = users
        .keys()
        .filter(|candidate| candidate.to_ascii_lowercase() == needle)
        .cloned()
        .collect();

    match matches.len() {
        0 => Err(format!(
            "unknown user '{}'; run /users to refresh directory",
            query
        )),
        1 => Ok(matches.remove(0)),
        _ => {
            matches.sort();
            Err(format!(
                "ambiguous user '{}'; matches: {}",
                query,
                matches.join(", ")
            ))
        }
    }
}

fn trust_status_label(state: &ClientState, user: &str, observed_fingerprint: &str) -> &'static str {
    match state.trust_store.peers.get(user) {
        Some(peer) if peer.fingerprint == observed_fingerprint && peer.verified => "trusted",
        Some(peer) if peer.fingerprint == observed_fingerprint => "known",
        Some(_) => "key-changed",
        None => "untrusted",
    }
}

fn is_explicit_scope_token(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.starts_with('#') || trimmed.to_ascii_lowercase().starts_with("dm:")
}

fn normalize_scope_token(raw: &str, fallback_channel: &str, allow_plain_channel: bool) -> String {
    let trimmed = raw.trim();
    if let Some(peer_raw) = trimmed.strip_prefix("dm:") {
        let peer = peer_raw.trim().to_ascii_lowercase();
        return format!("dm:{}", peer);
    }

    if trimmed.starts_with('#') || allow_plain_channel {
        let channel_raw = trimmed.trim_start_matches('#');
        return chatify::normalize_channel(channel_raw)
            .unwrap_or_else(|| fallback_channel.to_string());
    }

    fallback_channel.to_string()
}

fn parse_limit_token(raw: &str) -> Option<usize> {
    raw.strip_prefix("limit=")
        .or_else(|| raw.strip_prefix("l="))
        .and_then(|v| v.parse::<usize>().ok())
}

fn parse_replay_timestamp(raw: &str) -> Option<f64> {
    let ts = raw.parse::<f64>().ok()?;
    if ts.is_finite() && ts >= 0.0 {
        Some(ts)
    } else {
        None
    }
}

fn parse_toggle_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "y" => Some(true),
        "off" | "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

fn notification_key_for_token(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "enabled" => Some("notifications.enabled"),
        "dm" | "on_dm" => Some("notifications.on_dm"),
        "mention" | "on_mention" => Some("notifications.on_mention"),
        "all" | "all_messages" | "on_all_messages" => Some("notifications.on_all_messages"),
        "sound" | "sound_enabled" => Some("notifications.sound_enabled"),
        _ => None,
    }
}

fn notification_value_for_key(
    cfg: &chatify::config::NotificationConfig,
    key: &str,
) -> Option<bool> {
    match key {
        "notifications.enabled" => Some(cfg.enabled),
        "notifications.on_dm" => Some(cfg.on_dm),
        "notifications.on_mention" => Some(cfg.on_mention),
        "notifications.on_all_messages" => Some(cfg.on_all_messages),
        "notifications.sound_enabled" => Some(cfg.sound_enabled),
        _ => None,
    }
}

fn parse_notify_probe_level(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "info" => Some("INFO"),
        "warning" | "warn" => Some("WARNING"),
        "critical" | "crit" | "error" => Some("CRITICAL"),
        _ => None,
    }
}

fn parse_notify_test_args(tokens: &[&str]) -> (bool, &'static str, usize) {
    let mut sound_probe = false;
    let mut level = "INFO";
    let mut message_start = 1usize;

    if let Some(raw) = tokens.get(message_start) {
        if raw.eq_ignore_ascii_case("sound") {
            sound_probe = true;
            message_start += 1;
        }
    }

    if let Some(raw) = tokens.get(message_start) {
        if let Some(parsed_level) = parse_notify_probe_level(raw) {
            level = parsed_level;
            message_start += 1;
        }
    }

    (sound_probe, level, message_start)
}

fn parse_shell_like_argument(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let value = if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let last = bytes[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_plugin_command(input: &str) -> Result<PluginCommand, &'static str> {
    let rest = input
        .trim()
        .strip_prefix("/plugin")
        .map(str::trim)
        .ok_or("Usage: /plugin [list|install <plugin>|disable <plugin>]")?;

    if rest.is_empty() {
        return Ok(PluginCommand::List);
    }

    let subcommand_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let subcommand = &rest[..subcommand_end];
    let argument = rest[subcommand_end..].trim();

    match subcommand.to_ascii_lowercase().as_str() {
        "list" => {
            if argument.is_empty() {
                Ok(PluginCommand::List)
            } else {
                Err("Usage: /plugin [list|install <plugin>|disable <plugin>]")
            }
        }
        "install" => parse_shell_like_argument(argument)
            .map(PluginCommand::Install)
            .ok_or("Usage: /plugin install <plugin>"),
        "disable" => parse_shell_like_argument(argument)
            .map(PluginCommand::Disable)
            .ok_or("Usage: /plugin disable <plugin>"),
        _ => Err("Usage: /plugin [list|install <plugin>|disable <plugin>]"),
    }
}

fn media_kind_for_command(command: &str) -> Option<MediaKind> {
    match command {
        "/image" => Some(MediaKind::Image),
        "/video" => Some(MediaKind::Video),
        "/audio" => Some(MediaKind::Audio),
        _ => None,
    }
}

fn detect_audio_duration_ms(path: &Path) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let decoder = Decoder::new(std::io::BufReader::new(file)).ok()?;
    decoder
        .total_duration()
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
}

#[cfg(feature = "bridge-client")]
fn parse_bridge_command(input: &str) -> Result<BridgeCommand, &'static str> {
    let rest = input
        .trim()
        .strip_prefix("/bridge")
        .map(str::trim)
        .ok_or("Usage: /bridge status")?;

    if rest.eq_ignore_ascii_case("status") {
        Ok(BridgeCommand::Status)
    } else {
        Err("Usage: /bridge status")
    }
}

fn build_notification_export(
    cfg: &chatify::config::NotificationConfig,
    profile_user: &str,
    host: &str,
    port: u16,
    tls: bool,
) -> serde_json::Value {
    let normalized_user = if profile_user.trim().is_empty() {
        "anonymous".to_string()
    } else {
        profile_user.trim().to_string()
    };

    let normalized_host = if host.trim().is_empty() {
        "127.0.0.1".to_string()
    } else {
        host.trim().to_string()
    };

    serde_json::json!({
        "schema_version": 1,
        "profile": {
            "user": normalized_user,
            "host": normalized_host,
            "port": port,
            "tls": tls,
        },
        "notifications": {
            "enabled": cfg.enabled,
            "on_dm": cfg.on_dm,
            "on_mention": cfg.on_mention,
            "on_all_messages": cfg.on_all_messages,
            "sound_enabled": cfg.sound_enabled,
        }
    })
}

fn redact_notification_export(payload: &serde_json::Value) -> serde_json::Value {
    let mut redacted = payload.clone();
    if let Some(profile) = redacted.get_mut("profile").and_then(|v| v.as_object_mut()) {
        profile.insert(
            "user".to_string(),
            serde_json::Value::String("redacted".to_string()),
        );
        profile.insert(
            "host".to_string(),
            serde_json::Value::String("redacted".to_string()),
        );
    }
    redacted
}

fn sanitize_notify_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }

    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

fn notify_export_default_path(profile_user: &str, host: &str, port: u16, tls: bool) -> PathBuf {
    let user_component = sanitize_notify_component(profile_user.trim());
    let host_component = sanitize_notify_component(host.trim());
    let mode = if tls { "tls" } else { "plain" };
    let file_name = format!(
        "notify-export-{}-{}-{}-{}.json",
        user_component, host_component, port, mode
    );

    let base = Config::config_dir().unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".chatify")
    });
    base.join("exports").join(file_name)
}

fn notify_recommendations(cfg: &chatify::config::NotificationConfig) -> Vec<String> {
    let mut recommendations = Vec::new();
    if !cfg.enabled {
        recommendations.push(
            "enable notifications with '/notify enabled on' to receive desktop alerts".to_string(),
        );
    }

    if cfg.enabled && !cfg.on_dm && !cfg.on_mention && !cfg.on_all_messages {
        recommendations
            .push("all delivery flags are off; enable one of dm/mention/all".to_string());
    }

    if !cfg.sound_enabled {
        recommendations.push(
            "enable sound with '/notify sound on' before using '/notify test sound'".to_string(),
        );
    }

    recommendations
}

fn build_notify_diagnostics_json(
    cfg: &chatify::config::NotificationConfig,
    profile_user: &str,
    host: &str,
    port: u16,
    tls: bool,
) -> serde_json::Value {
    let user = if profile_user.trim().is_empty() {
        "anonymous".to_string()
    } else {
        profile_user.trim().to_string()
    };

    let normalized_host = if host.trim().is_empty() {
        "127.0.0.1".to_string()
    } else {
        host.trim().to_string()
    };

    let config_path = Config::config_path();
    let config_path_text = config_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unavailable)".to_string());
    let config_dir_state = config_path
        .as_ref()
        .and_then(|path| path.parent().map(|p| p.exists()))
        .map(|exists| if exists { "exists" } else { "missing" }.to_string())
        .unwrap_or_else(|| "(no parent)".to_string());

    let default_export_path = notify_export_default_path(&user, &normalized_host, port, tls)
        .display()
        .to_string();

    serde_json::json!({
        "schema_version": 1,
        "profile": {
            "user": user,
            "host": normalized_host,
            "port": port,
            "tls": tls,
        },
        "notifications": {
            "enabled": cfg.enabled,
            "on_dm": cfg.on_dm,
            "on_mention": cfg.on_mention,
            "on_all_messages": cfg.on_all_messages,
            "sound_enabled": cfg.sound_enabled,
        },
        "config_path": config_path_text,
        "config_dir_state": config_dir_state,
        "default_export_path": default_export_path,
        "recommendations": notify_recommendations(cfg),
    })
}

fn print_notification_settings(cfg: &chatify::config::NotificationConfig) {
    println!(
        "Notifications: enabled={} dm={} mention={} all={} sound={}",
        cfg.enabled, cfg.on_dm, cfg.on_mention, cfg.on_all_messages, cfg.sound_enabled
    );
}

fn print_notify_usage() {
    println!("Usage: /notify");
    println!("       /notify [enabled|dm|mention|all|sound]");
    println!("       /notify [enabled|dm|mention|all|sound] [on|off]");
    println!("       /notify reset");
    println!("       /notify doctor [--json]");
    println!("       /notify export [--redact] [path|stdout]");
    println!("       /notify test [sound] [info|warning|critical] [message]");
}

fn print_doctor_usage() {
    println!("Usage: /doctor [--json]");
}

fn millis_u64(elapsed: std::time::Duration) -> u64 {
    elapsed.as_millis().min(u64::MAX as u128) as u64
}

fn connection_doctor_recommendations(
    auth_ready: bool,
    dns_ok: bool,
    tcp_ok: bool,
    websocket_ok: bool,
    tls: bool,
    auto_reconnect: bool,
) -> Vec<String> {
    let mut recommendations = Vec::new();

    if !auth_ready {
        recommendations.push(
            "auth profile is incomplete; reconnect with valid username/password before troubleshooting"
                .to_string(),
        );
    }

    if !dns_ok {
        recommendations.push(
            "DNS resolution failed; verify --host value and local resolver settings".to_string(),
        );
    }

    if dns_ok && !tcp_ok {
        recommendations.push(
            "TCP connect failed; confirm server process is running and firewall allows the target port"
                .to_string(),
        );
    }

    if tcp_ok && !websocket_ok {
        if tls {
            recommendations.push(
                "WebSocket TLS handshake failed; verify certificate chain and server TLS settings"
                    .to_string(),
            );
        } else {
            recommendations.push(
                "WebSocket upgrade failed on plain mode; confirm server protocol and TLS mode match"
                    .to_string(),
            );
        }
    }

    if !auto_reconnect {
        recommendations.push(
            "auto reconnect is disabled; remove --no-reconnect for smoother recovery".to_string(),
        );
    }

    if recommendations.is_empty() {
        recommendations.push("no issues detected by doctor checks".to_string());
    }

    recommendations
}

async fn run_connection_doctor(state: &SharedState) -> serde_json::Value {
    let (username, host, port, tls, auto_reconnect, current_scope, password_hash_present, key_ok) = {
        let state_lock = state.lock().await;
        (
            state_lock.me.trim().to_string(),
            state_lock.client_config.host.trim().to_string(),
            state_lock.client_config.port,
            state_lock.client_config.tls,
            state_lock.client_config.auto_reconnect,
            state_lock.ch.clone(),
            !state_lock.pw.trim().is_empty(),
            pub_b64(&state_lock.priv_key).is_ok(),
        )
    };

    let normalized_user = if username.is_empty() {
        "anonymous".to_string()
    } else {
        username
    };
    let normalized_host = if host.is_empty() {
        "127.0.0.1".to_string()
    } else {
        host
    };
    let uri = format!(
        "{}://{}:{}",
        if tls { "wss" } else { "ws" },
        normalized_host,
        port
    );

    let username_format_ok = sanitize_username(&normalized_user) == normalized_user;
    let auth_ready = username_format_ok && password_hash_present && key_ok;

    let dns_started = Instant::now();
    let (dns_ok, dns_addresses, dns_error) = match timeout(
        Duration::from_secs(4),
        lookup_host((normalized_host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(addresses)) => {
            let dedup: BTreeSet<String> = addresses.map(|addr| addr.ip().to_string()).collect();
            let list: Vec<String> = dedup.into_iter().collect();
            if list.is_empty() {
                (
                    false,
                    list,
                    Some("resolver returned no addresses".to_string()),
                )
            } else {
                (true, list, None)
            }
        }
        Ok(Err(err)) => (false, Vec::new(), Some(err.to_string())),
        Err(_) => (
            false,
            Vec::new(),
            Some("dns resolution timed out after 4s".to_string()),
        ),
    };
    let dns_latency_ms = millis_u64(dns_started.elapsed());

    let tcp_started = Instant::now();
    let (tcp_ok, tcp_latency_ms, tcp_error) = match timeout(
        Duration::from_secs(5),
        TcpStream::connect((normalized_host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            (true, Some(millis_u64(tcp_started.elapsed())), None)
        }
        Ok(Err(err)) => (false, None, Some(err.to_string())),
        Err(_) => (
            false,
            None,
            Some("tcp connect timed out after 5s".to_string()),
        ),
    };

    let ws_started = Instant::now();
    let (websocket_ok, websocket_latency_ms, websocket_error) = if !tcp_ok {
        (
            false,
            None,
            Some("skipped because tcp probe failed".to_string()),
        )
    } else {
        match timeout(Duration::from_secs(8), connect_async(&uri)).await {
            Ok(Ok((mut ws_stream, _))) => {
                let _ = ws_stream.close(None).await;
                (true, Some(millis_u64(ws_started.elapsed())), None)
            }
            Ok(Err(err)) => (false, None, Some(friendly_ws_error(&err, tls))),
            Err(_) => (
                false,
                None,
                Some("websocket handshake timed out after 8s".to_string()),
            ),
        }
    };

    let recommendations = connection_doctor_recommendations(
        auth_ready,
        dns_ok,
        tcp_ok,
        websocket_ok,
        tls,
        auto_reconnect,
    );

    serde_json::json!({
        "schema_version": 1,
        "profile": {
            "user": normalized_user,
            "scope": current_scope,
            "host": normalized_host,
            "port": port,
            "tls": tls,
            "auto_reconnect": auto_reconnect,
            "uri": uri,
        },
        "auth": {
            "ready": auth_ready,
            "username_format_ok": username_format_ok,
            "password_hash_present": password_hash_present,
            "local_keypair_ready": key_ok,
        },
        "dns": {
            "ok": dns_ok,
            "latency_ms": dns_latency_ms,
            "addresses": dns_addresses,
            "error": dns_error,
        },
        "tcp": {
            "ok": tcp_ok,
            "latency_ms": tcp_latency_ms,
            "error": tcp_error,
        },
        "websocket": {
            "ok": websocket_ok,
            "latency_ms": websocket_latency_ms,
            "error": websocket_error,
        },
        "overall": {
            "healthy": auth_ready && dns_ok && tcp_ok && websocket_ok,
        },
        "recommendations": recommendations,
    })
}

fn print_connection_doctor_report(diagnostics: &serde_json::Value) {
    let profile = diagnostics
        .get("profile")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let auth = diagnostics
        .get("auth")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let dns = diagnostics
        .get("dns")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let tcp = diagnostics
        .get("tcp")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let websocket = diagnostics
        .get("websocket")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let dns_addresses = dns
        .get("addresses")
        .and_then(|v| v.as_array())
        .map(|entries| {
            let values: Vec<&str> = entries.iter().filter_map(|v| v.as_str()).collect();
            if values.is_empty() {
                "none".to_string()
            } else {
                values.join(",")
            }
        })
        .unwrap_or_else(|| "none".to_string());
    let dns_error = dns.get("error").and_then(|v| v.as_str()).unwrap_or("-");
    let tcp_error = tcp.get("error").and_then(|v| v.as_str()).unwrap_or("-");
    let websocket_error = websocket
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("-");

    println!("Connection doctor:");
    println!(
        "  profile: user={} scope={} server={}:{} tls={} auto_reconnect={}",
        profile
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("anonymous"),
        profile
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("general"),
        profile
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1"),
        profile.get("port").and_then(|v| v.as_u64()).unwrap_or(0),
        profile
            .get("tls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        profile
            .get("auto_reconnect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    );
    println!(
        "  auth: ready={} username_format={} password_hash={} keypair={}",
        auth.get("ready").and_then(|v| v.as_bool()).unwrap_or(false),
        auth.get("username_format_ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        auth.get("password_hash_present")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        auth.get("local_keypair_ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    );
    println!(
        "  dns: ok={} latency_ms={} addresses={} detail={}",
        dns.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        dns.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        dns_addresses,
        dns_error
    );
    println!(
        "  tcp: ok={} latency_ms={} detail={}",
        tcp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        tcp.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        tcp_error
    );
    println!(
        "  websocket: ok={} latency_ms={} detail={}",
        websocket
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        websocket
            .get("latency_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        websocket_error
    );

    let overall_healthy = diagnostics
        .get("overall")
        .and_then(|v| v.get("healthy"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "  overall: {}",
        if overall_healthy {
            "healthy"
        } else {
            "degraded"
        }
    );

    if let Some(recommendations) = diagnostics
        .get("recommendations")
        .and_then(|v| v.as_array())
    {
        for recommendation in recommendations {
            if let Some(text) = recommendation.as_str() {
                println!("  recommendation: {}", text);
            }
        }
    }
}

fn send_notification_test(
    cfg: &chatify::config::NotificationConfig,
    level: &str,
    message: &str,
    sound_probe: bool,
) {
    let mut probe_cfg = cfg.clone();
    // One-time probe should still work when notifications.enabled is currently off.
    probe_cfg.enabled = true;
    let title = format!("Chatify notification test [{}]", level);
    NotificationService::send(&probe_cfg, &title, message, true);

    if sound_probe {
        if cfg.sound_enabled {
            // Terminal bell fallback as a low-cost probe for sound-enabled setups.
            print!("\x07");
            let _ = io::stdout().flush();
        } else {
            eprintln!("notifications.sound_enabled is off; skipped local sound probe.");
        }
    }
}

fn format_scope_for_help(scope: &str) -> String {
    if scope.starts_with("dm:") {
        scope.to_string()
    } else {
        format!("#{}", scope)
    }
}

fn find_command_help(raw: &str) -> Option<CommandHelp> {
    let needle = raw.trim().trim_start_matches('/').to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }

    COMMANDS.iter().copied().find(|entry| {
        entry
            .name
            .trim_start_matches('/')
            .eq_ignore_ascii_case(&needle)
            || entry
                .aliases
                .iter()
                .any(|alias| alias.trim_start_matches('/').eq_ignore_ascii_case(&needle))
    })
}

fn print_command_help(entry: CommandHelp) {
    println!("Command: {}", entry.name);
    println!("Usage: {}", entry.usage);
    println!("Description: {}", entry.summary);
    if !entry.aliases.is_empty() {
        println!("Aliases: {}", entry.aliases.join(", "));
    }
}

fn print_commands(filter: Option<&str>) {
    let filter = filter.map(|value| value.trim().trim_start_matches('/').to_ascii_lowercase());
    let mut matching: Vec<CommandHelp> = COMMANDS
        .iter()
        .copied()
        .filter(|entry| {
            if let Some(filter) = &filter {
                if filter.is_empty() {
                    return true;
                }

                entry.name.to_ascii_lowercase().contains(filter)
                    || entry.usage.to_ascii_lowercase().contains(filter)
                    || entry.summary.to_ascii_lowercase().contains(filter)
                    || entry
                        .aliases
                        .iter()
                        .any(|alias| alias.to_ascii_lowercase().contains(filter))
            } else {
                true
            }
        })
        .collect();

    matching.sort_by(|a, b| a.name.cmp(b.name));

    if matching.is_empty() {
        if let Some(filter) = &filter {
            println!("No commands matched '{}'.", filter);
        } else {
            println!("No commands available.");
        }
        return;
    }

    println!("Available commands:");
    for entry in matching {
        println!("  {:<38} {}", entry.usage, entry.summary);
    }
}

fn print_help() {
    print_commands(None);
    println!("  {:<38} Send a message to active channel", "<text>");
    println!("Use /help <command> for details.");
}

fn print_prompt() {
    print!("\x1b[32m>\x1b[0m ");
    let _ = io::stdout().flush();
}

async fn print_recent_messages(state: &SharedState, limit: usize) {
    let state_lock = state.lock().await;
    let current_user = state_lock.me.clone();
    let mut shown = 0usize;
    for msg in state_lock.message_history.iter().rev() {
        if msg.id.is_empty() {
            continue;
        }

        let reaction_summary = state_lock.reaction_summary(&msg.id);
        let (display_content, _) =
            handlers::format_content_for_mentions(&msg.content, &current_user);
        let short_id: String = msg.id.chars().take(8).collect();
        if reaction_summary.is_empty() {
            println!("#{} {}: {}", short_id, msg.sender, display_content);
        } else {
            println!(
                "#{} {}: {} {}",
                short_id, msg.sender, display_content, reaction_summary
            );
        }

        shown += 1;
        if shown >= limit {
            break;
        }
    }

    if shown == 0 {
        println!("No message IDs available yet. Send or load history first.");
    }
}

async fn send_input_to_active_scope(state: &SharedState, body: &str) {
    let mut state_lock = state.lock().await;
    let scope = state_lock.ch.clone();
    let username = state_lock.me.clone();

    if let Some(peer_raw) = scope.strip_prefix("dm:") {
        if state_lock.users.is_empty() {
            if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                eprintln!("failed to request users: {}", err);
            }
            println!(
                "No key directory loaded yet; requested /users from server. Retry the DM after refresh."
            );
            return;
        }

        let canonical = match resolve_known_username(&state_lock.users, peer_raw) {
            Ok(user) => user,
            Err(err) => {
                println!("{}", err);
                return;
            }
        };

        let audit_len_before = state_lock.trust_store.audit_log.len();
        match state_lock.ensure_peer_trusted_for_dm(&canonical) {
            Ok(fingerprint) => {
                if let Err(err) = state_lock.send_dm(&canonical, body) {
                    eprintln!("failed to send dm: {}", err);
                } else {
                    state_lock.dismiss_unread_marker(&scope);
                    println!(
                        "[sending] {} {} -> {} ({})",
                        scope,
                        username,
                        canonical,
                        ClientState::format_fingerprint_for_display(&fingerprint)
                    );
                }
            }
            Err(err) => {
                println!("dm blocked by trust policy: {}", err);
                if let Some(pubkey_b64) = state_lock.users.get(&canonical) {
                    if let Some(observed) = ClientState::fingerprint_for_pubkey(pubkey_b64) {
                        println!(
                            "current fingerprint for {}: {}",
                            canonical,
                            ClientState::format_fingerprint_for_display(&observed)
                        );
                    }
                }
            }
        }

        if state_lock.trust_store.audit_log.len() != audit_len_before {
            if let Err(err) = state_lock.save_trust_store() {
                eprintln!("failed to persist trust store: {}", err);
            }
        }
        return;
    }

    if let Err(err) = state_lock.send_message(&scope, body) {
        eprintln!("failed to send message: {}", err);
    } else {
        state_lock.dismiss_unread_marker(&scope);
        println!("[sending] {} {}: {}", scope, username, body);
    }
}

async fn handle_user_input(state: &SharedState, input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return true;
    }

    if !trimmed.starts_with('/') {
        send_input_to_active_scope(state, trimmed).await;
        return true;
    }

    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next().unwrap_or("");

    match cmd {
        "/help" => {
            let maybe_command = parts.next();
            if parts.next().is_some() {
                println!("Usage: /help [command]");
                return true;
            }

            if let Some(command_name) = maybe_command {
                if let Some(entry) = find_command_help(command_name) {
                    print_command_help(entry);
                } else {
                    println!(
                        "Unknown command '{}'. Run /commands {} to search.",
                        command_name,
                        command_name.trim_start_matches('/')
                    );
                }
            } else {
                print_help();
            }
        }
        "/commands" => {
            let filter = parts.next();
            if parts.next().is_some() {
                println!("Usage: /commands [filter]");
                return true;
            }
            print_commands(filter);
        }
        "/quit" | "/exit" | "/q" => {
            return false;
        }
        "/join" | "/switch" => {
            let Some(channel_raw) = parts.next() else {
                println!("Usage: /join <channel>");
                return true;
            };
            let channel =
                chatify::normalize_channel(channel_raw).unwrap_or_else(|| "general".to_string());

            let mut state_lock = state.lock().await;
            state_lock.chs.insert(channel.clone(), true);
            state_lock.switch_scope(channel.clone());
            if let Err(err) = state_lock.send_join(&channel) {
                eprintln!("failed to join channel: {}", err);
            }
        }
        "/leave" | "/part" => {
            let maybe_channel_raw = parts.next();
            if parts.next().is_some() {
                println!("Usage: /leave [channel]");
                return true;
            }

            let channel = if let Some(channel_raw) = maybe_channel_raw {
                chatify::normalize_channel(channel_raw).unwrap_or_else(|| "general".to_string())
            } else {
                let current = state.lock().await.ch.clone();
                if current.starts_with("dm:") {
                    println!("Usage: /leave <channel>");
                    return true;
                }
                current
            };

            let mut state_lock = state.lock().await;
            if let Err(err) = state_lock.send_leave(&channel) {
                eprintln!("failed to leave channel: {}", err);
            } else {
                state_lock.chs.remove(&channel);
                if state_lock.ch == channel {
                    state_lock.switch_scope("general".to_string());
                }
            }
        }
        "/history" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            let mut scope = current_scope.clone();
            let mut limit = DEFAULT_HISTORY_LIMIT;

            if let Some(first) = tokens.first().copied() {
                if let Ok(parsed_limit) = first.parse::<usize>() {
                    limit = parsed_limit.clamp(1, MAX_HISTORY_LIMIT);
                    let _ = tokens.remove(0);
                } else {
                    scope = normalize_scope_token(first, &current_scope, true);
                    let _ = tokens.remove(0);
                }
            }

            if let Some(next) = tokens.first().copied() {
                if let Ok(parsed_limit) = next.parse::<usize>() {
                    limit = parsed_limit.clamp(1, MAX_HISTORY_LIMIT);
                    let _ = tokens.remove(0);
                } else {
                    println!("Usage: /history [ch|dm:user] [limit]");
                    return true;
                }
            }

            if !tokens.is_empty() {
                println!("Usage: /history [ch|dm:user] [limit]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_history(&scope, limit) {
                eprintln!("failed to request history: {}", err);
            } else {
                println!(
                    "history request sent for {} (limit={})",
                    format_scope_for_help(&scope),
                    limit
                );
            }
        }
        "/search" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            if tokens.is_empty() {
                println!("Usage: /search [#ch|dm:user] <query> [limit=N]");
                return true;
            }

            let mut scope = current_scope.clone();
            if let Some(first) = tokens.first().copied() {
                if is_explicit_scope_token(first) {
                    scope = normalize_scope_token(first, &current_scope, false);
                    let _ = tokens.remove(0);
                }
            }

            let mut limit = DEFAULT_SEARCH_LIMIT;
            if let Some(last) = tokens.last().copied() {
                if let Some(parsed) = parse_limit_token(last) {
                    limit = parsed.clamp(1, MAX_SEARCH_LIMIT);
                    let _ = tokens.pop();
                }
            }

            let query = tokens.join(" ").trim().to_string();
            if query.is_empty() {
                println!("Usage: /search [#ch|dm:user] <query> [limit=N]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_search(&scope, &query, limit) {
                eprintln!("failed to request search: {}", err);
            } else {
                println!(
                    "search request sent for {} (limit={}): {}",
                    format_scope_for_help(&scope),
                    limit,
                    query
                );
            }
        }
        "/replay" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            if tokens.is_empty() {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                return true;
            }

            let from_ts_raw = tokens.remove(0);
            let Some(from_ts) = parse_replay_timestamp(from_ts_raw) else {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                println!("from_ts must be a non-negative unix timestamp in seconds");
                return true;
            };

            let mut scope = current_scope.clone();
            if let Some(first) = tokens.first().copied() {
                if is_explicit_scope_token(first) {
                    scope = normalize_scope_token(first, &current_scope, false);
                    let _ = tokens.remove(0);
                }
            }

            let mut limit = DEFAULT_REPLAY_LIMIT;
            if let Some(last) = tokens.last().copied() {
                if let Some(parsed) = parse_limit_token(last) {
                    limit = parsed.clamp(1, MAX_REPLAY_LIMIT);
                    let _ = tokens.pop();
                }
            }

            if !tokens.is_empty() {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_replay(&scope, from_ts, limit) {
                eprintln!("failed to request replay: {}", err);
            } else {
                println!(
                    "replay request sent for {} from ts={} (limit={})",
                    format_scope_for_help(&scope),
                    from_ts,
                    limit
                );
            }
        }
        "/users" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                eprintln!("failed to request users: {}", err);
            }
        }
        "/metrics" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_metrics() {
                eprintln!("failed to request metrics: {}", err);
            }
        }
        "/db-profile" | "/dbprofile" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_db_profile() {
                eprintln!("failed to request db profile: {}", err);
            }
        }
        "/doctor" => {
            let tokens: Vec<&str> = parts.collect();
            let json_output = if tokens.is_empty() {
                false
            } else if tokens.len() == 1 && tokens[0].eq_ignore_ascii_case("--json") {
                true
            } else {
                print_doctor_usage();
                return true;
            };

            let diagnostics = run_connection_doctor(state).await;
            if json_output {
                match serde_json::to_string_pretty(&diagnostics) {
                    Ok(rendered) => println!("{}", rendered),
                    Err(err) => eprintln!("failed to serialize doctor diagnostics: {}", err),
                }
            } else {
                print_connection_doctor_report(&diagnostics);
            }
        }
        "/typing" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            let mut typing = true;

            if let Some(first) = tokens.first().copied() {
                match first.to_ascii_lowercase().as_str() {
                    "on" | "true" => {
                        typing = true;
                        let _ = tokens.remove(0);
                    }
                    "off" | "false" => {
                        typing = false;
                        let _ = tokens.remove(0);
                    }
                    _ => {}
                }
            }

            let scope = if let Some(first) = tokens.first().copied() {
                let parsed = normalize_scope_token(first, &current_scope, true);
                let _ = tokens.remove(0);
                parsed
            } else {
                current_scope.clone()
            };

            if !tokens.is_empty() {
                println!("Usage: /typing [on|off] [#ch|dm:user]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_typing(&scope, typing) {
                eprintln!("failed to send typing state: {}", err);
            } else {
                let status = if typing { "on" } else { "off" };
                println!("typing={} for {}", status, format_scope_for_help(&scope));
            }
        }
        "/voice" | "/vc" => {
            let subcommand = parts.next().unwrap_or("status").to_ascii_lowercase();

            match subcommand.as_str() {
                "on" | "start" | "join" => {
                    let room_arg = parts.next();
                    if parts.next().is_some() {
                        println!("Usage: /voice on [room]");
                        return true;
                    }

                    let requested_room = room_arg
                        .and_then(chatify::normalize_channel)
                        .unwrap_or_default();

                    let (media_enabled, already_active, ws_tx, default_room) = {
                        let state_lock = state.lock().await;
                        let default_room = if state_lock.ch.starts_with("dm:") {
                            "general".to_string()
                        } else {
                            chatify::normalize_channel(&state_lock.ch)
                                .unwrap_or_else(|| "general".to_string())
                        };

                        (
                            state_lock.media_enabled,
                            state_lock.voice_active,
                            state_lock.ws_tx.clone(),
                            default_room,
                        )
                    };

                    if !media_enabled {
                        println!(
                            "Voice is disabled by client media settings (--no-media or config)."
                        );
                        return true;
                    }

                    if already_active {
                        println!("Voice is already active.");
                        return true;
                    }

                    let room = if requested_room.is_empty() {
                        default_room
                    } else {
                        requested_room
                    };

                    let started = match start_voice_session(room.clone(), ws_tx) {
                        Ok(session) => session,
                        Err(err) => {
                            eprintln!("failed to start local voice session: {}", err);
                            return true;
                        }
                    };
                    let event_tx = started.event_tx;

                    let mut state_lock = state.lock().await;
                    if state_lock.voice_active {
                        let _ = event_tx.send(VoiceEvent::Stop);
                        println!("Voice became active concurrently; keeping existing session.");
                        return true;
                    }

                    if let Err(err) = state_lock.send_voice_join(&room) {
                        let _ = event_tx.send(VoiceEvent::Stop);
                        eprintln!("failed to join voice room: {}", err);
                        return true;
                    }

                    state_lock.voice_session = Some(chatify_client::state::VoiceSession {
                        room: room.clone(),
                        event_tx,
                    });
                    state_lock.voice_active = true;
                    state_lock.voice_muted = false;
                    state_lock.voice_deafened = false;
                    state_lock.voice_speaking = false;

                    println!("Voice started in room #{}.", room);
                }
                "off" | "stop" | "leave" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice off");
                        return true;
                    }

                    let mut state_lock = state.lock().await;
                    let Some(session) = state_lock.voice_session.take() else {
                        println!("Voice is not active.");
                        return true;
                    };

                    let room = session.room.clone();
                    let _ = session.event_tx.send(VoiceEvent::Stop);

                    if let Err(err) = state_lock.send_voice_leave(&room) {
                        eprintln!("failed to leave voice room: {}", err);
                    }

                    state_lock.voice_active = false;
                    state_lock.voice_muted = false;
                    state_lock.voice_deafened = false;
                    state_lock.voice_speaking = false;
                    state_lock.voice_members.clear();

                    println!("Voice stopped for room #{}.", room);
                }
                "mute" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice mute");
                        return true;
                    }

                    let mut state_lock = state.lock().await;
                    if !state_lock.voice_active {
                        println!("Voice is not active.");
                        return true;
                    }

                    if let Err(err) = state_lock.send_voice_state(Some(true), None) {
                        eprintln!("failed to send mute state: {}", err);
                    }
                    state_lock.voice_muted = true;
                    println!("Voice muted.");
                }
                "unmute" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice unmute");
                        return true;
                    }

                    let mut state_lock = state.lock().await;
                    if !state_lock.voice_active {
                        println!("Voice is not active.");
                        return true;
                    }

                    if let Err(err) = state_lock.send_voice_state(Some(false), None) {
                        eprintln!("failed to send mute state: {}", err);
                    }
                    state_lock.voice_muted = false;
                    println!("Voice unmuted.");
                }
                "deafen" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice deafen");
                        return true;
                    }

                    let mut state_lock = state.lock().await;
                    if !state_lock.voice_active {
                        println!("Voice is not active.");
                        return true;
                    }

                    if let Err(err) = state_lock.send_voice_state(None, Some(true)) {
                        eprintln!("failed to send deafen state: {}", err);
                    }
                    state_lock.voice_deafened = true;
                    println!("Voice deafened.");
                }
                "undeafen" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice undeafen");
                        return true;
                    }

                    let mut state_lock = state.lock().await;
                    if !state_lock.voice_active {
                        println!("Voice is not active.");
                        return true;
                    }

                    if let Err(err) = state_lock.send_voice_state(None, Some(false)) {
                        eprintln!("failed to send deafen state: {}", err);
                    }
                    state_lock.voice_deafened = false;
                    println!("Voice undeafened.");
                }
                "status" => {
                    if parts.next().is_some() {
                        println!("Usage: /voice status");
                        return true;
                    }

                    let state_lock = state.lock().await;
                    if !state_lock.voice_active {
                        println!("Voice status: OFF");
                    } else if let Some(session) = &state_lock.voice_session {
                        println!(
                            "Voice status: ON room=#{} muted={} deafened={} speaking={} members={}",
                            session.room,
                            state_lock.voice_muted,
                            state_lock.voice_deafened,
                            state_lock.voice_speaking,
                            state_lock.voice_members.len()
                        );
                    } else {
                        println!("Voice status: ON (session metadata unavailable)");
                    }
                }
                _ => {
                    println!("Usage: /voice <on|off|mute|unmute|deafen|undeafen|status> [room]");
                }
            }
        }
        "/screen" | "/ss" => {
            let subcommand = parts.next().unwrap_or("status").to_ascii_lowercase();

            match subcommand.as_str() {
                "on" | "start" => {
                    let room_arg = parts.next();
                    if parts.next().is_some() {
                        println!("Usage: /screen start [room]");
                        return true;
                    }

                    let room = if let Some(raw_room) = room_arg {
                        chatify::normalize_channel(raw_room)
                            .unwrap_or_else(|| "general".to_string())
                    } else {
                        let current = state.lock().await.ch.clone();
                        if current.starts_with("dm:") {
                            "general".to_string()
                        } else {
                            chatify::normalize_channel(&current)
                                .unwrap_or_else(|| "general".to_string())
                        }
                    };

                    let state_lock = state.lock().await;
                    if let Err(err) = state_lock.send_screen_start(&room) {
                        eprintln!("failed to send screen-share start request: {}", err);
                    } else {
                        println!("Requested screen-share start for room #{}.", room);
                    }
                }
                "off" | "stop" => {
                    let room_arg = parts.next();
                    if parts.next().is_some() {
                        println!("Usage: /screen stop [room]");
                        return true;
                    }

                    let room = if let Some(raw_room) = room_arg {
                        chatify::normalize_channel(raw_room)
                            .unwrap_or_else(|| "general".to_string())
                    } else {
                        let current = state.lock().await.ch.clone();
                        if current.starts_with("dm:") {
                            "general".to_string()
                        } else {
                            chatify::normalize_channel(&current)
                                .unwrap_or_else(|| "general".to_string())
                        }
                    };

                    let state_lock = state.lock().await;
                    if let Err(err) = state_lock.send_screen_stop(&room) {
                        eprintln!("failed to send screen-share stop request: {}", err);
                    } else {
                        println!("Requested screen-share stop for room #{}.", room);
                    }
                }
                "status" => {
                    if parts.next().is_some() {
                        println!("Usage: /screen status");
                        return true;
                    }

                    let state_lock = state.lock().await;
                    let active = state_lock.screen_share.is_some();
                    println!(
                        "Screen-share status: {} viewing={} frames={} last_from={} last_seq={}",
                        if active { "ON" } else { "OFF" },
                        state_lock.screen_viewing,
                        state_lock.screen_frames_received,
                        state_lock
                            .screen_last_frame_from
                            .as_deref()
                            .unwrap_or("n/a"),
                        state_lock
                            .screen_last_frame_seq
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "n/a".to_string())
                    );
                }
                _ => {
                    println!("Usage: /screen <start|stop|status> [room]");
                }
            }
        }
        "/notify" => {
            let tokens: Vec<&str> = parts.collect();

            if tokens.is_empty() {
                let state_lock = state.lock().await;
                print_notification_settings(&state_lock.config.notifications);
                return true;
            }

            let target = tokens[0];

            if target.eq_ignore_ascii_case("reset") {
                if tokens.len() != 1 {
                    print_notify_usage();
                    return true;
                }

                let (old_config, new_config) = {
                    let mut state_lock = state.lock().await;
                    let old_config = state_lock.config.clone();
                    state_lock.config.notifications =
                        chatify::config::NotificationConfig::default();
                    let new_config = state_lock.config.clone();
                    (old_config, new_config)
                };

                if let Err(err) = new_config.save() {
                    eprintln!("failed to persist notification config: {}", err);
                    let mut state_lock = state.lock().await;
                    state_lock.config = old_config;
                    return true;
                }

                println!("Notification settings reset to defaults.");
                print_notification_settings(&new_config.notifications);
                return true;
            }

            if target.eq_ignore_ascii_case("export") {
                let mut redact = false;
                let mut destination: Option<&str> = None;
                for token in tokens.iter().skip(1) {
                    if token.eq_ignore_ascii_case("--redact") {
                        if redact {
                            print_notify_usage();
                            return true;
                        }
                        redact = true;
                        continue;
                    }

                    if destination.is_none() {
                        destination = Some(*token);
                    } else {
                        print_notify_usage();
                        return true;
                    }
                }

                let maybe_path = destination;

                let (payload, me, host, port, tls) = {
                    let state_lock = state.lock().await;
                    (
                        build_notification_export(
                            &state_lock.config.notifications,
                            &state_lock.me,
                            &state_lock.client_config.host,
                            state_lock.client_config.port,
                            state_lock.client_config.tls,
                        ),
                        state_lock.me.clone(),
                        state_lock.client_config.host.clone(),
                        state_lock.client_config.port,
                        state_lock.client_config.tls,
                    )
                };

                let export_payload = if redact {
                    redact_notification_export(&payload)
                } else {
                    payload
                };

                let serialized = match serde_json::to_string_pretty(&export_payload) {
                    Ok(v) => v,
                    Err(err) => {
                        eprintln!("failed to serialize notification export: {}", err);
                        return true;
                    }
                };

                if maybe_path
                    .map(|v| v.eq_ignore_ascii_case("stdout") || v == "-")
                    .unwrap_or(false)
                {
                    println!("{}", serialized);
                    return true;
                }

                let target_path = maybe_path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| notify_export_default_path(&me, &host, port, tls));
                if let Some(parent) = target_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(err) = fs::create_dir_all(parent) {
                            eprintln!("failed to create export directory: {}", err);
                            return true;
                        }
                    }
                }

                if let Err(err) = fs::write(&target_path, serialized) {
                    eprintln!("failed to write notification export: {}", err);
                    return true;
                }

                println!(
                    "Exported notification settings to {}",
                    target_path.display()
                );
                if redact {
                    println!("Export mode: redacted profile fields.");
                }

                return true;
            }

            if target.eq_ignore_ascii_case("doctor") {
                let json_output = if tokens.len() == 1 {
                    false
                } else if tokens.len() == 2 && tokens[1].eq_ignore_ascii_case("--json") {
                    true
                } else {
                    print_notify_usage();
                    return true;
                };

                let (notifications, me, host, port, tls) = {
                    let state_lock = state.lock().await;
                    (
                        state_lock.config.notifications.clone(),
                        state_lock.me.clone(),
                        state_lock.client_config.host.clone(),
                        state_lock.client_config.port,
                        state_lock.client_config.tls,
                    )
                };

                let diagnostics =
                    build_notify_diagnostics_json(&notifications, &me, &host, port, tls);

                if json_output {
                    match serde_json::to_string_pretty(&diagnostics) {
                        Ok(rendered) => println!("{}", rendered),
                        Err(err) => {
                            eprintln!("failed to serialize notify diagnostics: {}", err);
                        }
                    }
                    return true;
                }

                println!("Notify doctor:");
                println!(
                    "  profile: user={} server={}:{} tls={}",
                    diagnostics
                        .get("profile")
                        .and_then(|v| v.get("user"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("anonymous"),
                    diagnostics
                        .get("profile")
                        .and_then(|v| v.get("host"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("127.0.0.1"),
                    port,
                    tls
                );
                print_notification_settings(&notifications);
                println!(
                    "  config_path: {}",
                    diagnostics
                        .get("config_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unavailable)")
                );
                println!(
                    "  config_dir_state: {}",
                    diagnostics
                        .get("config_dir_state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)")
                );
                println!(
                    "  default_export_path: {}",
                    diagnostics
                        .get("default_export_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)")
                );

                if let Some(recommendations) = diagnostics
                    .get("recommendations")
                    .and_then(|v| v.as_array())
                {
                    for recommendation in recommendations {
                        if let Some(text) = recommendation.as_str() {
                            println!("  recommendation: {}", text);
                        }
                    }
                }

                return true;
            }

            if target.eq_ignore_ascii_case("test") {
                let (sound_probe, level, message_start) = parse_notify_test_args(&tokens);
                let (notifications, me) = {
                    let state_lock = state.lock().await;
                    (
                        state_lock.config.notifications.clone(),
                        state_lock.me.clone(),
                    )
                };

                let body = if tokens.len() > message_start {
                    tokens[message_start..].join(" ").trim().to_string()
                } else if me.is_empty() {
                    format!(
                        "This is a {} test notification from Chatify.",
                        level.to_ascii_lowercase()
                    )
                } else {
                    format!(
                        "This is a {} test notification for {}.",
                        level.to_ascii_lowercase(),
                        me
                    )
                };

                if body.is_empty() {
                    print_notify_usage();
                    return true;
                }

                if !notifications.enabled {
                    println!("notifications.enabled is off; sending one-time test anyway.");
                }

                send_notification_test(&notifications, level, &body, sound_probe);
                println!(
                    "Sent desktop notification probe (level={}, sound_probe={}).",
                    level,
                    if sound_probe { "on" } else { "off" }
                );
                return true;
            }

            let Some(config_key) = notification_key_for_token(target) else {
                println!(
                    "Unknown notify setting '{}'. Use: enabled, dm, mention, all, sound.",
                    target
                );
                return true;
            };

            if tokens.len() == 1 {
                let state_lock = state.lock().await;
                let value =
                    notification_value_for_key(&state_lock.config.notifications, config_key)
                        .unwrap_or(false);
                println!("{} = {}", config_key, if value { "on" } else { "off" });
                return true;
            }

            if tokens.len() != 2 {
                print_notify_usage();
                return true;
            }

            let value_raw = tokens[1];

            let Some(enabled) = parse_toggle_bool(value_raw) else {
                println!("Invalid notify value '{}'. Use on/off.", value_raw);
                return true;
            };

            let (old_config, new_config) = {
                let mut state_lock = state.lock().await;
                let old_config = state_lock.config.clone();
                if let Err(err) = state_lock
                    .config
                    .set_value(config_key, if enabled { "true" } else { "false" })
                {
                    println!("{}", err);
                    return true;
                }
                let new_config = state_lock.config.clone();
                (old_config, new_config)
            };

            if let Err(err) = new_config.save() {
                eprintln!("failed to persist notification config: {}", err);
                let mut state_lock = state.lock().await;
                state_lock.config = old_config;
                return true;
            }

            println!(
                "Updated {} = {}.",
                config_key,
                if enabled { "on" } else { "off" }
            );
            print_notification_settings(&new_config.notifications);
        }
        "/plugin" => match parse_plugin_command(trimmed) {
            Ok(PluginCommand::List) => {
                let state_lock = state.lock().await;
                if let Err(err) = state_lock.send_plugin_list() {
                    eprintln!("failed to request plugin inventory: {}", err);
                } else {
                    println!("plugin inventory request sent");
                }
            }
            Ok(PluginCommand::Install(spec)) => {
                let state_lock = state.lock().await;
                if let Err(err) = state_lock.send_plugin_install(&spec) {
                    eprintln!("failed to install plugin: {}", err);
                } else {
                    println!("plugin install request sent for {}", spec);
                }
            }
            Ok(PluginCommand::Disable(plugin_id)) => {
                let state_lock = state.lock().await;
                if let Err(err) = state_lock.send_plugin_disable(&plugin_id) {
                    eprintln!("failed to disable plugin: {}", err);
                } else {
                    println!("plugin disable request sent for {}", plugin_id);
                }
            }
            Err(usage) => {
                println!("{}", usage);
            }
        },
        #[cfg(feature = "bridge-client")]
        "/bridge" => match parse_bridge_command(trimmed) {
            Ok(BridgeCommand::Status) => {
                let state_lock = state.lock().await;
                if let Err(err) = state_lock.send_bridge_status() {
                    eprintln!("failed to request bridge status: {}", err);
                } else {
                    println!("bridge status request sent");
                }
            }
            Err(usage) => {
                println!("{}", usage);
            }
        },
        "/dm" => {
            let Some(raw_user) = parts.next() else {
                println!("Usage: /dm <user> <message>");
                return true;
            };

            let body = parts.collect::<Vec<&str>>().join(" ").trim().to_string();
            if body.is_empty() {
                println!("Usage: /dm <user> <message>");
                return true;
            }

            let mut state_lock = state.lock().await;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!("No key directory loaded yet; requested /users from server. Retry /dm after refresh.");
                return true;
            }

            let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                Ok(user) => user,
                Err(err) => {
                    println!("{}", err);
                    return true;
                }
            };

            let audit_len_before = state_lock.trust_store.audit_log.len();
            match state_lock.ensure_peer_trusted_for_dm(&canonical) {
                Ok(fingerprint) => {
                    if let Err(err) = state_lock.send_dm(&canonical, &body) {
                        eprintln!("failed to send dm: {}", err);
                    } else {
                        println!(
                            "dm sent to {} (trusted fingerprint {})",
                            canonical,
                            ClientState::format_fingerprint_for_display(&fingerprint)
                        );
                    }
                }
                Err(err) => {
                    println!("dm blocked by trust policy: {}", err);
                    if let Some(pubkey_b64) = state_lock.users.get(&canonical) {
                        if let Some(observed) = ClientState::fingerprint_for_pubkey(pubkey_b64) {
                            println!(
                                "current fingerprint for {}: {}",
                                canonical,
                                ClientState::format_fingerprint_for_display(&observed)
                            );
                        }
                    }
                }
            }

            if state_lock.trust_store.audit_log.len() != audit_len_before {
                if let Err(err) = state_lock.save_trust_store() {
                    eprintln!("failed to persist trust store: {}", err);
                }
            }
        }
        "/fingerprint" => {
            let user_arg = parts.next();
            if parts.next().is_some() {
                println!("Usage: /fingerprint [user]");
                return true;
            }

            let mut state_lock = state.lock().await;
            let mut trust_store_dirty = false;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!(
                    "No key directory loaded yet; requested /users from server. Run /fingerprint again."
                );
                return true;
            }

            if let Some(raw_user) = user_arg {
                let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                    Ok(user) => user,
                    Err(err) => {
                        println!("{}", err);
                        return true;
                    }
                };

                let Some(pubkey_b64) = state_lock.users.get(&canonical) else {
                    println!(
                        "unknown user '{}'; run /users to refresh directory",
                        canonical
                    );
                    return true;
                };

                let Some(fingerprint) = ClientState::fingerprint_for_pubkey(pubkey_b64) else {
                    println!("user '{}' has an invalid public key", canonical);
                    return true;
                };

                println!(
                    "{} fingerprint: {}",
                    canonical,
                    ClientState::format_fingerprint_for_display(&fingerprint)
                );

                let status = trust_status_label(&state_lock, &canonical, &fingerprint);
                if let Some(peer) = state_lock.trust_store.peers.get(&canonical) {
                    println!("trust status: {} (trusted_at={})", status, peer.trusted_at);
                } else {
                    println!("trust status: {}", status);
                }

                state_lock.append_trust_audit(
                    "fingerprint_view",
                    &canonical,
                    &format!("fingerprint={}", fingerprint),
                );
                trust_store_dirty = true;
            } else {
                let mut users: Vec<String> = state_lock.users.keys().cloned().collect();
                users.sort_by_key(|a| a.to_ascii_lowercase());

                println!("Known user fingerprints:");
                for user in users {
                    let Some(pubkey_b64) = state_lock.users.get(&user) else {
                        continue;
                    };

                    let Some(fingerprint) = ClientState::fingerprint_for_pubkey(pubkey_b64) else {
                        println!("  {} [invalid-key]", user);
                        continue;
                    };

                    println!(
                        "  {} [{}] {}",
                        user,
                        trust_status_label(&state_lock, &user, &fingerprint),
                        ClientState::format_fingerprint_for_display(&fingerprint)
                    );
                }
            }

            if trust_store_dirty {
                if let Err(err) = state_lock.save_trust_store() {
                    eprintln!("failed to persist trust store: {}", err);
                }
            }
        }
        "/trust" => {
            let Some(raw_user) = parts.next() else {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            };
            let Some(fingerprint) = parts.next() else {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            };
            if parts.next().is_some() {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            }

            let mut state_lock = state.lock().await;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!(
                    "No key directory loaded yet; requested /users from server. Retry /trust after refresh."
                );
                return true;
            }

            let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                Ok(user) => user,
                Err(err) => {
                    println!("{}", err);
                    return true;
                }
            };

            match state_lock.trust_peer(&canonical, fingerprint) {
                Ok(observed) => {
                    println!(
                        "trusted {} with fingerprint {}",
                        canonical,
                        ClientState::format_fingerprint_for_display(&observed)
                    );
                }
                Err(err) => {
                    println!("trust failed: {}", err);
                    if let Some(pubkey_b64) = state_lock.users.get(&canonical) {
                        if let Some(observed) = ClientState::fingerprint_for_pubkey(pubkey_b64) {
                            println!(
                                "current fingerprint for {}: {}",
                                canonical,
                                ClientState::format_fingerprint_for_display(&observed)
                            );
                        }
                    }
                }
            }

            if let Err(err) = state_lock.save_trust_store() {
                eprintln!("failed to persist trust store: {}", err);
            }
        }
        "/trust-audit" => {
            let limit = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(DEFAULT_TRUST_AUDIT_LIMIT)
                .clamp(1, MAX_TRUST_AUDIT_LIMIT);
            if parts.next().is_some() {
                println!("Usage: /trust-audit [limit]");
                return true;
            }

            let state_lock = state.lock().await;
            if state_lock.trust_store.audit_log.is_empty() {
                println!("No trust audit entries yet.");
                return true;
            }

            println!(
                "Trust audit (latest {} entries):",
                limit.min(state_lock.trust_store.audit_log.len())
            );
            for entry in state_lock.trust_store.audit_log.iter().rev().take(limit) {
                println!(
                    "  ts={} action={} peer={} details={}",
                    entry.timestamp, entry.action, entry.peer, entry.details
                );
            }
        }
        "/trust-export" => {
            let maybe_path = parts.next();
            if parts.next().is_some() {
                println!("Usage: /trust-export [path]");
                return true;
            }

            let state_lock = state.lock().await;
            let target_path = maybe_path
                .map(PathBuf::from)
                .unwrap_or_else(|| state_lock.trust_audit_export_path());

            match state_lock.export_trust_audit_to_path(&target_path) {
                Ok(entries) => {
                    println!(
                        "exported {} trust audit entries to {}",
                        entries,
                        target_path.display()
                    );
                }
                Err(err) => {
                    eprintln!("failed to export trust audit: {}", err);
                }
            }
        }
        "/recent" => {
            let limit = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(DEFAULT_RECENT_LIMIT)
                .clamp(1, MAX_RECENT_LIMIT);
            print_recent_messages(state, limit).await;
        }
        "/sync" => {
            let state_lock = state.lock().await;
            let channel = state_lock.ch.clone();
            if let Err(err) = state_lock.send_reaction_sync(&channel, DEFAULT_REACTION_SYNC_LIMIT) {
                eprintln!("failed to sync reactions: {}", err);
            }
        }
        "/image" | "/video" | "/audio" => {
            let Some(path_str) = trimmed
                .strip_prefix(cmd)
                .and_then(parse_shell_like_argument)
            else {
                println!("Usage: {} \"<path>\"", cmd);
                return true;
            };
            let Some(media_kind) = media_kind_for_command(cmd) else {
                println!("Usage: {} \"<path>\"", cmd);
                return true;
            };

            let path = Path::new(&path_str);
            if !path.exists() {
                eprintln!("file not found: {}", path_str);
                return true;
            }
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("cannot read file {}: {}", path_str, e);
                    return true;
                }
            };
            let file_size = metadata.len();
            const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
            if file_size > MAX_FILE_SIZE {
                eprintln!(
                    "file too large: {} bytes (max {})",
                    file_size, MAX_FILE_SIZE
                );
                return true;
            }
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let mime = guess_mime_from_path(path, media_kind);
            let duration_ms = if media_kind == MediaKind::Audio {
                detect_audio_duration_ms(path)
            } else {
                None
            };
            let data = match fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("failed to read file: {}", e);
                    return true;
                }
            };

            let (channel, media_enabled) = {
                let state_lock = state.lock().await;
                (state_lock.ch.clone(), state_lock.media_enabled)
            };
            if !media_enabled {
                println!("Media is disabled by client media settings (--no-media or config).");
                return true;
            }
            if channel.starts_with("dm:") {
                println!("Media attachments are only supported in channels right now.");
                return true;
            }

            let file_id = chatify::fresh_nonce_hex();
            {
                let state_lock = state.lock().await;
                if let Err(e) = state_lock.send_file_meta(OutgoingMediaMeta {
                    channel: &channel,
                    file_id: &file_id,
                    filename,
                    media_kind: media_kind.wire_name(),
                    size: file_size,
                    mime: mime.as_deref(),
                    duration_ms,
                }) {
                    eprintln!("failed to send file metadata: {}", e);
                    return true;
                }
            }

            let mut chunk_send_failed = false;
            const CHUNK_SIZE: usize = 16 * 1024;
            for (index, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
                let state_lock = state.lock().await;
                if let Err(e) = state_lock.send_file_chunk(&channel, &file_id, index as u64, chunk)
                {
                    eprintln!("failed to send file chunk: {}", e);
                    chunk_send_failed = true;
                    break;
                }
                drop(state_lock);
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }

            if chunk_send_failed {
                return true;
            }

            println!("[sent] {} {} to #{}", media_kind.label(), filename, channel);
        }
        "/react" => {
            let Some(msg_ref) = parts.next() else {
                println!("Usage: /react <msg_id|#index> <emoji>");
                return true;
            };
            let Some(emoji) = parts.next() else {
                println!("Usage: /react <msg_id|#index> <emoji>");
                return true;
            };
            let emoji = emoji.trim();
            if !is_valid_reaction_emoji(emoji) {
                println!("Invalid emoji token. Use a non-empty value up to 32 bytes.");
                return true;
            }

            let state_lock = state.lock().await;
            let channel = state_lock.ch.clone();
            let resolved_msg_id = if let Some(index_str) = msg_ref.strip_prefix('#') {
                let Some(index) = index_str.parse::<usize>().ok() else {
                    println!(
                        "Invalid message index '{}'. Use e.g. #1 for most recent.",
                        index_str
                    );
                    return true;
                };
                let Some(msg_id) = state_lock.resolve_recent_message_id_in_channel(&channel, index)
                else {
                    println!(
                        "Could not resolve message index #{} in channel #{}.",
                        index, channel
                    );
                    return true;
                };
                msg_id
            } else {
                msg_ref.to_string()
            };

            if let Err(err) = state_lock.send_reaction(&channel, &resolved_msg_id, emoji) {
                eprintln!("failed to send reaction: {}", err);
            } else {
                println!(
                    "reaction sent: {} -> #{}",
                    emoji,
                    resolved_msg_id.chars().take(8).collect::<String>()
                );
            }
        }
        _ => {
            println!("Unknown command. Type /help or /commands.");
        }
    }

    true
}

fn reconnect_backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(5);
    let seconds = (RECONNECT_BASE_DELAY_SECS << shift).min(RECONNECT_MAX_DELAY_SECS);
    Duration::from_secs(seconds)
}

fn enqueue_pending_frame(queue: &mut VecDeque<String>, frame: String) {
    if queue.len() >= MAX_OFFLINE_OUTBOUND_QUEUE {
        let _ = queue.pop_front();
        eprintln!(
            "[offline-queue] pending queue reached {} frames; dropped oldest message.",
            MAX_OFFLINE_OUTBOUND_QUEUE
        );
    }
    queue.push_back(frame);
}

fn enqueue_pending_frame_front(queue: &mut VecDeque<String>, frame: String) {
    if queue.len() >= MAX_OFFLINE_OUTBOUND_QUEUE {
        let _ = queue.pop_back();
        eprintln!(
            "[offline-queue] pending queue reached {} frames; dropped newest message.",
            MAX_OFFLINE_OUTBOUND_QUEUE
        );
    }
    queue.push_front(frame);
}

fn friendly_ws_error(error: &tungstenite::Error, tls: bool) -> String {
    if let tungstenite::Error::Io(io_error) = error {
        return match io_error.kind() {
            io::ErrorKind::ConnectionRefused => {
                "Server refused the connection. Verify host/port and that chatify-server is running."
                    .to_string()
            }
            io::ErrorKind::TimedOut => {
                "Connection timed out. Check network reachability and firewall rules.".to_string()
            }
            io::ErrorKind::NotFound | io::ErrorKind::AddrNotAvailable => {
                "Destination host or port is unavailable. Validate the connection profile."
                    .to_string()
            }
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof => {
                "Connection dropped by peer. The client will attempt to reconnect.".to_string()
            }
            io::ErrorKind::PermissionDenied => {
                "Connection blocked by local OS policy or firewall.".to_string()
            }
            _ => format!("Network I/O error: {}", io_error),
        };
    }

    let raw = error.to_string();
    let lower = raw.to_ascii_lowercase();

    if lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake") {
        return if tls {
            "TLS handshake failed. Verify cert/key configuration and trust chain.".to_string()
        } else {
            "Handshake failed. Server may require TLS; try enabling --tls.".to_string()
        };
    }

    if lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("401")
        || lower.contains("403")
    {
        return "Authentication rejected by server. Check username/password and access policy."
            .to_string();
    }

    format!("WebSocket error: {}", raw)
}

fn is_auth_rejection_frame(data: &serde_json::Value) -> bool {
    if data.get("t").and_then(|v| v.as_str()) != Some("err") {
        return false;
    }

    let message = data
        .get("m")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    [
        "auth",
        "credential",
        "password",
        "unauthorized",
        "forbidden",
        "invalid token",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn channels_to_rejoin(state: &SharedState) -> Vec<String> {
    let state_lock = state.lock().await;
    let mut channels = BTreeSet::new();
    channels.insert("general".to_string());

    if !state_lock.ch.starts_with("dm:") {
        let normalized =
            chatify::normalize_channel(&state_lock.ch).unwrap_or_else(|| "general".to_string());
        channels.insert(normalized);
    }

    for (channel, joined) in &state_lock.chs {
        if !joined {
            continue;
        }

        if let Some(normalized) = chatify::normalize_channel(channel) {
            channels.insert(normalized);
        }
    }

    channels.into_iter().collect()
}

async fn wait_backoff_with_buffer(
    outbound_rx: &mut mpsc::UnboundedReceiver<String>,
    pending_frames: &mut VecDeque<String>,
    delay: Duration,
) -> bool {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            _ = &mut sleep => return true,
            maybe_frame = outbound_rx.recv() => {
                match maybe_frame {
                    Some(frame) => enqueue_pending_frame(pending_frames, frame),
                    None => return false,
                }
            }
        }
    }
}

async fn run_connection_supervisor(
    state: SharedState,
    mut outbound_rx: mpsc::UnboundedReceiver<String>,
    uri: String,
    auth: ConnectionAuth,
    reconnect_enabled: bool,
    tls: bool,
) {
    let mut reconnect_attempt = 0u32;
    let mut pending_frames = VecDeque::new();

    loop {
        let connect_result = connect_async(&uri).await;
        let (ws_stream, _) = match connect_result {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("[connect] {}", friendly_ws_error(&error, tls));

                if !reconnect_enabled {
                    eprintln!(
                        "[connect] auto-reconnect is off; exiting after failed connection attempt."
                    );
                    break;
                }

                reconnect_attempt = reconnect_attempt.saturating_add(1);
                let delay = reconnect_backoff_delay(reconnect_attempt);
                eprintln!(
                    "[reconnect] Attempt #{} in {}s. Outbound messages are queued.",
                    reconnect_attempt,
                    delay.as_secs()
                );

                if !wait_backoff_with_buffer(&mut outbound_rx, &mut pending_frames, delay).await {
                    break;
                }
                continue;
            }
        };

        reconnect_attempt = 0;
        println!("[connection] Connected to {}", uri);

        let (mut write, mut read) = ws_stream.split();

        let auth_frame = serde_json::json!({
            "t": "auth",
            "u": auth.username,
            "pw": auth.password_hash,
            "pk": auth.public_key,
            "status": {"text": "Online", "emoji": ""}
        })
        .to_string();

        if let Err(error) = write.send(Message::Text(auth_frame)).await {
            eprintln!(
                "[connect] failed to send auth frame: {}",
                friendly_ws_error(&error, tls)
            );

            if !reconnect_enabled {
                eprintln!("[connect] auto-reconnect is off; exiting after failed auth frame send.");
                break;
            }

            reconnect_attempt = reconnect_attempt.saturating_add(1);
            let delay = reconnect_backoff_delay(reconnect_attempt);
            eprintln!(
                "[reconnect] Attempt #{} in {}s. Outbound messages are queued.",
                reconnect_attempt,
                delay.as_secs()
            );

            if !wait_backoff_with_buffer(&mut outbound_rx, &mut pending_frames, delay).await {
                break;
            }
            continue;
        }

        let channels = channels_to_rejoin(&state).await;
        let mut reconnect_reason = String::new();
        for channel in channels {
            let join_frame = serde_json::json!({"t": "join", "ch": channel}).to_string();
            if let Err(error) = write.send(Message::Text(join_frame)).await {
                reconnect_reason = format!(
                    "failed to restore joined channels: {}",
                    friendly_ws_error(&error, tls)
                );
                break;
            }
        }

        if !reconnect_reason.is_empty() {
            eprintln!("[connection] {}", reconnect_reason);
            if !reconnect_enabled {
                eprintln!("[connection] auto-reconnect is off; exiting after restore failure.");
                break;
            }

            reconnect_attempt = reconnect_attempt.saturating_add(1);
            let delay = reconnect_backoff_delay(reconnect_attempt);
            eprintln!(
                "[reconnect] Attempt #{} in {}s. Outbound messages are queued.",
                reconnect_attempt,
                delay.as_secs()
            );

            if !wait_backoff_with_buffer(&mut outbound_rx, &mut pending_frames, delay).await {
                break;
            }
            continue;
        }

        while let Some(frame) = pending_frames.pop_front() {
            if let Err(error) = write.send(Message::Text(frame.clone())).await {
                enqueue_pending_frame_front(&mut pending_frames, frame);
                reconnect_reason = format!(
                    "connection dropped while flushing queued messages: {}",
                    friendly_ws_error(&error, tls)
                );
                break;
            }
        }

        if !reconnect_reason.is_empty() {
            eprintln!("[connection] {}", reconnect_reason);
            if !reconnect_enabled {
                eprintln!("[connection] auto-reconnect is off; exiting after queue flush failure.");
                break;
            }

            reconnect_attempt = reconnect_attempt.saturating_add(1);
            let delay = reconnect_backoff_delay(reconnect_attempt);
            eprintln!(
                "[reconnect] Attempt #{} in {}s. Outbound messages are queued.",
                reconnect_attempt,
                delay.as_secs()
            );

            if !wait_backoff_with_buffer(&mut outbound_rx, &mut pending_frames, delay).await {
                break;
            }
            continue;
        }

        let mut authenticated = false;
        loop {
            tokio::select! {
                maybe_outbound = outbound_rx.recv() => {
                    let Some(frame) = maybe_outbound else {
                        return;
                    };

                    if let Err(error) = write.send(Message::Text(frame.clone())).await {
                        enqueue_pending_frame_front(&mut pending_frames, frame);
                        reconnect_reason = format!("outbound stream failed: {}", friendly_ws_error(&error, tls));
                        break;
                    }
                }
                inbound = read.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                                if data.get("t").and_then(|v| v.as_str()) == Some("ok") {
                                    authenticated = true;
                                }

                                if !authenticated && is_auth_rejection_frame(&data) {
                                    let msg = data
                                        .get("m")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("authentication rejected");
                                    eprintln!(
                                        "[auth] {}. Check username/password and server auth policy.",
                                        msg
                                    );
                                    return;
                                }

                                if let Some(map) = data.as_object() {
                                    handlers::dispatch_event(&state, map).await;
                                }
                            }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            reconnect_reason = frame
                                .map(|close| format!(
                                    "server closed connection (code={} reason='{}')",
                                    close.code,
                                    close.reason
                                ))
                                .unwrap_or_else(|| "server closed connection".to_string());
                            break;
                        }
                        Some(Err(error)) => {
                            reconnect_reason = friendly_ws_error(&error, tls);
                            break;
                        }
                        Some(_) => {}
                        None => {
                            reconnect_reason =
                                "websocket stream ended unexpectedly; server may have restarted"
                                    .to_string();
                            break;
                        }
                    }
                }
            }
        }

        eprintln!("[connection] {}", reconnect_reason);

        if !reconnect_enabled {
            eprintln!("[connection] auto-reconnect is off; exiting client network loop.");
            break;
        }

        reconnect_attempt = reconnect_attempt.saturating_add(1);
        let delay = reconnect_backoff_delay(reconnect_attempt);
        eprintln!(
            "[reconnect] Attempt #{} in {}s. Outbound messages are queued.",
            reconnect_attempt,
            delay.as_secs()
        );

        if !wait_backoff_with_buffer(&mut outbound_rx, &mut pending_frames, delay).await {
            break;
        }
    }
}

async fn run_input_loop(state: SharedState) {
    print_help();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print_prompt();
        let Ok(next) = lines.next_line().await else {
            break;
        };
        let Some(line) = next else {
            break;
        };

        if !handle_user_input(&state, &line).await {
            break;
        }
    }
}

#[tokio::main]
async fn main() -> ChatifyResult<()> {
    let args = Args::parse();
    let mut config = Config::load();
    NotificationService::init();
    let client_config = args.merge_with_config(&config);

    if client_config.log_enabled {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    let uri = client_config.uri();
    info!("Prepared connection target {}", uri);

    let default_username = if config.session.remember_username {
        let remembered = config.session.last_username.trim();
        if remembered.is_empty() {
            std::env::var("USERNAME")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_else(|_| "user".to_string())
        } else {
            remembered.to_string()
        }
    } else {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "user".to_string())
    };

    let input_username = prompt_input("Username", Some(&default_username))?;
    let username = sanitize_username(&input_username);

    if config.session.remember_username && config.session.last_username != username {
        config.session.last_username = username.clone();
        if let Err(err) = config.save() {
            eprintln!("failed to persist remembered username: {}", err);
        }
    }

    let mut password = String::new();
    while password.is_empty() {
        password = prompt_input("Password", None)?;
        if password.is_empty() {
            println!("Password cannot be empty.");
        }
    }
    let pw_hash = pw_hash_client(&password).map_err(ChatifyError::Validation)?;

    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let state: SharedState = Arc::new(Mutex::new(ClientState::new(
        tx,
        client_config.clone(),
        config,
    )));

    let priv_key = new_keypair();
    let pub_key = pub_b64(&priv_key).map_err(ChatifyError::Crypto)?;

    {
        let mut state_lock = state.lock().await;
        state_lock.me = username.clone();
        state_lock.pw = pw_hash.clone();
        state_lock.priv_key = priv_key;

        if state_lock.config.session.remember_channel {
            if let Some(saved_channel) =
                chatify::normalize_channel(&state_lock.config.session.last_channel)
            {
                state_lock.ch = saved_channel.clone();
                state_lock.chs.insert(saved_channel, true);
            }
        }

        match state_lock.load_trust_store() {
            Ok(true) => {
                println!(
                    "Loaded trust store with {} trusted peers.",
                    state_lock.trust_store.peers.len()
                );
            }
            Ok(false) => {}
            Err(err) => {
                eprintln!("failed to load trust store: {}", err);
            }
        }
    }

    println!(
        "Connecting to {} (auto-reconnect: {}).",
        uri,
        if client_config.auto_reconnect {
            "on"
        } else {
            "off"
        }
    );
    println!("Tip: use /commands for discoverability and /help <command> for details.");

    let auth = ConnectionAuth {
        username,
        password_hash: pw_hash,
        public_key: pub_key,
    };

    let mut connection_task = tokio::spawn(run_connection_supervisor(
        state.clone(),
        rx,
        uri,
        auth,
        client_config.auto_reconnect,
        client_config.tls,
    ));

    let mut input_task = tokio::spawn({
        let state = state.clone();
        async move {
            match chatify_client::ui::run_tui_loop(state.clone(), |state, line| async move {
                handle_user_input(&state, &line).await
            })
            .await
            {
                Ok(()) => {}
                Err(err) => {
                    std::eprintln!("tui unavailable ({}); falling back to line mode", err);
                    run_input_loop(state).await;
                }
            }
        }
    });

    tokio::select! {
        _ = &mut input_task => {}
        _ = &mut connection_task => {}
    }

    connection_task.abort();
    input_task.abort();

    {
        let state_lock = state.lock().await;
        let mut persisted = state_lock.config.clone();
        if persisted.session.remember_username {
            persisted.session.last_username = state_lock.me.clone();
        }
        if persisted.session.remember_channel {
            persisted.session.last_channel = state_lock.ch.clone();
        }
        drop(state_lock);

        if let Err(err) = persisted.save() {
            eprintln!("failed to persist session defaults: {}", err);
        }
    }

    println!("Disconnected");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_notification_export, build_notify_diagnostics_json,
        connection_doctor_recommendations, is_auth_rejection_frame, notification_key_for_token,
        notification_value_for_key, notify_export_default_path, parse_notify_probe_level,
        parse_notify_test_args, parse_plugin_command, parse_toggle_bool, reconnect_backoff_delay,
        redact_notification_export, sanitize_notify_component, PluginCommand,
    };
    #[cfg(feature = "bridge-client")]
    use super::{parse_bridge_command, BridgeCommand};

    #[test]
    fn parse_toggle_bool_accepts_truthy_and_falsey_values() {
        for value in ["on", "true", "1", "yes", "y", "ON"] {
            assert_eq!(parse_toggle_bool(value), Some(true));
        }

        for value in ["off", "false", "0", "no", "n", "OFF"] {
            assert_eq!(parse_toggle_bool(value), Some(false));
        }
    }

    #[test]
    fn parse_toggle_bool_rejects_unknown_values() {
        assert_eq!(parse_toggle_bool("maybe"), None);
        assert_eq!(parse_toggle_bool(""), None);
    }

    #[test]
    fn reconnect_backoff_delay_caps_at_maximum() {
        assert_eq!(reconnect_backoff_delay(1).as_secs(), 1);
        assert_eq!(reconnect_backoff_delay(2).as_secs(), 2);
        assert_eq!(reconnect_backoff_delay(3).as_secs(), 4);
        assert_eq!(reconnect_backoff_delay(6).as_secs(), 30);
        assert_eq!(reconnect_backoff_delay(12).as_secs(), 30);
    }

    #[test]
    fn auth_rejection_detection_is_specific_to_auth_errors() {
        assert!(is_auth_rejection_frame(
            &serde_json::json!({"t": "err", "m": "invalid credentials"})
        ));
        assert!(is_auth_rejection_frame(
            &serde_json::json!({"t": "err", "m": "unauthorized user"})
        ));
        assert!(!is_auth_rejection_frame(
            &serde_json::json!({"t": "err", "m": "rate limit exceeded"})
        ));
        assert!(!is_auth_rejection_frame(&serde_json::json!({"t": "ok"})));
    }

    #[test]
    fn doctor_recommendations_include_actionable_failures() {
        let recommendations =
            connection_doctor_recommendations(false, false, false, false, true, false);

        let joined = recommendations.join(" | ");
        assert!(joined.contains("auth profile is incomplete"));
        assert!(joined.contains("DNS resolution failed"));
        assert!(joined.contains("auto reconnect is disabled"));
    }

    #[test]
    fn doctor_recommendations_report_healthy_path() {
        let recommendations =
            connection_doctor_recommendations(true, true, true, true, false, true);
        assert_eq!(
            recommendations,
            vec!["no issues detected by doctor checks".to_string()]
        );
    }

    #[test]
    fn notification_key_for_token_supports_aliases() {
        assert_eq!(
            notification_key_for_token("enabled"),
            Some("notifications.enabled")
        );
        assert_eq!(
            notification_key_for_token("dm"),
            Some("notifications.on_dm")
        );
        assert_eq!(
            notification_key_for_token("on_dm"),
            Some("notifications.on_dm")
        );
        assert_eq!(
            notification_key_for_token("mention"),
            Some("notifications.on_mention")
        );
        assert_eq!(
            notification_key_for_token("on_mention"),
            Some("notifications.on_mention")
        );
        assert_eq!(
            notification_key_for_token("all"),
            Some("notifications.on_all_messages")
        );
        assert_eq!(
            notification_key_for_token("sound"),
            Some("notifications.sound_enabled")
        );
        assert_eq!(notification_key_for_token("unknown"), None);
    }

    #[test]
    fn parse_plugin_command_defaults_to_list() {
        assert_eq!(parse_plugin_command("/plugin"), Ok(PluginCommand::List));
        assert_eq!(
            parse_plugin_command("/plugin list"),
            Ok(PluginCommand::List)
        );
    }

    #[test]
    fn parse_plugin_command_preserves_quoted_install_spec() {
        assert_eq!(
            parse_plugin_command(r#"/plugin install "C:\Program Files\Chatify\plugins\poll.exe""#),
            Ok(PluginCommand::Install(
                r#"C:\Program Files\Chatify\plugins\poll.exe"#.to_string()
            ))
        );
    }

    #[test]
    fn parse_plugin_command_requires_install_and_disable_arguments() {
        assert_eq!(
            parse_plugin_command("/plugin install"),
            Err("Usage: /plugin install <plugin>")
        );
        assert_eq!(
            parse_plugin_command("/plugin disable"),
            Err("Usage: /plugin disable <plugin>")
        );
    }

    #[test]
    fn parse_plugin_command_rejects_extra_list_args() {
        assert_eq!(
            parse_plugin_command("/plugin list extra"),
            Err("Usage: /plugin [list|install <plugin>|disable <plugin>]")
        );
    }

    #[cfg(feature = "bridge-client")]
    #[test]
    fn parse_bridge_command_accepts_status_only() {
        assert_eq!(
            parse_bridge_command("/bridge status"),
            Ok(BridgeCommand::Status)
        );
        assert_eq!(
            parse_bridge_command("/bridge"),
            Err("Usage: /bridge status")
        );
        assert_eq!(
            parse_bridge_command("/bridge metrics"),
            Err("Usage: /bridge status")
        );
    }

    #[test]
    fn parse_notify_probe_level_supports_aliases() {
        assert_eq!(parse_notify_probe_level("info"), Some("INFO"));
        assert_eq!(parse_notify_probe_level("warning"), Some("WARNING"));
        assert_eq!(parse_notify_probe_level("warn"), Some("WARNING"));
        assert_eq!(parse_notify_probe_level("critical"), Some("CRITICAL"));
        assert_eq!(parse_notify_probe_level("crit"), Some("CRITICAL"));
        assert_eq!(parse_notify_probe_level("error"), Some("CRITICAL"));
        assert_eq!(parse_notify_probe_level("other"), None);
    }

    #[test]
    fn parse_notify_test_args_supports_sound_and_level() {
        let (sound, level, start) = parse_notify_test_args(&["test", "sound", "warning", "disk"]);
        assert!(sound);
        assert_eq!(level, "WARNING");
        assert_eq!(start, 3);

        let (sound, level, start) = parse_notify_test_args(&["test", "critical", "disk"]);
        assert!(!sound);
        assert_eq!(level, "CRITICAL");
        assert_eq!(start, 2);

        let (sound, level, start) = parse_notify_test_args(&["test", "sound"]);
        assert!(sound);
        assert_eq!(level, "INFO");
        assert_eq!(start, 2);
    }

    #[test]
    fn build_notification_export_contains_expected_fields() {
        let cfg = chatify::config::NotificationConfig {
            enabled: true,
            on_dm: false,
            on_mention: true,
            on_all_messages: false,
            sound_enabled: true,
        };

        let export = build_notification_export(&cfg, "alice", "chatify.local", 8765, true);
        assert_eq!(
            export.get("schema_version").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            export
                .get("profile")
                .and_then(|v| v.get("user"))
                .and_then(|v| v.as_str()),
            Some("alice")
        );
        assert_eq!(
            export
                .get("profile")
                .and_then(|v| v.get("host"))
                .and_then(|v| v.as_str()),
            Some("chatify.local")
        );
        assert_eq!(
            export
                .get("notifications")
                .and_then(|v| v.get("sound_enabled"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn redact_notification_export_masks_profile_identifiers() {
        let cfg = chatify::config::NotificationConfig::default();
        let export = build_notification_export(&cfg, "alice", "chatify.local", 8765, false);
        let redacted = redact_notification_export(&export);

        assert_eq!(
            redacted
                .get("profile")
                .and_then(|v| v.get("user"))
                .and_then(|v| v.as_str()),
            Some("redacted")
        );
        assert_eq!(
            redacted
                .get("profile")
                .and_then(|v| v.get("host"))
                .and_then(|v| v.as_str()),
            Some("redacted")
        );
    }

    #[test]
    fn sanitize_notify_component_replaces_unsafe_chars() {
        assert_eq!(sanitize_notify_component("Alice Admin"), "Alice_Admin");
        assert_eq!(sanitize_notify_component("chatify.local"), "chatify.local");
        assert_eq!(sanitize_notify_component(""), "default");
    }

    #[test]
    fn notify_export_default_path_contains_expected_suffix() {
        let path = notify_export_default_path("alice", "chatify.local", 8765, true);
        let as_text = path.to_string_lossy();
        assert!(as_text.contains("notify-export-alice-chatify.local-8765-tls.json"));
    }

    #[test]
    fn build_notify_diagnostics_json_contains_recommendations() {
        let cfg = chatify::config::NotificationConfig {
            enabled: false,
            on_dm: false,
            on_mention: false,
            on_all_messages: false,
            sound_enabled: false,
        };

        let diagnostics = build_notify_diagnostics_json(&cfg, "", "", 8765, false);

        let recommendations = diagnostics
            .get("recommendations")
            .and_then(|v| v.as_array())
            .expect("recommendations should be array");
        assert!(recommendations.len() >= 2);
    }

    #[test]
    fn notification_value_for_key_reads_expected_flags() {
        let cfg = chatify::config::NotificationConfig {
            enabled: true,
            on_dm: false,
            on_mention: true,
            on_all_messages: false,
            sound_enabled: true,
        };

        assert_eq!(
            notification_value_for_key(&cfg, "notifications.enabled"),
            Some(true)
        );
        assert_eq!(
            notification_value_for_key(&cfg, "notifications.on_dm"),
            Some(false)
        );
        assert_eq!(
            notification_value_for_key(&cfg, "notifications.on_mention"),
            Some(true)
        );
        assert_eq!(
            notification_value_for_key(&cfg, "notifications.on_all_messages"),
            Some(false)
        );
        assert_eq!(
            notification_value_for_key(&cfg, "notifications.sound_enabled"),
            Some(true)
        );
        assert_eq!(
            notification_value_for_key(&cfg, "notifications.unknown"),
            None
        );
    }
}
