//! Chatify WebSocket Client
//!
//! A real-time chat client with support for channels, direct messages, voice,
//! file transfers, message editing, reactions, and user status tracking.

use clicord_server::config::Config;
use clicord_server::crypto::{
    channel_key, dec_bytes, dh_key, enc_bytes, new_keypair, pub_b64, pw_hash_client,
};
use clicord_server::error::{ChatifyError, ChatifyResult};
use clicord_server::notifications::NotificationService;
use clicord_server::ui::ansi::{strip_ansi, visible_width};
use clicord_server::ui::emoji;
use clicord_server::ui::markdown::render_markdown;
use clicord_server::ui::theme::{self as theme_mod, OwnedTheme, RESET as THEME_RESET};
use clicord_server::screen_share::{
    ScreenShareManager, ScreenShareOptions, ScreenShareState, QualityPreset,
};
use clicord_server::voice::VoiceMemberInfo;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;

use zeroize::Zeroize;

use base64::{self, engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::terminal::{size as term_size, Clear, ClearType};
use rand::RngCore;
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use chrono::{DateTime, Utc};
use futures_util::SinkExt;
use futures_util::StreamExt;
use image::GenericImageView;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_HISTORY: usize = 100;
const INVALID_UTF8_PLACEHOLDER: &str = "[Invalid UTF-8]";
const HELP_TEXT: &str = "Available commands: /commands [filter], /help [command], /join, /switch, /dm, /typing [on|off] [#channel|dm:user], /status [presence|text], /theme [name|list], /emoji [on|off|list [query]], /image <path>, /video <path>, /me, /users, /channels, /voice [room], /history [channel] [window], /search <query>, /replay <timestamp>, /rewind <Ns|Nm|Nh|Nd> [limit], /fingerprint [user], /trust <user> <fingerprint>, /clear, /edit [#N] <new text>, /quit";
const RETRO_MIN_WIDTH: usize = 72;
const RETRO_MIN_HEIGHT: usize = 18;
const DASHBOARD_HEADER_ROWS: usize = 2;
const DASHBOARD_FOOTER_ROWS: usize = 1;
const DASHBOARD_GUTTER_WIDTH: usize = 3;
const SIDEBAR_RATIO_PERCENT: usize = 34;
const SIDEBAR_MIN_WIDTH: usize = 28;
const SIDEBAR_MAX_WIDTH: usize = 46;
const MAIN_MIN_WIDTH: usize = 24;
const FEED_LOOKBACK_MESSAGES: usize = 48;
const PROFILE_PANEL_ROWS: usize = 8;
const COMMANDS_PANEL_ROWS: usize = 6;
const MIN_CHANNEL_PANEL_ROWS: usize = 3;
const TRUST_STORE_FILENAME: &str = "trust-store-v1.json";
const TRUST_STORE_DIR_WINDOWS: &str = "Chatify";
const TRUST_STORE_DIR_UNIX: &str = ".chatify";
const TRUST_AUDIT_MAX_ENTRIES: usize = 64;
const CLIENT_ID_HEX_LEN: usize = 12;
const CLIENT_ID_GROUP_SIZE: usize = 4;
const DEFAULT_GUEST_PREFIX: &str = "guest";
const DEFAULT_GUEST_SUFFIX_BYTES: usize = 3;
const COMPACT_MODE_HINT: &str = "compact mode: expand terminal for side panels";
const DM_UNREAD_SCOPE_PREFIX: &str = "dm:";
const ACTIVITY_PULSE_WINDOW_SECS: u64 = 12;
const TYPING_TTL_SECS: u64 = 6;
const MAX_MEDIA_TRANSFER_BYTES: u64 = 100 * 1024 * 1024;
const FILE_CHUNK_BYTES: usize = 8 * 1024;
const MEDIA_PREVIEW_WIDTH: u16 = 56;
const MEDIA_STORE_DIR: &str = "media";

#[derive(Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    example: &'static str,
}

const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec {
        name: "/commands",
        usage: "/commands [filter]",
        summary: "Show command palette with optional filter.",
        example: "/commands dm",
    },
    CommandSpec {
        name: "/help",
        usage: "/help [command]",
        summary: "Show overview or details for one command.",
        example: "/help replay",
    },
    CommandSpec {
        name: "/join",
        usage: "/join <channel>",
        summary: "Join and focus a channel.",
        example: "/join general",
    },
    CommandSpec {
        name: "/switch",
        usage: "/switch <channel>",
        summary: "Alias for /join to switch active channel.",
        example: "/switch ops",
    },
    CommandSpec {
        name: "/dm",
        usage: "/dm <user> <message>",
        summary: "Send direct encrypted message.",
        example: "/dm alice hello",
    },
    CommandSpec {
        name: "/typing",
        usage: "/typing [on|off] [#channel|dm:user]",
        summary: "Broadcast typing state for a channel or DM scope.",
        example: "/typing on #general",
    },
    CommandSpec {
        name: "/status",
        usage: "/status [online|away|busy|invisible] [text]",
        summary: "Update your local presence and broadcast text/emoji status.",
        example: "/status away grabbing coffee",
    },
    CommandSpec {
        name: "/theme",
        usage: "/theme [name|list]",
        summary: "List available themes or switch to a new one.",
        example: "/theme dracula",
    },
    CommandSpec {
        name: "/emoji",
        usage: "/emoji [on|off|list [query]]",
        summary: "Toggle shortcode expansion and browse emoji shortcodes.",
        example: "/emoji list heart",
    },
    CommandSpec {
        name: "/image",
        usage: "/image <path>",
        summary: "Send an image file to the current channel.",
        example: "/image C:/Users/me/Pictures/screenshot.png",
    },
    CommandSpec {
        name: "/video",
        usage: "/video <path>",
        summary: "Send a video file to the current channel.",
        example: "/video C:/Users/me/Videos/demo.mp4",
    },
    CommandSpec {
        name: "/me",
        usage: "/me [action]",
        summary: "Show your profile or send an action message.",
        example: "/me is reviewing logs",
    },
    CommandSpec {
        name: "/users",
        usage: "/users",
        summary: "Refresh online users and public keys.",
        example: "/users",
    },
    CommandSpec {
        name: "/channels",
        usage: "/channels",
        summary: "List channels and online count.",
        example: "/channels",
    },
    CommandSpec {
        name: "/bridge",
        usage: "/bridge [status]",
        summary: "Show connected bridge instances (requires bridge-client feature).",
        example: "/bridge status",
    },
    CommandSpec {
        name: "/voice",
        usage: "/voice [room]",
        summary: "Toggle voice session in a room.",
        example: "/voice general",
    },
    CommandSpec {
        name: "/screen",
        usage: "/screen [start|stop|list|quality] [options]",
        summary: "Toggle screen sharing in a voice room.",
        example: "/screen start --audio",
    },
    CommandSpec {
        name: "/history",
        usage: "/history [#channel|dm:user] [window]",
        summary: "Load persisted history by scope and optional time window.",
        example: "/history #general 24h",
    },
    CommandSpec {
        name: "/search",
        usage: "/search <query>",
        summary: "Search persisted events in current channel.",
        example: "/search incident",
    },
    CommandSpec {
        name: "/replay",
        usage: "/replay <timestamp>",
        summary: "Replay events from timestamp.",
        example: "/replay 2026-03-30 12:00:00",
    },
    CommandSpec {
        name: "/rewind",
        usage: "/rewind <Ns|Nm|Nh|Nd> [limit]",
        summary: "Fetch events from recent duration.",
        example: "/rewind 2h 150",
    },
    CommandSpec {
        name: "/fingerprint",
        usage: "/fingerprint [user]",
        summary: "Inspect trust fingerprint(s).",
        example: "/fingerprint alice",
    },
    CommandSpec {
        name: "/trust",
        usage: "/trust <user> <fingerprint>",
        summary: "Trust currently observed peer fingerprint.",
        example: "/trust alice aa:bb:...",
    },
    CommandSpec {
        name: "/clear",
        usage: "/clear",
        summary: "Clear current local dashboard feed.",
        example: "/clear",
    },
    CommandSpec {
        name: "/edit",
        usage: "/edit [#N] <new text>",
        summary: "Edit your last (or Nth most recent) message.",
        example: "/edit fixed the typo",
    },
    CommandSpec {
        name: "/quit",
        usage: "/quit",
        summary: "Exit client gracefully.",
        example: "/quit",
    },
];

fn normalize_command_token(raw: &str) -> String {
    let token = raw.trim().to_lowercase();
    if token.starts_with('/') {
        token
    } else {
        format!("/{}", token)
    }
}

fn canonical_command(raw: &str) -> String {
    let token = normalize_command_token(raw);
    match token.as_str() {
        "/q" | "/exit" => "/quit".to_string(),
        "/w" => "/dm".to_string(),
        "/ty" => "/typing".to_string(),
        "/st" => "/status".to_string(),
        "/th" => "/theme".to_string(),
        "/em" => "/emoji".to_string(),
        "/img" => "/image".to_string(),
        "/vid" => "/video".to_string(),
        "/h" => "/history".to_string(),
        "/cmd" | "/palette" => "/commands".to_string(),
        "/c" => "/channels".to_string(),
        "/u" => "/users".to_string(),
        _ => token,
    }
}

fn find_command_spec(name: &str) -> Option<CommandSpec> {
    COMMAND_SPECS.iter().copied().find(|spec| spec.name == name)
}

fn unique_autocomplete_command(raw: &str) -> Option<&'static str> {
    let token = normalize_command_token(raw);
    let mut matches = COMMAND_SPECS
        .iter()
        .filter(|spec| spec.name.starts_with(&token))
        .map(|spec| spec.name);
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first)
    } else {
        None
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr: Vec<usize> = vec![0; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

fn suggest_commands(raw: &str) -> Vec<&'static str> {
    let token = normalize_command_token(raw);

    let mut prefix_matches: Vec<&'static str> = COMMAND_SPECS
        .iter()
        .filter(|spec| spec.name.starts_with(&token))
        .map(|spec| spec.name)
        .collect();
    prefix_matches.sort_unstable();
    if !prefix_matches.is_empty() {
        prefix_matches.truncate(4);
        return prefix_matches;
    }

    let mut fuzzy: Vec<(usize, &'static str)> = COMMAND_SPECS
        .iter()
        .map(|spec| (edit_distance(&token, spec.name), spec.name))
        .filter(|(dist, _)| *dist <= 4)
        .collect();
    fuzzy.sort_by(|(da, na), (db, nb)| da.cmp(db).then_with(|| na.cmp(nb)));
    fuzzy.into_iter().map(|(_, name)| name).take(4).collect()
}

fn command_palette_lines(filter_raw: &str) -> Vec<String> {
    let filter = filter_raw.trim().to_lowercase();
    let mut filtered: Vec<CommandSpec> = COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| {
            if filter.is_empty() {
                return true;
            }
            spec.name.contains(&filter)
                || spec.usage.to_lowercase().contains(&filter)
                || spec.summary.to_lowercase().contains(&filter)
        })
        .collect();
    filtered.sort_by(|a, b| a.name.cmp(b.name));

    let mut lines = vec![format!("Command palette: {} command(s)", filtered.len())];
    if !filter.is_empty() {
        lines.push(format!("Filter: '{}'", filter));
    }
    for spec in filtered {
        lines.push(format!("{} — {}", spec.usage, spec.summary));
    }
    lines
}

fn command_help_lines(raw_command: &str) -> Vec<String> {
    let token = canonical_command(raw_command);
    if let Some(spec) = find_command_spec(&token) {
        vec![
            format!("{}", spec.usage),
            format!("Summary: {}", spec.summary),
            format!("Example: {}", spec.example),
        ]
    } else {
        let suggestions = suggest_commands(raw_command);
        if suggestions.is_empty() {
            vec![
                format!("Unknown command '{}'.", raw_command.trim()),
                "Use /commands to browse available commands.".to_string(),
            ]
        } else {
            vec![
                format!("Unknown command '{}'.", raw_command.trim()),
                format!("Did you mean: {}", suggestions.join(", ")),
            ]
        }
    }
}

fn theme_name_exists(state: &ClientState, requested: &str) -> bool {
    if theme_mod::find_builtin(requested).is_some() {
        return true;
    }
    state
        .config
        .ui
        .custom_themes
        .iter()
        .any(|theme| theme.name.eq_ignore_ascii_case(requested))
}

fn list_available_themes(state: &ClientState) -> (Vec<String>, Vec<String>) {
    let mut builtin: Vec<String> = theme_mod::builtin_names()
        .into_iter()
        .map(|name| name.to_string())
        .collect();
    builtin.sort_unstable();

    let mut custom: Vec<String> = state
        .config
        .ui
        .custom_themes
        .iter()
        .map(|theme| theme.name.clone())
        .collect();
    custom.sort_unstable_by_key(|name| name.to_lowercase());

    (builtin, custom)
}

fn maybe_expand_emoji(state: &ClientState, text: &str) -> String {
    if state.config.ui.enable_emoji {
        emoji::expand_shortcodes(text)
    } else {
        text.to_string()
    }
}

fn footer_hint_line(state: &ClientState, width: usize) -> String {
    let mut users: Vec<String> = state.users.keys().cloned().collect();
    users.sort_unstable();
    let (_unknown, _trusted, changed) = state.trust_counts_for_users(&users);

    let is_sharing = state.screen_share.as_ref()
        .map(|ss| ss.session().map(|s| s.state == ScreenShareState::Sharing).unwrap_or(false))
        .unwrap_or(false);

    let raw = if changed > 0 {
        format!(
            "-> SECURITY: {} changed key(s) detected. Run /fingerprint <user> and /trust <user> <fingerprint>",
            changed
        )
    } else if is_sharing {
        "-> SCREEN sharing | /screen stop | /voice to stop | /quit".to_string()
    } else if state.voice_active {
        "-> Voice ON | /voice to stop | /typing on #room | /quit".to_string()
    } else {
        "-> /commands | /help <command> | /switch #channel | /typing on #channel | /quit"
            .to_string()
    };
    fit_to_width(&raw, width)
}

type SharedState = Arc<tokio::sync::Mutex<ClientState>>;
type JsonMap = HashMap<String, serde_json::Value>;

#[derive(Clone, Copy)]
struct DashboardGeometry {
    width: usize,
    body_rows: usize,
    left_w: usize,
    right_w: usize,
}

impl DashboardGeometry {
    fn from_terminal(width: usize, height: usize) -> Option<Self> {
        if width < RETRO_MIN_WIDTH || height < RETRO_MIN_HEIGHT {
            return None;
        }

        let body_rows = height.saturating_sub(DASHBOARD_HEADER_ROWS + DASHBOARD_FOOTER_ROWS);

        let mut right_w = (width.saturating_mul(SIDEBAR_RATIO_PERCENT) / 100)
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        if right_w + MAIN_MIN_WIDTH > width.saturating_sub(DASHBOARD_GUTTER_WIDTH) {
            right_w = width
                .saturating_sub(DASHBOARD_GUTTER_WIDTH)
                .saturating_sub(MAIN_MIN_WIDTH);
        }
        let left_w = width.saturating_sub(right_w + DASHBOARD_GUTTER_WIDTH);

        Some(Self {
            width,
            body_rows,
            left_w,
            right_w,
        })
    }
}

struct AuthContext {
    me: String,
    users: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum TrustState {
    #[default]
    Unknown,
    Trusted,
    Changed,
}

impl TrustState {
    fn label(self) -> &'static str {
        match self {
            TrustState::Unknown => "unknown",
            TrustState::Trusted => "trusted",
            TrustState::Changed => "changed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustAuditEntry {
    ts: u64,
    event: String,
    fingerprint: String,
    details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerTrustRecord {
    state: TrustState,
    current_fingerprint: String,
    trusted_fingerprint: Option<String>,
    first_seen_ts: u64,
    last_seen_ts: u64,
    last_change_ts: Option<u64>,
    audit: Vec<TrustAuditEntry>,
}

impl PeerTrustRecord {
    fn new(fingerprint: String, ts: u64, source: &str) -> Self {
        let mut record = Self {
            state: TrustState::Unknown,
            current_fingerprint: fingerprint.clone(),
            trusted_fingerprint: None,
            first_seen_ts: ts,
            last_seen_ts: ts,
            last_change_ts: None,
            audit: Vec::new(),
        };
        record.push_audit(ts, "first_seen", &fingerprint, source);
        record
    }

    fn push_audit(&mut self, ts: u64, event: &str, fingerprint: &str, details: &str) {
        self.audit.push(TrustAuditEntry {
            ts,
            event: event.to_string(),
            fingerprint: fingerprint.to_string(),
            details: details.to_string(),
        });
        if self.audit.len() > TRUST_AUDIT_MAX_ENTRIES {
            let drop_count = self.audit.len().saturating_sub(TRUST_AUDIT_MAX_ENTRIES);
            self.audit.drain(0..drop_count);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustStoreFile {
    version: u32,
    peers: HashMap<String, PeerTrustRecord>,
}

impl Default for TrustStoreFile {
    fn default() -> Self {
        Self {
            version: 1,
            peers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct TrustStore {
    path: PathBuf,
    peers: HashMap<String, PeerTrustRecord>,
}

#[derive(Debug, Clone)]
struct TrustObserveResult {
    warning: Option<String>,
    changed: bool,
    persist_required: bool,
}

impl TrustStore {
    fn default_path() -> PathBuf {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata)
                .join(TRUST_STORE_DIR_WINDOWS)
                .join(TRUST_STORE_FILENAME);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(TRUST_STORE_DIR_UNIX)
                .join(TRUST_STORE_FILENAME);
        }
        PathBuf::from(TRUST_STORE_FILENAME)
    }

    fn load_default() -> Self {
        Self::load(Self::default_path())
    }

    fn load(path: PathBuf) -> Self {
        let peers = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<TrustStoreFile>(&raw).ok())
            .map(|file| file.peers)
            .unwrap_or_default();
        Self { path, peers }
    }

    fn persist(&self) -> ChatifyResult<()> {
        let file = TrustStoreFile {
            version: 1,
            peers: self.peers.clone(),
        };
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    ChatifyError::Validation(format!(
                        "failed to create trust store directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }
        let encoded = serde_json::to_string_pretty(&file).map_err(|e| {
            ChatifyError::Validation(format!("failed to encode trust store JSON: {}", e))
        })?;
        fs::write(&self.path, encoded).map_err(|e| {
            ChatifyError::Validation(format!(
                "failed to write trust store '{}': {}",
                self.path.display(),
                e
            ))
        })
    }

    fn state_for(&self, username: &str) -> TrustState {
        self.peers
            .get(username)
            .map(|r| r.state)
            .unwrap_or(TrustState::Unknown)
    }

    fn observe(
        &mut self,
        username: &str,
        fingerprint: &str,
        source: &str,
        ts: u64,
    ) -> TrustObserveResult {
        let Some(record) = self.peers.get_mut(username) else {
            self.peers.insert(
                username.to_string(),
                PeerTrustRecord::new(fingerprint.to_string(), ts, source),
            );
            return TrustObserveResult {
                warning: None,
                changed: false,
                persist_required: true,
            };
        };

        record.last_seen_ts = ts;
        if record.current_fingerprint == fingerprint {
            if record.state == TrustState::Changed
                && record.trusted_fingerprint.as_deref() == Some(fingerprint)
            {
                record.state = TrustState::Trusted;
                record.push_audit(ts, "key_restored", fingerprint, source);
                return TrustObserveResult {
                    warning: Some(format!(
                        "Key for '{}' returned to trusted fingerprint {}.",
                        username, fingerprint
                    )),
                    changed: false,
                    persist_required: true,
                };
            }
            return TrustObserveResult {
                warning: None,
                changed: false,
                persist_required: false,
            };
        }

        let old = record.current_fingerprint.clone();
        record.current_fingerprint = fingerprint.to_string();
        record.last_change_ts = Some(ts);
        if record.trusted_fingerprint.as_deref() == Some(fingerprint) {
            record.state = TrustState::Trusted;
            record.push_audit(ts, "key_restored", fingerprint, source);
            TrustObserveResult {
                warning: Some(format!(
                    "Key for '{}' rotated back to trusted fingerprint {}.",
                    username, fingerprint
                )),
                changed: true,
                persist_required: true,
            }
        } else {
            record.state = TrustState::Changed;
            record.push_audit(
                ts,
                "key_changed",
                fingerprint,
                &format!("{}; previous={}", source, old),
            );
            TrustObserveResult {
                warning: Some(format!(
                    "SECURITY WARNING: key for '{}' changed ({} -> {}). Run /fingerprint {} then /trust {} <fingerprint>.",
                    username, old, fingerprint, username, username
                )),
                changed: true,
                persist_required: true,
            }
        }
    }

    fn trust_current_fingerprint(
        &mut self,
        username: &str,
        provided_fingerprint: &str,
        ts: u64,
    ) -> ChatifyResult<String> {
        let normalized = normalize_fingerprint_input(provided_fingerprint).ok_or_else(|| {
            ChatifyError::Validation(
                "invalid fingerprint format (expected 64 hex chars, with or without ':')"
                    .to_string(),
            )
        })?;

        let record = self.peers.get_mut(username).ok_or_else(|| {
            ChatifyError::Validation(format!("no observed key for user '{}'", username))
        })?;

        let expected =
            normalize_fingerprint_input(&record.current_fingerprint).ok_or_else(|| {
                ChatifyError::Validation(format!(
                    "stored fingerprint for '{}' is invalid; request /users and retry",
                    username
                ))
            })?;

        if normalized != expected {
            return Err(ChatifyError::Validation(format!(
                "fingerprint mismatch for '{}': expected {}",
                username, record.current_fingerprint
            )));
        }

        record.trusted_fingerprint = Some(record.current_fingerprint.clone());
        record.state = TrustState::Trusted;
        record.last_seen_ts = ts;
        let trusted_fp = record.current_fingerprint.clone();
        record.push_audit(ts, "trusted", &trusted_fp, "manual trust confirmation");
        Ok(record.current_fingerprint.clone())
    }
}

// Enumeration of supported message types in the protocol
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum MessageType {
    Msg,
    Img,
    Audio,
    Dm,
    Sys,
    UnreadMarker,
    Typing,
    Users,
    Joined,
    Info,
    Vdata,
    Err,
    FileMeta,
    FileChunk,
    StatusUpdate,
    Reaction,
    Edit,
}

/// A displayed message in the client UI
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DisplayedMessage {
    /// Formatted timestamp
    time: String,
    /// Message content
    text: String,
    /// Message type
    msg_type: MessageType,
    /// Username (if applicable)
    user: Option<String>,
    /// Channel name (if applicable)
    channel: Option<String>,
    /// If true, the message was edited after initial send
    #[allow(dead_code)]
    edited: bool,
}

impl Default for DisplayedMessage {
    fn default() -> Self {
        Self {
            time: String::new(),
            text: String::new(),
            msg_type: MessageType::Sys,
            user: None,
            channel: None,
            edited: false,
        }
    }
}

/// Record of a message the client sent, kept so `/edit` can reference it.
#[derive(Debug, Clone)]
#[expect(dead_code)]
struct SentMessage {
    /// The channel the message was sent to.
    channel: String,
    /// Plaintext content as typed by the user.
    plaintext: String,
    /// Unix timestamp (seconds) when the message was sent.
    timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

impl MediaKind {
    fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::File => "file",
        }
    }

    fn label(self) -> &'static str {
        match self {
            MediaKind::Image => "[IMAGE]",
            MediaKind::Video => "[VIDEO]",
            MediaKind::Audio => "[AUDIO]",
            MediaKind::File => "[FILE]",
        }
    }
}

/// File transfer metadata
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileTransfer {
    /// Filename for display
    filename: String,
    /// Total file size in bytes
    size: u64,
    /// Sender username
    from: String,
    /// Channel where this transfer was announced
    channel: String,
    /// Optional MIME type
    mime: Option<String>,
    /// Media kind for rendering hints
    media_kind: MediaKind,
    /// Temporary path used while receiving chunks
    temp_path: PathBuf,
    /// Final path after transfer is complete
    final_path: PathBuf,
    /// Number of bytes received so far
    received: u64,
    /// Expected next chunk index
    next_index: u64,
}

/// User presence state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum PresenceState {
    #[default]
    Online,
    Away,
    Busy,
    Invisible,
}

impl PresenceState {
    fn from_token(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "online" | "on" => Some(PresenceState::Online),
            "away" | "idle" => Some(PresenceState::Away),
            "busy" | "dnd" => Some(PresenceState::Busy),
            "invisible" | "offline" | "off" => Some(PresenceState::Invisible),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PresenceState::Online => "online",
            PresenceState::Away => "away",
            PresenceState::Busy => "busy",
            PresenceState::Invisible => "invisible",
        }
    }

    fn indicator(&self) -> char {
        match self {
            PresenceState::Online => '🟢',
            PresenceState::Away => '🟡',
            PresenceState::Busy => '🔴',
            PresenceState::Invisible => '⚫',
        }
    }
}

/// User status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct Status {
    /// Status text (e.g., "Online", "Away", "Coding")
    text: String,
    /// Status emoji (e.g., '🟢')
    emoji: char,
    /// Presence state
    #[serde(default)]
    presence: PresenceState,
    /// Short bio / description
    #[serde(default)]
    bio: String,
    /// Avatar emoji (single emoji displayed prominently)
    #[serde(default = "default_avatar")]
    avatar: String,
}

fn default_avatar() -> String {
    "😎".to_string()
}

#[derive(Debug, Clone)]
struct TypingPresence {
    user: String,
    scope: String,
    expires_at: u64,
}

/// Voice frame transferred over WebSocket.
#[derive(Debug, Clone)]
struct VoiceFrame {
    sample_rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

/// Runtime state for an active voice session.
struct VoiceSession {
    room: String,
    event_tx: std_mpsc::Sender<VoiceEvent>,
    task: thread::JoinHandle<()>,
}

enum VoiceEvent {
    Captured(VoiceFrame),
    Playback(VoiceFrame),
    #[allow(dead_code)]
    MuteState(bool),
    #[allow(dead_code)]
    SpeakingState(bool),
    Stop,
}

enum SessionStopFeedback {
    Silent,
    Notify,
}

enum CommandFlow {
    Continue,
    Exit,
}

/// Client connection state and data
#[allow(dead_code)]
struct ClientState {
    /// Message sender (queues messages to be sent via WebSocket)
    ws_tx: mpsc::UnboundedSender<String>,
    /// Current username
    me: String,
    /// Human-friendly client identifier shown in the dashboard
    client_id: String,
    /// Password (stored in memory for key derivation)
    pw: String,
    /// Current channel
    ch: String,
    /// Map of subscribed channels
    chs: HashMap<String, bool>,
    /// Known users and their public keys
    users: HashMap<String, String>,
    /// Persisted trust state and fingerprints for peers
    trust_store: TrustStore,
    /// Cached channel-specific encryption keys
    chan_keys: HashMap<String, Vec<u8>>,
    /// Cached DM-specific encryption keys
    dm_keys: HashMap<String, Vec<u8>>,
    /// Our private key for ECDH
    priv_key: Vec<u8>,
    /// Whether the client is running
    running: bool,
    /// Whether we're in a voice call
    voice_active: bool,
    /// Active voice session runtime (if any)
    voice_session: Option<VoiceSession>,
    /// Current theme (resolved from config)
    theme: OwnedTheme,
    /// Active file transfers
    file_transfers: HashMap<String, FileTransfer>,
    /// Message history for display
    message_history: VecDeque<DisplayedMessage>,
    /// Current user status
    status: Status,
    /// Message reactions (msg_id -> emoji -> count)
    reactions: HashMap<String, HashMap<String, u32>>,
    /// Unread counters keyed by scope (`channel` or `dm:<user>`)
    unread_counts: HashMap<String, usize>,
    /// Scopes that already received an unread separator in the current cycle
    unread_separator_scopes: HashSet<String>,
    /// Recent live activity hint `(label, timestamp_secs)`
    activity_hint: Option<(String, u64)>,
    /// Active typing indicators for peers
    typing_presence: HashMap<String, TypingPresence>,
    /// Recent sent messages, most recent last. Used by `/edit` to reference
    /// the user's own messages by index.
    recent_sents: VecDeque<SentMessage>,
    /// Enable debug logging
    log_enabled: bool,
    /// Configuration (for runtime access and persistence)
    config: Config,
    /// Screen sharing manager
    screen_share: Option<ScreenShareManager>,
    /// Whether we're viewing someone else's screen
    screen_viewing: bool,
    /// Voice channel members (for UI display)
    voice_members: Vec<String>,
    /// Whether we're muted in voice
    voice_muted: bool,
    /// Whether we're deafened in voice
    voice_deafened: bool,
    /// Whether we're currently speaking
    voice_speaking: bool,
    /// Whether rich media rendering is enabled
    media_enabled: bool,
}

impl ClientState {
    fn log(&self, level: &str, msg: &str) {
        if !self.log_enabled {
            return;
        }

        match level {
            "ERROR" => error!("{}", msg),
            "WARN" => warn!("{}", msg),
            "DEBUG" => debug!("{}", msg),
            _ => info!("{}", msg),
        }
    }

    fn ckey(&mut self, ch: &str) -> Vec<u8> {
        if !self.chan_keys.contains_key(ch) {
            let key = channel_key(&self.pw, ch);
            self.chan_keys.insert(ch.to_string(), key.clone());
            key
        } else {
            self.chan_keys[ch].clone()
        }
    }

    fn dmkey(&mut self, name: &str) -> ChatifyResult<Vec<u8>> {
        if self.trust_store.state_for(name) == TrustState::Changed {
            return Err(ChatifyError::Validation(format!(
                "key for '{}' is marked as changed; run /fingerprint {} and /trust {} <fingerprint>",
                name, name, name
            )));
        }

        if !self.dm_keys.contains_key(name) {
            let pk = self
                .users
                .get(name)
                .ok_or_else(|| ChatifyError::Validation(format!("user '{}' not found", name)))?;
            let key = dh_key(&self.priv_key, pk).map_err(|e| {
                ChatifyError::Crypto(format!("invalid public key for user '{}': {}", name, e))
            })?;
            self.dm_keys.insert(name.to_string(), key.clone());
            Ok(key)
        } else {
            Ok(self.dm_keys[name].clone())
        }
    }

    fn observe_peer_pubkey(&mut self, username: &str, pubkey: &str, source: &str) {
        if username.is_empty() || username == self.me || pubkey.is_empty() {
            return;
        }

        let fingerprint = match fingerprint_from_pubkey(pubkey) {
            Ok(v) => v,
            Err(e) => {
                self.log(
                    "WARN",
                    &format!("Failed to fingerprint key for '{}': {}", username, e),
                );
                return;
            }
        };

        let key_changed = self
            .users
            .get(username)
            .map(|existing| existing != pubkey)
            .unwrap_or(false);

        self.users.insert(username.to_string(), pubkey.to_string());
        if key_changed {
            self.dm_keys.remove(username);
        }

        let result = self
            .trust_store
            .observe(username, &fingerprint, source, now_secs());

        if result.changed {
            self.dm_keys.remove(username);
        }

        if let Some(msg) = result.warning {
            self.add_notice(msg);
        }

        if result.persist_required {
            if let Err(e) = self.trust_store.persist() {
                self.add_notice(format!("Failed to persist trust store: {}", e));
            }
        }
    }

    fn trust_peer(&mut self, username: &str, fingerprint: &str) -> ChatifyResult<String> {
        let confirmed =
            self.trust_store
                .trust_current_fingerprint(username, fingerprint, now_secs())?;
        self.dm_keys.remove(username);
        self.trust_store.persist()?;
        Ok(format!("Trusted '{}' fingerprint {}.", username, confirmed))
    }

    fn trust_record_line(&self, username: &str) -> Option<String> {
        self.trust_store.peers.get(username).map(|record| {
            let trusted = record.trusted_fingerprint.as_deref().unwrap_or("-");
            let changed_at = record
                .last_change_ts
                .map(format_time_full)
                .unwrap_or_else(|| "never".to_string());
            format!(
                "{} [{}] current={} trusted={} last_change={} audit_entries={}",
                username,
                record.state.label(),
                record.current_fingerprint,
                trusted,
                changed_at,
                record.audit.len()
            )
        })
    }

    fn trust_counts_for_users(&self, users: &[String]) -> (usize, usize, usize) {
        let mut unknown = 0usize;
        let mut trusted = 0usize;
        let mut changed = 0usize;
        for user in users {
            match self.trust_store.state_for(user) {
                TrustState::Unknown => unknown += 1,
                TrustState::Trusted => trusted += 1,
                TrustState::Changed => changed += 1,
            }
        }
        (unknown, trusted, changed)
    }

    fn unread_total(&self) -> usize {
        self.unread_counts.values().copied().sum()
    }

    fn unread_dm_total(&self) -> usize {
        self.unread_counts
            .iter()
            .filter(|(scope, _)| scope.starts_with(DM_UNREAD_SCOPE_PREFIX))
            .map(|(_, count)| *count)
            .sum()
    }

    fn unread_for_channel(&self, channel: &str) -> usize {
        self.unread_counts.get(channel).copied().unwrap_or(0)
    }

    fn clear_unread_for_channel(&mut self, channel: &str) {
        self.unread_counts.remove(channel);
        self.unread_separator_scopes.remove(channel);
    }

    fn clear_unread_for_dm(&mut self, peer: &str) {
        let scope = dm_unread_scope(peer);
        self.unread_counts.remove(&scope);
        self.unread_separator_scopes.remove(&scope);
    }

    fn note_unread_scope(&mut self, scope: &str) {
        if scope.trim().is_empty() {
            return;
        }

        *self.unread_counts.entry(scope.to_string()).or_insert(0) += 1;

        if self.unread_separator_scopes.insert(scope.to_string()) {
            self.push_message(DisplayedMessage {
                time: format_time(now_secs()),
                text: unread_separator_text(scope),
                msg_type: MessageType::UnreadMarker,
                user: None,
                channel: unread_separator_channel(scope),
                edited: false,
            });
        }
    }

    fn note_live_activity(&mut self, actor: &str, scope: &str) {
        self.activity_hint = Some((format!("{}@{}", actor, scope), now_secs()));
    }

    fn live_activity_label(&self) -> String {
        if let Some((label, ts)) = self.activity_hint.as_ref() {
            if now_secs().saturating_sub(*ts) <= ACTIVITY_PULSE_WINDOW_SECS {
                return label.clone();
            }
        }
        "idle".to_string()
    }

    fn prune_typing_presence(&mut self) {
        let now = now_secs();
        self.typing_presence
            .retain(|_, entry| entry.expires_at > now);
    }

    fn set_typing_presence(&mut self, user: &str, scope: &str, typing: bool) {
        if user.trim().is_empty() || scope.trim().is_empty() {
            return;
        }

        self.prune_typing_presence();
        let key = format!("{}|{}", scope.to_lowercase(), user.to_lowercase());

        if typing {
            self.typing_presence.insert(
                key,
                TypingPresence {
                    user: user.to_string(),
                    scope: scope.to_string(),
                    expires_at: now_secs().saturating_add(TYPING_TTL_SECS),
                },
            );
        } else {
            self.typing_presence.remove(&key);
        }
    }

    fn typing_count(&self) -> usize {
        let now = now_secs();
        self.typing_presence
            .values()
            .filter(|entry| entry.expires_at > now)
            .count()
    }

    fn active_typing_users(&self) -> HashSet<String> {
        let now = now_secs();
        self.typing_presence
            .values()
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.user.clone())
            .collect()
    }

    fn typing_summary(&self, max_entries: usize) -> String {
        if max_entries == 0 {
            return "none".to_string();
        }

        let now = now_secs();
        let mut labels: Vec<String> = self
            .typing_presence
            .values()
            .filter(|entry| entry.expires_at > now)
            .map(|entry| format!("{}@{}", entry.user, entry.scope))
            .collect();
        labels.sort();

        if labels.is_empty() {
            return "none".to_string();
        }

        if labels.len() <= max_entries {
            return labels.join(", ");
        }

        let extra = labels.len().saturating_sub(max_entries);
        let visible = labels.into_iter().take(max_entries).collect::<Vec<_>>();
        format!("{} +{}", visible.join(", "), extra)
    }

    /// Add a message to the history, keeping only the last 100 messages
    #[allow(dead_code)]
    fn push_message(&mut self, msg: DisplayedMessage) {
        self.message_history.push_back(msg);
        if self.message_history.len() > MAX_HISTORY {
            let _ = self.message_history.pop_front();
        }
    }

    fn refresh_dashboard(&self) {
        let _ = render_terminal_dashboard(self);
    }

    fn add_message(&mut self, msg: DisplayedMessage) {
        self.push_message(msg);
        self.refresh_dashboard();
    }

    fn add_notice(&mut self, text: impl Into<String>) {
        self.add_message(DisplayedMessage {
            time: format_time(now_secs()),
            text: text.into(),
            msg_type: MessageType::Sys,
            user: None,
            channel: None,
            edited: false,
        });
    }

    fn add_notice_batch<I>(&mut self, lines: I)
    where
        I: IntoIterator<Item = String>,
    {
        for line in lines {
            self.push_message(DisplayedMessage {
                time: format_time(now_secs()),
                text: line,
                msg_type: MessageType::Sys,
                user: None,
                channel: None,
                edited: false,
            });
        }
        self.refresh_dashboard();
    }

    /// Records a sent message so `/edit` can reference it later.
    fn record_sent(&mut self, channel: &str, plaintext: &str) {
        const MAX_SENT_HISTORY: usize = 50;
        self.recent_sents.push_back(SentMessage {
            channel: channel.to_string(),
            plaintext: plaintext.to_string(),
            timestamp: now_secs(),
        });
        while self.recent_sents.len() > MAX_SENT_HISTORY {
            self.recent_sents.pop_front();
        }
    }

    /// Returns the Nth most recent sent message (1-indexed, 1 = most recent)
    /// filtered to the given channel. Returns `None` if the index is out of
    /// range or the channel doesn't match.
    fn find_recent_sent(&self, channel: &str, index: usize) -> Option<&SentMessage> {
        if index == 0 {
            return None;
        }
        self.recent_sents
            .iter()
            .rev()
            .filter(|m| m.channel == channel)
            .nth(index - 1)
    }

    /// Finds a message in the display feed by user, channel, and content,
    /// then updates it in-place and marks it as edited.
    fn update_message_in_feed(
        &mut self,
        user: &str,
        channel: &str,
        old_text: &str,
        new_text: &str,
    ) -> bool {
        // Search in reverse to match the most recent occurrence.
        for msg in self.message_history.iter_mut().rev() {
            let channel_match = msg
                .channel
                .as_deref()
                .map(|c| c == channel)
                .unwrap_or(false);
            if msg.user.as_deref() == Some(user)
                && channel_match
                && (msg.msg_type == MessageType::Msg || msg.msg_type == MessageType::Dm)
                && msg.text == old_text
            {
                msg.text = new_text.to_string();
                msg.edited = true;
                return true;
            }
        }
        false
    }
}

// Helper functions

/// Format a Unix timestamp as HH:MM
fn format_time(ts: u64) -> String {
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or_else(Utc::now);
    datetime.format("%H:%M").to_string()
}

fn format_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}KB", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_time_full(ts: u64) -> String {
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or_else(Utc::now);
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn parse_status_payload(status: Option<&serde_json::Value>) -> (String, char) {
    let text = status
        .and_then(|value| value.get("text"))
        .and_then(|value| value.as_str())
        .unwrap_or("Online")
        .trim()
        .to_string();

    let emoji = status
        .and_then(|value| value.get("emoji"))
        .and_then(|value| value.as_str())
        .and_then(|raw| raw.chars().next())
        .unwrap_or(PresenceState::Online.indicator());

    (text, emoji)
}

/// Formats a duration in seconds as a compact human-readable string
/// (e.g. "2d 3h", "45m", "30s").
fn format_duration(secs: u64) -> String {
    if secs >= 86400 {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Render an image as compact ASCII for terminal preview.
fn img_to_ascii(bytes: &[u8], width: u16) -> String {
    let img = match image::load_from_memory(bytes) {
        Ok(v) => v,
        Err(_) => return "[image preview unavailable: decode failed]".to_string(),
    };

    let source_dims = img.dimensions();
    if source_dims.0 == 0 || source_dims.1 == 0 {
        return "[image preview unavailable: invalid dimensions]".to_string();
    }

    let target_w = u32::from(width).clamp(24, 96);
    let estimated_h =
        ((source_dims.1 as f32 / source_dims.0 as f32) * target_w as f32 * 0.5).round() as u32;
    let target_h = estimated_h.clamp(6, 24);

    let grayscale = img
        .resize_exact(target_w, target_h, image::imageops::FilterType::Triangle)
        .grayscale()
        .to_luma8();

    let ramp = b"@%#*+=-:. ";
    let mut out = String::new();
    for y in 0..grayscale.height() {
        for x in 0..grayscale.width() {
            let lum = usize::from(grayscale.get_pixel(x, y)[0]);
            let idx = lum.saturating_mul(ramp.len().saturating_sub(1)) / 255;
            out.push(ramp[idx] as char);
        }
        if y + 1 < grayscale.height() {
            out.push('\n');
        }
    }

    out
}

fn trim_wrapped_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn sanitize_filename(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }

    if out.is_empty() || out.chars().all(|ch| ch == '.') {
        "file.bin".to_string()
    } else {
        out
    }
}

fn format_byte_size(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let size_f = size as f64;
    if size_f >= GIB {
        format!("{:.2} GiB", size_f / GIB)
    } else if size_f >= MIB {
        format!("{:.2} MiB", size_f / MIB)
    } else if size_f >= KIB {
        format!("{:.2} KiB", size_f / KIB)
    } else {
        format!("{} B", size)
    }
}

fn media_store_root() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join(TRUST_STORE_DIR_WINDOWS)
            .join(MEDIA_STORE_DIR);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(TRUST_STORE_DIR_UNIX)
            .join(MEDIA_STORE_DIR);
    }
    PathBuf::from(MEDIA_STORE_DIR)
}

fn media_file_paths(file_id: &str, filename: &str) -> ChatifyResult<(PathBuf, PathBuf)> {
    let root = media_store_root();
    fs::create_dir_all(&root).map_err(|e| {
        ChatifyError::Validation(format!(
            "failed to create media directory '{}': {}",
            root.display(),
            e
        ))
    })?;

    let safe_id = sanitize_filename(file_id);
    let safe_name = sanitize_filename(filename);
    let final_name = format!("{}-{}", safe_id, safe_name);
    let final_path = root.join(&final_name);
    let temp_path = root.join(format!("{}.part", final_name));
    Ok((temp_path, final_path))
}

fn guess_mime_from_filename(filename: &str) -> Option<&'static str> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|v| v.to_str())?
        .to_ascii_lowercase();

    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "avi" => Some("video/x-msvideo"),
        "mkv" => Some("video/x-matroska"),
        "webm" => Some("video/webm"),
        "m4v" => Some("video/x-m4v"),
        _ => None,
    }
}

fn infer_media_kind(filename: &str, mime: Option<&str>) -> MediaKind {
    if let Some(raw_mime) = mime {
        let normalized = raw_mime.trim().to_ascii_lowercase();
        if normalized.starts_with("image/") {
            return MediaKind::Image;
        }
        if normalized.starts_with("video/") {
            return MediaKind::Video;
        }
    }

    match Path::new(filename)
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") => MediaKind::Image,
        Some("mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v") => MediaKind::Video,
        _ => MediaKind::File,
    }
}

fn parse_media_kind_hint(kind: Option<&str>, filename: &str, mime: Option<&str>) -> MediaKind {
    match kind.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("image") => MediaKind::Image,
        Some("video") => MediaKind::Video,
        Some("audio") => MediaKind::Audio,
        Some("file") => MediaKind::File,
        _ => infer_media_kind(filename, mime),
    }
}

fn queue_media_upload(
    state: &mut ClientState,
    path_arg: &str,
    forced_kind: MediaKind,
) -> ChatifyResult<()> {
    let raw_path = trim_wrapped_quotes(path_arg);
    if raw_path.is_empty() {
        return Err(ChatifyError::Validation(
            "missing media path (use quoted path if it contains spaces)".to_string(),
        ));
    }

    let path = Path::new(raw_path);
    let metadata = fs::metadata(path).map_err(|e| {
        ChatifyError::Validation(format!(
            "unable to read media file '{}': {}",
            path.display(),
            e
        ))
    })?;

    if !metadata.is_file() {
        return Err(ChatifyError::Validation(format!(
            "'{}' is not a file",
            path.display()
        )));
    }

    let file_size = metadata.len();
    if file_size == 0 {
        return Err(ChatifyError::Validation("media file is empty".to_string()));
    }
    if file_size > MAX_MEDIA_TRANSFER_BYTES {
        return Err(ChatifyError::Validation(format!(
            "media file exceeds max size of {} bytes",
            MAX_MEDIA_TRANSFER_BYTES
        )));
    }

    let bytes = fs::read(path).map_err(|e| {
        ChatifyError::Validation(format!(
            "failed to read media file '{}': {}",
            path.display(),
            e
        ))
    })?;

    let filename = path
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| "upload.bin".to_string());
    let mime = guess_mime_from_filename(&filename);
    let media_kind = match forced_kind {
        MediaKind::File => infer_media_kind(&filename, mime),
        _ => forced_kind,
    };

    let file_id = format!("{}-{}", state.me.to_lowercase(), fresh_nonce_hex());
    let channel = state.ch.clone();

    let mut announce = serde_json::json!({
        "t": "file_meta",
        "ch": channel.clone(),
        "filename": filename,
        "size": file_size,
        "file_id": file_id.clone(),
        "media_kind": media_kind.as_str()
    });
    if let Some(mime_type) = mime {
        announce["mime"] = serde_json::json!(mime_type);
    }
    enqueue_timed(&state.ws_tx, announce);

    for (index, chunk) in bytes.chunks(FILE_CHUNK_BYTES).enumerate() {
        enqueue_timed(
            &state.ws_tx,
            serde_json::json!({
                "t": "file_chunk",
                "ch": channel.clone(),
                "file_id": file_id.clone(),
                "index": index as u64,
                "data": general_purpose::STANDARD.encode(chunk)
            }),
        );
    }

    state.add_notice(format!(
        "{} upload queued: {} ({})",
        media_kind.label(),
        path.display(),
        format_byte_size(file_size)
    ));

    Ok(())
}

/// Normalize channel name to match server rules.
fn normalize_channel(raw: &str) -> Option<String> {
    clicord_server::normalize_channel(raw)
}

fn is_valid_username_token(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn normalize_event_scope(raw: &str, current_channel: &str) -> Option<String> {
    let candidate = raw.trim();
    if candidate.is_empty() {
        return Some(current_channel.to_string());
    }

    if let Some(peer_raw) = candidate.strip_prefix("dm:") {
        let peer = peer_raw.trim().to_lowercase();
        if is_valid_username_token(&peer) {
            return Some(format!("dm:{}", peer));
        }
        return None;
    }

    normalize_channel(candidate)
}

#[derive(Debug, Clone)]
enum TypingScope {
    Channel(String),
    Dm(String),
}

impl TypingScope {
    fn label(&self) -> String {
        match self {
            TypingScope::Channel(ch) => format!("#{}", ch),
            TypingScope::Dm(peer) => format!("dm:{}", peer),
        }
    }
}

fn parse_typing_scope(raw: &str, current_channel: &str) -> Option<TypingScope> {
    let candidate = raw.trim();
    if candidate.is_empty() {
        return Some(TypingScope::Channel(current_channel.to_string()));
    }

    if let Some(peer_raw) = candidate.strip_prefix("dm:") {
        let peer = peer_raw.trim().to_lowercase();
        if is_valid_username_token(&peer) {
            return Some(TypingScope::Dm(peer));
        }
        return None;
    }

    normalize_channel(candidate).map(TypingScope::Channel)
}

fn enqueue_typing_state(ws_tx: &mpsc::UnboundedSender<String>, scope: &TypingScope, typing: bool) {
    let payload = match scope {
        TypingScope::Channel(ch) => serde_json::json!({
            "t": "typing",
            "ch": ch,
            "typing": typing
        }),
        TypingScope::Dm(peer) => serde_json::json!({
            "t": "typing",
            "to": peer,
            "typing": typing
        }),
    };
    enqueue_timed(ws_tx, payload);
}

fn dm_unread_scope(peer: &str) -> String {
    format!("{}{}", DM_UNREAD_SCOPE_PREFIX, peer.trim().to_lowercase())
}

fn unread_separator_channel(scope: &str) -> Option<String> {
    if scope.starts_with(DM_UNREAD_SCOPE_PREFIX) {
        None
    } else {
        Some(scope.to_string())
    }
}

fn unread_separator_text(scope: &str) -> String {
    if let Some(peer) = scope.strip_prefix(DM_UNREAD_SCOPE_PREFIX) {
        return format!("----- new DM from {} -----", peer);
    }
    format!("----- new activity in #{} -----", scope)
}

fn parse_history_args(args: &str, current_channel: &str) -> Result<(String, Option<u64>), String> {
    let mut parts = args.split_whitespace();
    let first = parts.next();
    let second = parts.next();
    if parts.next().is_some() {
        return Err("Usage: /history [#channel|dm:user] [window]".to_string());
    }

    match (first, second) {
        (None, None) => Ok((current_channel.to_string(), None)),
        (Some(one), None) => {
            if let Some(window_secs) = parse_duration_secs(one) {
                Ok((current_channel.to_string(), Some(window_secs)))
            } else if let Some(scope) = normalize_event_scope(one, current_channel) {
                Ok((scope, None))
            } else {
                Err("Usage: /history [#channel|dm:user] [window]".to_string())
            }
        }
        (Some(scope_raw), Some(window_raw)) => {
            let Some(scope) = normalize_event_scope(scope_raw, current_channel) else {
                return Err("Usage: /history [#channel|dm:user] [window]".to_string());
            };
            let Some(window_secs) = parse_duration_secs(window_raw) else {
                return Err("Invalid window. Use values like 90s, 15m, 24h, 7d.".to_string());
            };
            Ok((scope, Some(window_secs)))
        }
        _ => Err("Usage: /history [#channel|dm:user] [window]".to_string()),
    }
}

fn parse_replay_timestamp(raw: &str) -> Option<f64> {
    let ts = raw.trim();
    if ts.is_empty() {
        return None;
    }

    if let Ok(v) = ts.parse::<f64>() {
        if v.is_finite() && v >= 0.0 {
            return Some(v);
        }
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return Some(dt.timestamp_millis() as f64 / 1000.0);
    }

    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp_millis() as f64 / 1000.0);
    }

    None
}

fn format_scope_label(scope: &str) -> String {
    if scope.starts_with("dm:") {
        scope.to_string()
    } else {
        format!("#{}", scope)
    }
}

fn normalize_fingerprint_input(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.len() == 64 {
        Some(cleaned)
    } else {
        None
    }
}

fn canonical_fingerprint_from_hex(hex: &str) -> String {
    hex.as_bytes()
        .chunks(2)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .collect::<Vec<_>>()
        .join(":")
}

fn fingerprint_from_pubkey(pubkey_b64: &str) -> ChatifyResult<String> {
    let bytes = general_purpose::STANDARD.decode(pubkey_b64).map_err(|e| {
        ChatifyError::Validation(format!(
            "invalid public key base64 while fingerprinting: {}",
            e
        ))
    })?;
    let digest = Sha256::digest(bytes);
    Ok(canonical_fingerprint_from_hex(&hex::encode(digest)))
}

fn parse_limit(input: &str, default: usize, max: usize) -> usize {
    input
        .trim()
        .parse::<usize>()
        .ok()
        .unwrap_or(default)
        .clamp(1, max)
}

fn parse_duration_secs(spec: &str) -> Option<u64> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    let (num, mul) = match s.chars().last()? {
        's' | 'S' => (&s[..s.len().saturating_sub(1)], 1u64),
        'm' | 'M' => (&s[..s.len().saturating_sub(1)], 60u64),
        'h' | 'H' => (&s[..s.len().saturating_sub(1)], 3600u64),
        'd' | 'D' => (&s[..s.len().saturating_sub(1)], 86400u64),
        _ => (s, 1u64),
    };
    num.trim()
        .parse::<u64>()
        .ok()
        .map(|v| v.saturating_mul(mul))
}

fn ts_value(v: &serde_json::Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f as u64))
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    clicord_server::now_secs()
}

fn truncate_with_ellipsis(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let vis_len = visible_width(input);
    if vis_len <= width {
        return input.to_string();
    }

    // For rich ANSI strings, truncation is unsafe for terminal state,
    // so we return it intact (or we could strip it).
    if input.contains('\x1b') {
        let stripped = strip_ansi(input);
        return truncate_with_ellipsis(&stripped, width);
    }

    let mut out = String::new();
    let mut chars = input.chars();
    for _ in 0..width {
        match chars.next() {
            Some(ch) => out.push(ch),
            None => return out,
        }
    }

    if chars.next().is_some() {
        if width == 1 {
            return ".".to_string();
        }
        out.pop();
        out.push('…');
    }

    out
}

fn fit_to_width(input: &str, width: usize) -> String {
    let clipped = truncate_with_ellipsis(input, width);
    let clipped_len = visible_width(&clipped);
    if clipped_len >= width {
        clipped
    } else {
        format!("{}{}", clipped, " ".repeat(width - clipped_len))
    }
}

fn wrap_words(input: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;

    for word in input.split_whitespace() {
        let word_len = visible_width(word);
        let next_len = if current_len == 0 {
            word_len
        } else {
            current_len + 1 + word_len
        };

        if next_len > width {
            if !current.is_empty() {
                lines.push(current);
            }
            if word_len > width {
                current = truncate_with_ellipsis(word, width);
                current_len = visible_width(&current);
            } else {
                current = word.to_string();
                current_len = word_len;
            }
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            current_len = next_len;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    if lines.len() > max_lines {
        lines.split_off(lines.len() - max_lines)
    } else {
        lines
    }
}

fn build_panel(title: &str, lines: &[String], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if width < 2 {
        return vec![fit_to_width("", width); height];
    }

    let inner = width.saturating_sub(2);
    let titled = truncate_with_ellipsis(&format!(" {} ", title), inner);
    let titled_len = titled.chars().count();

    let mut out = Vec::with_capacity(height);
    out.push(format!(
        "┌{}{}┐",
        titled,
        "─".repeat(inner.saturating_sub(titled_len))
    ));

    let body_rows = height.saturating_sub(2);
    for idx in 0..body_rows {
        let raw = lines.get(idx).cloned().unwrap_or_default();
        out.push(format!("│{}│", fit_to_width(&raw, inner)));
    }

    if height > 1 {
        out.push(format!("└{}┘", "─".repeat(inner)));
    }

    out
}

fn format_feed_entry(msg: &DisplayedMessage) -> String {
    let stamp = format!("[{}]", msg.time);
    match msg.msg_type {
        MessageType::Msg => {
            let user = msg.user.as_deref().unwrap_or("?");
            let suffix = if msg.edited { " (edited)" } else { "" };
            format!("{} {}: {}{}", stamp, user, msg.text, suffix)
        }
        MessageType::Dm => {
            let user = msg.user.as_deref().unwrap_or("?");
            let suffix = if msg.edited { " (edited)" } else { "" };
            format!("{} DM {} {}{}", stamp, user, msg.text, suffix)
        }
        MessageType::Edit => format!("{} edit {}", stamp, msg.text),
        MessageType::UnreadMarker => msg.text.clone(),
        MessageType::Sys => format!("{} • {}", stamp, msg.text),
        _ => format!("{} {}", stamp, msg.text),
    }
}

fn feed_empty_state_lines() -> Vec<String> {
    vec![
        "No messages yet in this room.".to_string(),
        "Start with a hello or switch rooms using /join <channel>.".to_string(),
        "Tip: use /commands to discover power actions quickly.".to_string(),
    ]
}

fn prefixed_wrapped_lines(prefix: &str, text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let prefix_width = prefix.chars().count();
    let content_width = width.saturating_sub(prefix_width).max(1);
    wrap_words(text, content_width, max_lines)
        .into_iter()
        .map(|line| format!("{}{}", prefix, line))
        .collect()
}

fn prefixed_preformatted_lines(
    prefix: &str,
    text: &str,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let continuation = " ".repeat(prefix.chars().count());
    let mut output = Vec::new();
    let mut seen_any = false;

    for (index, raw_line) in text.lines().enumerate() {
        if output.len() >= max_lines {
            break;
        }
        let marker = if index == 0 { prefix } else { &continuation };
        let content_width = width.saturating_sub(marker.chars().count()).max(1);
        let clipped = truncate_with_ellipsis(raw_line, content_width);
        output.push(format!("{}{}", marker, clipped));
        seen_any = true;
    }

    if !seen_any {
        output.push(prefix.to_string());
    }

    output
}

fn render_grouped_feed_lines<'a, I>(
    messages: I,
    width: usize,
    max_lines: usize,
    enable_markdown: bool,
    enable_syntax_highlighting: bool,
) -> Vec<String>
where
    I: IntoIterator<Item = &'a DisplayedMessage>,
{
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current_group: Option<(String, String)> = None;

    for msg in messages {
        match msg.msg_type {
            MessageType::Msg => {
                let user = msg.user.as_deref().unwrap_or("?");
                let channel = msg.channel.as_deref().unwrap_or("general");
                let key = (user.to_string(), channel.to_string());
                let group_started = current_group.as_ref() != Some(&key);

                if group_started {
                    lines.push(format!("[{}] {}  #{}", msg.time, user, channel));
                }

                let prefix = if group_started { "  > " } else { "    " };
                let suffix = if msg.edited { " (edited)" } else { "" };
                let display_text = format!("{}{}", msg.text, suffix);

                // Markdown applied AFTER structural logic, BEFORE wrapping
                let styled_text =
                    render_markdown(&display_text, enable_markdown, enable_syntax_highlighting);

                lines.extend(prefixed_wrapped_lines(
                    prefix,
                    &styled_text,
                    width,
                    max_lines,
                ));
                current_group = Some(key);
            }
            MessageType::Img => {
                let user = msg.user.as_deref().unwrap_or("?");
                let header = format!("[{}] IMG {} ", msg.time, user);
                lines.extend(prefixed_preformatted_lines(
                    &header, &msg.text, width, max_lines,
                ));
                current_group = None;
            }
            MessageType::Audio => {
                let user = msg.user.as_deref().unwrap_or("?");
                let header = format!("[{}] AUDIO {} ", msg.time, user);
                lines.extend(prefixed_preformatted_lines(
                    &header, &msg.text, width, max_lines,
                ));
                current_group = None;
            }
            _ => {
                lines.extend(wrap_words(&format_feed_entry(msg), width, max_lines));
                current_group = None;
            }
        }
    }

    if lines.len() > max_lines {
        lines.split_off(lines.len() - max_lines)
    } else {
        lines
    }
}

fn collect_recent_feed_lines(state: &ClientState, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let start = state
        .message_history
        .len()
        .saturating_sub(FEED_LOOKBACK_MESSAGES);
    let mut lines = render_grouped_feed_lines(
        state.message_history.iter().skip(start),
        width,
        max_lines,
        state.config.ui.enable_markdown,
        state.config.ui.enable_syntax_highlighting,
    );

    if lines.is_empty() {
        lines = feed_empty_state_lines();
    }

    if lines.len() > max_lines {
        lines.split_off(lines.len() - max_lines)
    } else {
        lines
    }
}

fn build_right_column(state: &ClientState, width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let profile_h = PROFILE_PANEL_ROWS;
    let commands_h = COMMANDS_PANEL_ROWS;
    let remaining = height.saturating_sub(profile_h + commands_h);
    let online_h = remaining.saturating_mul(2) / 3;
    let channels_h = remaining.saturating_sub(online_h);

    let mut known_users: Vec<String> = state.users.keys().cloned().collect();
    known_users.sort_unstable();
    let (unknown, trusted, changed) = state.trust_counts_for_users(&known_users);
    let unread_total = state.unread_total();
    let unread_dm = state.unread_dm_total();
    let typing_summary = state.typing_summary(2);

    let profile_lines = vec![
        format!(
            "{} {} {} [{}]",
            state.status.avatar, state.me, state.status.emoji, state.client_id
        ),
        format!(
            "status: {} ({})",
            state.status.text,
            state.status.presence.as_str()
        ),
        format!(
            "bio: {}",
            if state.status.bio.trim().is_empty() {
                "(none)"
            } else {
                state.status.bio.as_str()
            }
        ),
        format!("channel: #{}", state.ch),
        format!("voice: {}", if state.voice_active { "ON" } else { "OFF" }),
        format!(
            "trust T:{} U:{} C:{} | unread:{} (dm:{})",
            trusted, unknown, changed, unread_total, unread_dm
        ),
        format!("typing: {}", typing_summary),
    ];

    let mut command_lines = vec![
        "/commands [filter]".to_string(),
        "/help [command]".to_string(),
        "/join <channel> /switch".to_string(),
        "/dm <user> <msg>".to_string(),
        "/status [presence] [text]".to_string(),
        "/theme [name|list]".to_string(),
        "/emoji [on|off|list]".to_string(),
        "/image <path>".to_string(),
        "/video <path>".to_string(),
        "/typing on|off [scope]".to_string(),
        "/users /channels".to_string(),
        "/history /search /replay".to_string(),
        "/fingerprint /trust /quit".to_string(),
    ];
    if let Some(last) = state.message_history.back() {
        command_lines.push(truncate_with_ellipsis(
            &format!("last: {}", format_feed_entry(last)),
            width.saturating_sub(2),
        ));
    }

    let mut online_users: Vec<String> = state.users.keys().cloned().collect();
    online_users.sort_unstable();
    let typing_users = state.active_typing_users();
    let online_lines = if online_users.is_empty() {
        vec![
            "No peers online yet.".to_string(),
            "Use /users to refresh live roster.".to_string(),
        ]
    } else {
        online_users
            .into_iter()
            .map(|u| {
                if typing_users.contains(&u) {
                    format!("• {} (typing)", u)
                } else {
                    format!("• {}", u)
                }
            })
            .collect()
    };

    let mut channels: Vec<String> = state.chs.keys().cloned().collect();
    channels.sort_unstable();
    let channel_lines = if channels.is_empty() {
        vec![
            "No channels tracked yet.".to_string(),
            "Join one: /join general".to_string(),
        ]
    } else {
        channels
            .into_iter()
            .map(|ch| {
                let unread = state.unread_for_channel(&ch);
                let unread_badge = if unread > 0 {
                    format!(" [{}]", unread)
                } else {
                    String::new()
                };
                if ch == state.ch {
                    format!("* #{}{}", ch, unread_badge)
                } else {
                    format!("  #{}{}", ch, unread_badge)
                }
            })
            .collect()
    };

    let mut out = Vec::new();
    out.extend(build_panel(
        "PROFILE",
        &profile_lines,
        width,
        profile_h.min(height),
    ));
    if out.len() < height {
        out.extend(build_panel(
            "QUICK ACTIONS",
            &command_lines,
            width,
            commands_h.min(height.saturating_sub(out.len())),
        ));
    }
    if out.len() < height {
        out.extend(build_panel(
            "LIVE ROSTER",
            &online_lines,
            width,
            online_h.min(height.saturating_sub(out.len())),
        ));
    }
    if out.len() < height {
        out.extend(build_panel(
            "CHANNEL DOCK",
            &channel_lines,
            width,
            channels_h
                .max(MIN_CHANNEL_PANEL_ROWS)
                .min(height.saturating_sub(out.len())),
        ));
    }

    if out.len() < height {
        out.extend(std::iter::repeat_n(" ".repeat(width), height - out.len()));
    }
    out.truncate(height);
    out
}

fn render_compact_dashboard(
    out: &mut io::Stdout,
    state: &ClientState,
    width: usize,
    height: usize,
) -> io::Result<()> {
    let (header_line, status_line) = dashboard_header_lines(state);
    writeln!(
        out,
        "{}{}{}",
        state.theme.header,
        fit_to_width(&header_line, width),
        THEME_RESET
    )?;
    writeln!(
        out,
        "{}{}{}",
        state.theme.dim,
        fit_to_width(&format!("{} | {}", status_line, COMPACT_MODE_HINT), width,),
        THEME_RESET
    )?;

    let max_feed_lines = height.saturating_sub(DASHBOARD_HEADER_ROWS + DASHBOARD_FOOTER_ROWS);
    let feed_lines = collect_recent_feed_lines(state, width, max_feed_lines);
    for line in feed_lines {
        writeln!(out, "{}", fit_to_width(&line, width))?;
    }

    writeln!(
        out,
        "{}{}{}",
        state.theme.hint,
        footer_hint_line(state, width),
        THEME_RESET
    )
}

fn render_full_dashboard(
    out: &mut io::Stdout,
    state: &ClientState,
    geom: DashboardGeometry,
) -> io::Result<()> {
    let (header, subtitle) = dashboard_header_lines(state);

    writeln!(
        out,
        "{}{}{}",
        state.theme.header,
        fit_to_width(&header, geom.width),
        THEME_RESET
    )?;
    writeln!(
        out,
        "{}{}{}",
        state.theme.dim,
        fit_to_width(&subtitle, geom.width),
        THEME_RESET
    )?;

    let left_feed_lines =
        collect_recent_feed_lines(state, geom.left_w.saturating_sub(2), geom.body_rows);
    let left_panel = build_panel("GLOBAL_FEED", &left_feed_lines, geom.left_w, geom.body_rows);
    let right_panel = build_right_column(state, geom.right_w, geom.body_rows);

    for row in 0..geom.body_rows {
        let left_line = left_panel
            .get(row)
            .cloned()
            .unwrap_or_else(|| " ".repeat(geom.left_w));
        let right_line = right_panel
            .get(row)
            .cloned()
            .unwrap_or_else(|| " ".repeat(geom.right_w));
        writeln!(
            out,
            "{}{}{}   {}{}{}",
            state.theme.feed_text,
            left_line,
            THEME_RESET,
            state.theme.sidebar_text,
            right_line,
            THEME_RESET
        )?;
    }

    writeln!(
        out,
        "{}{}{}",
        state.theme.hint,
        footer_hint_line(state, geom.width),
        THEME_RESET
    )
}

fn render_terminal_dashboard(state: &ClientState) -> io::Result<()> {
    let (w, h) = term_size().unwrap_or((120, 36));
    let width = usize::from(w);
    let height = usize::from(h);

    let mut out = io::stdout();
    execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;

    if let Some(geom) = DashboardGeometry::from_terminal(width, height) {
        render_full_dashboard(&mut out, state, geom)?;
    } else {
        render_compact_dashboard(&mut out, state, width, height)?;
    }

    out.flush()
}

fn status_chip(label: &str, value: impl std::fmt::Display) -> String {
    format!("[{}:{}]", label, value)
}

fn dashboard_header_lines(state: &ClientState) -> (String, String) {
    let mut known_users: Vec<String> = state.users.keys().cloned().collect();
    known_users.sort_unstable();
    let (unknown, trusted, changed) = state.trust_counts_for_users(&known_users);

    let header = format!(
        "// CHATIFY // {} {} {} {} {} {}",
        status_chip("ONLINE", state.users.len()),
        status_chip("CHANNELS", state.chs.len()),
        status_chip("EVENTS", state.message_history.len()),
        status_chip("UNREAD", state.unread_total()),
        status_chip("TYPING", state.typing_count()),
        status_chip("THEME", &state.theme.name),
    );

    let subtitle = format!(
        "{} {} {} {} {} {}",
        status_chip("ROOM", format!("#{}", state.ch)),
        status_chip("VOICE", if state.voice_active { "ON" } else { "OFF" }),
        status_chip("TRUST", format!("T{}/U{}/C{}", trusted, unknown, changed)),
        status_chip(
            "STATUS",
            format!(
                "{} {} ({})",
                state.status.emoji,
                state.status.text,
                state.status.presence.as_str()
            )
        ),
        status_chip("LIVE", state.live_activity_label()),
        status_chip("CLIENT", &state.client_id),
    );

    (header, subtitle)
}

fn fresh_nonce_hex() -> String {
    clicord_server::fresh_nonce_hex()
}

fn generate_guest_username() -> String {
    let mut bytes = [0u8; DEFAULT_GUEST_SUFFIX_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{}-{}", DEFAULT_GUEST_PREFIX, hex::encode(bytes))
}

fn build_client_id(username: &str, public_key_b64: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update(b":");
    hasher.update(public_key_b64.as_bytes());

    let digest_hex = hex::encode(hasher.finalize());
    let compact = digest_hex
        .chars()
        .take(CLIENT_ID_HEX_LEN)
        .collect::<String>()
        .to_ascii_uppercase();

    let mut grouped = String::with_capacity(compact.len() + compact.len() / CLIENT_ID_GROUP_SIZE);
    for (index, ch) in compact.chars().enumerate() {
        if index > 0 && index % CLIENT_ID_GROUP_SIZE == 0 {
            grouped.push('-');
        }
        grouped.push(ch);
    }

    format!("CID-{}", grouped)
}

fn read_username() -> ChatifyResult<String> {
    print!("username: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let username = input.trim();
    if username.is_empty() {
        let guest_username = generate_guest_username();
        println!("Empty username provided, using '{}'.", guest_username);
        Ok(guest_username)
    } else {
        Ok(username.to_string())
    }
}

fn extract_auth_text(
    auth_reply: Result<
        Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        tokio::time::error::Elapsed,
    >,
) -> ChatifyResult<String> {
    match auth_reply {
        Ok(Some(Ok(Message::Text(resp)))) => Ok(resp),
        Ok(Some(Ok(_))) => Err(ChatifyError::Validation(
            "authentication failed: server sent unexpected frame".to_string(),
        )),
        Ok(Some(Err(e))) => Err(ChatifyError::WebSocket(Box::new(e))),
        Ok(None) => Err(ChatifyError::Validation(
            "authentication failed: connection closed by server".to_string(),
        )),
        Err(_) => Err(ChatifyError::Validation(
            "authentication failed: timeout waiting for server response".to_string(),
        )),
    }
}

fn parse_auth_payload(resp: &str, fallback_username: &str) -> ChatifyResult<AuthContext> {
    let resp_val: serde_json::Value = serde_json::from_str(resp)?;
    let typ = resp_val.get("t").and_then(|v| v.as_str()).unwrap_or("");
    if typ == "err" {
        let msg = resp_val
            .get("m")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown server error")
            .to_string();
        return Err(ChatifyError::Validation(format!(
            "authentication failed: {}",
            msg
        )));
    }
    if typ != "ok" {
        return Err(ChatifyError::Validation(format!(
            "authentication failed: unexpected handshake type '{}'",
            typ
        )));
    }
    if !resp_val.get("users").map(|v| v.is_array()).unwrap_or(false) {
        return Err(ChatifyError::Validation(
            "authentication failed: malformed users payload".to_string(),
        ));
    }
    if !resp_val
        .get("channels")
        .map(|v| v.is_array())
        .unwrap_or(false)
    {
        return Err(ChatifyError::Validation(
            "authentication failed: malformed channels payload".to_string(),
        ));
    }

    let me = resp_val["u"]
        .as_str()
        .unwrap_or(fallback_username)
        .to_string();

    let users: Vec<serde_json::Value> =
        serde_json::from_value(resp_val["users"].clone()).unwrap_or_default();
    let mut user_map: HashMap<String, String> = HashMap::new();
    for user in users {
        if let Some(name) = user.get("u").and_then(|v| v.as_str()) {
            if let Some(pk) = user.get("pk").and_then(|v| v.as_str()) {
                user_map.insert(name.to_string(), pk.to_string());
            }
        }
    }

    Ok(AuthContext {
        me,
        users: user_map,
    })
}

fn enqueue_json(ws_tx: &mpsc::UnboundedSender<String>, payload: serde_json::Value) {
    let _ = ws_tx.send(payload.to_string());
}

fn enqueue_timed(ws_tx: &mpsc::UnboundedSender<String>, mut payload: serde_json::Value) {
    if payload.get("ts").is_none() {
        payload["ts"] = serde_json::json!(now_secs());
    }
    if payload.get("n").is_none() {
        payload["n"] = serde_json::json!(fresh_nonce_hex());
    }
    enqueue_json(ws_tx, payload);
}

fn append_event_to_history(state: &mut ClientState, event: &serde_json::Value) -> bool {
    let t = event.get("t").and_then(|v| v.as_str()).unwrap_or("");
    let ts = event.get("ts").map(ts_value).unwrap_or(0);
    match t {
        "msg" => {
            let ch = event
                .get("ch")
                .and_then(|v| v.as_str())
                .unwrap_or("general");
            let u = event.get("u").and_then(|v| v.as_str()).unwrap_or("?");
            let c = event.get("c").and_then(|v| v.as_str()).unwrap_or("");
            let encrypted = match general_purpose::STANDARD.decode(c) {
                Ok(bytes) => bytes,
                Err(_) => return false,
            };
            let content = match dec_bytes(&state.ckey(ch), &encrypted) {
                Ok(content) => String::from_utf8(content)
                    .unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string()),
                Err(_) => return false,
            };
            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: content,
                msg_type: MessageType::Msg,
                user: Some(u.to_string()),
                channel: Some(ch.to_string()),
                edited: false,
            });
            true
        }
        "sys" => {
            let m = event.get("m").and_then(|v| v.as_str()).unwrap_or("");
            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: m.to_string(),
                msg_type: MessageType::Sys,
                user: None,
                channel: None,
                edited: false,
            });
            true
        }
        "edit" => {
            let user = event.get("u").and_then(|v| v.as_str()).unwrap_or("?");
            let old_text = event.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
            let new_text = event.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
            let ch = event.get("ch").and_then(|v| v.as_str()).unwrap_or("");

            // Try to update the original message in-place.
            if !state.update_message_in_feed(user, ch, old_text, new_text) {
                // Original not found (scrollback limit or different client) —
                // show an edit notice instead.
                state.add_message(DisplayedMessage {
                    time: format_time(ts),
                    text: format!("{} edited a message: '{}'", user, new_text),
                    msg_type: MessageType::Edit,
                    user: Some(user.to_string()),
                    channel: Some(ch.to_string()),
                    edited: false,
                });
            }
            state.refresh_dashboard();
            true
        }
        "dm" => {
            let from = event.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            let to = event.get("to").and_then(|v| v.as_str()).unwrap_or("?");

            let c = event.get("c").and_then(|v| v.as_str()).unwrap_or("");
            let encrypted = match general_purpose::STANDARD.decode(c) {
                Ok(bytes) => bytes,
                Err(_) => return false,
            };

            let peer = if from == state.me { to } else { from };
            let dm_key = if let Some(pk) = event.get("pk").and_then(|v| v.as_str()) {
                if pk.is_empty() {
                    match state.dmkey(peer) {
                        Ok(key) => key,
                        Err(_) => return false,
                    }
                } else {
                    match dh_key(&state.priv_key, pk) {
                        Ok(key) => key,
                        Err(_) => return false,
                    }
                }
            } else {
                match state.dmkey(peer) {
                    Ok(key) => key,
                    Err(_) => return false,
                }
            };
            let content = match dec_bytes(&dm_key, &encrypted) {
                Ok(content) => String::from_utf8(content)
                    .unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string()),
                Err(_) => return false,
            };
            let arrow = if from == state.me { "→" } else { "←" };

            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!("{} {} {}", from, arrow, content),
                msg_type: MessageType::Dm,
                user: Some(from.to_string()),
                channel: None,
                edited: false,
            });
            true
        }
        "join" => {
            let ch = event
                .get("ch")
                .and_then(|v| v.as_str())
                .unwrap_or("general");
            let user = event.get("u").and_then(|v| v.as_str()).unwrap_or("?");
            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!("{} joined #{}", user, ch),
                msg_type: MessageType::Sys,
                user: Some(user.to_string()),
                channel: Some(ch.to_string()),
                edited: false,
            });
            true
        }
        "file_meta" => {
            let ch = event
                .get("ch")
                .and_then(|v| v.as_str())
                .unwrap_or("general");
            let from = event.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            let filename = event
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("file.bin");
            let size = event.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let media_kind = parse_media_kind_hint(
                event.get("media_kind").and_then(|v| v.as_str()),
                filename,
                event.get("mime").and_then(|v| v.as_str()),
            );
            let mime_hint = event
                .get("mime")
                .and_then(|v| v.as_str())
                .map(|v| format!(" | {}", v))
                .unwrap_or_default();

            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!(
                    "{} {} shared '{}' ({}){}",
                    media_kind.label(),
                    from,
                    filename,
                    format_byte_size(size),
                    mime_hint
                ),
                msg_type: MessageType::FileMeta,
                user: Some(from.to_string()),
                channel: Some(ch.to_string()),
                edited: false,
            });
            true
        }
        _ => false,
    }
}

async fn handle_msg_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };

    let mut state_lock = state.lock().await;
    let key = state_lock.ckey(ch);
    if let Ok(content) = dec_bytes(&key, &encrypted) {
        let message =
            String::from_utf8(content).unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string());
        let is_self = u == state_lock.me.as_str();
        if !is_self {
            state_lock.note_live_activity(u, &format!("#{}", ch));
            if ch != state_lock.ch {
                state_lock.note_unread_scope(ch);
            }

            let mentioned = message.contains(&format!("@{}", state_lock.me));
            if state_lock.config.notifications.enabled
                && (state_lock.config.notifications.on_all_messages
                    || (mentioned && state_lock.config.notifications.on_mention))
            {
                NotificationService::send(
                    &state_lock.config.notifications,
                    &format!("New message in #{}", ch),
                    &format!("{}: {}", u, message),
                    mentioned, // force if mentioned
                );
            }
        }

        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: message,
            msg_type: MessageType::Msg,
            user: Some(u.to_string()),
            channel: Some(ch.to_string()),
            edited: false,
        });
    }
}

async fn handle_img_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let a = data.get("a").and_then(|v| v.as_str()).unwrap_or("");

    let mut state_lock = state.lock().await;
    
    if u != state_lock.me {
        state_lock.note_live_activity(u, &format!("#{}", ch));
        if ch != state_lock.ch {
            state_lock.note_unread_scope(ch);
        }
    }
    
    if state_lock.media_enabled {
        let image_bytes = match general_purpose::STANDARD.decode(a) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let ascii_art = img_to_ascii(&image_bytes, MEDIA_PREVIEW_WIDTH);
        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: format!("inline image\n{}", ascii_art),
            msg_type: MessageType::Img,
            user: Some(u.to_string()),
            channel: Some(ch.to_string()),
            edited: false,
        });
    } else {
        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: format!("[📷 Image from {}]", u),
            msg_type: MessageType::Sys,
            user: Some(u.to_string()),
            channel: Some(ch.to_string()),
            edited: false,
        });
    }
}

async fn handle_file_meta_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let file_id = data
        .get("file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if file_id.is_empty() {
        return;
    }

    let from = data
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let channel = data
        .get("ch")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let filename = sanitize_filename(
        data.get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("file.bin"),
    );
    let size = data.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    if size > MAX_MEDIA_TRANSFER_BYTES {
        return;
    }

    let mime = data
        .get("mime")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    let media_kind = parse_media_kind_hint(
        data.get("media_kind").and_then(|v| v.as_str()),
        &filename,
        mime.as_deref(),
    );

    let (temp_path, final_path) = match media_file_paths(&file_id, &filename) {
        Ok(paths) => paths,
        Err(_) => return,
    };

    let _ = fs::remove_file(&temp_path);
    let _ = fs::remove_file(&final_path);
    if fs::File::create(&temp_path).is_err() {
        return;
    }

    let mut state_lock = state.lock().await;
    if from != state_lock.me {
        state_lock.note_live_activity(&from, &format!("#{}", channel));
        if channel != state_lock.ch {
            state_lock.note_unread_scope(&channel);
        }
    }

    state_lock.file_transfers.insert(
        file_id.clone(),
        FileTransfer {
            filename: filename.clone(),
            size,
            from: from.clone(),
            channel: channel.clone(),
            mime: mime.clone(),
            media_kind,
            temp_path,
            final_path,
            received: 0,
            next_index: 0,
        },
    );

    let mime_hint = mime
        .as_deref()
        .map(|v| format!(" | {}", v))
        .unwrap_or_default();
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!(
            "{} {} shared '{}' ({}){} [id:{}]",
            media_kind.label(),
            from,
            filename,
            format_byte_size(size),
            mime_hint,
            file_id
        ),
        msg_type: MessageType::FileMeta,
        user: Some(from),
        channel: Some(channel),
        edited: false,
    });
}

async fn handle_file_chunk_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let file_id = data
        .get("file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if file_id.is_empty() {
        return;
    }

    let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
    let chunk_data = data.get("data").and_then(|v| v.as_str()).unwrap_or("");
    let decoded = match general_purpose::STANDARD.decode(chunk_data) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    if decoded.is_empty() {
        return;
    }

    let mut completed: Option<FileTransfer> = None;
    let mut failure_notice: Option<String> = None;
    let mut should_remove = false;
    let mut completed_transfer = false;

    {
        let mut state_lock = state.lock().await;
        {
            let Some(transfer) = state_lock.file_transfers.get_mut(&file_id) else {
                return;
            };

            if index != transfer.next_index {
                return;
            }

            if transfer.received.saturating_add(decoded.len() as u64)
                > transfer.size.saturating_add(FILE_CHUNK_BYTES as u64)
            {
                should_remove = true;
                failure_notice = Some(format!(
                    "dropped transfer '{}' due to size overflow",
                    transfer.filename
                ));
            } else {
                match fs::OpenOptions::new()
                    .append(true)
                    .open(&transfer.temp_path)
                {
                    Ok(mut temp_file) => {
                        if let Err(e) = temp_file.write_all(&decoded) {
                            should_remove = true;
                            failure_notice = Some(format!(
                                "failed to write transfer chunk for '{}': {}",
                                transfer.filename, e
                            ));
                        } else {
                            transfer.received =
                                transfer.received.saturating_add(decoded.len() as u64);
                            transfer.next_index = transfer.next_index.saturating_add(1);
                            if transfer.received >= transfer.size {
                                should_remove = true;
                                completed_transfer = true;
                            }
                        }
                    }
                    Err(e) => {
                        should_remove = true;
                        failure_notice = Some(format!(
                            "failed to append transfer chunk for '{}': {}",
                            transfer.filename, e
                        ));
                    }
                }
            }
        }

        if should_remove {
            let removed = state_lock.file_transfers.remove(&file_id);
            if completed_transfer {
                completed = removed;
            }
            if let Some(notice) = failure_notice {
                state_lock.add_notice(notice);
            }
        }
    }

    if let Some(transfer) = completed {
        if let Err(e) = fs::rename(&transfer.temp_path, &transfer.final_path) {
            let mut state_lock = state.lock().await;
            state_lock.add_notice(format!(
                "failed to finalize transfer '{}': {}",
                transfer.filename, e
            ));
            return;
        }

        let mut state_lock = state.lock().await;
        let mut base_text = format!(
            "{} saved '{}' ({}): {}",
            transfer.media_kind.label(),
            transfer.filename,
            format_byte_size(transfer.size),
            transfer.final_path.display()
        );

        if let Some(mime) = transfer.mime.as_deref() {
            base_text.push_str(&format!(" | {}", mime));
        }

        let (msg_type, text) = if transfer.media_kind == MediaKind::Image && state_lock.media_enabled {
            let image_preview = fs::read(&transfer.final_path)
                .ok()
                .map(|bytes| img_to_ascii(&bytes, MEDIA_PREVIEW_WIDTH));
            if let Some(preview) = image_preview {
                (MessageType::Img, format!("{}\n{}", base_text, preview))
            } else {
                (MessageType::FileMeta, base_text)
            }
        } else if transfer.media_kind == MediaKind::Audio {
            let audio_info = if state_lock.media_enabled {
                format!("{} | {}", format_byte_size(transfer.size), transfer.filename)
            } else {
                format!("[AUDIO] {} from {}", format_byte_size(transfer.size), transfer.from)
            };
            (MessageType::Audio, audio_info)
        } else if transfer.media_kind == MediaKind::Video && state_lock.media_enabled {
            let video_preview = fs::read(&transfer.final_path)
                .ok()
                .map(|bytes| img_to_ascii(&bytes, MEDIA_PREVIEW_WIDTH));
            if let Some(preview) = video_preview {
                (MessageType::Img, format!("{}\n{}", base_text, preview))
            } else {
                (MessageType::FileMeta, base_text)
            }
        } else {
            (MessageType::FileMeta, base_text)
        };

        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text,
            msg_type,
            user: Some(transfer.from),
            channel: Some(transfer.channel),
            edited: false,
        });
    }
}

async fn handle_err_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let message = data
        .get("m")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown server error");

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("server error: {}", message),
        msg_type: MessageType::Err,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_sys_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: m.to_string(),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_dm_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
    if let Some(pk) = data.get("pk").and_then(|v| v.as_str()) {
        let mut state_lock = state.lock().await;
        state_lock.observe_peer_pubkey(frm, pk, "dm_live");
    }
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let mut state_lock = state.lock().await;
    let peer = if frm == state_lock.me { to } else { frm };
    let dm_key = match state_lock.dmkey(peer) {
        Ok(key) => key,
        Err(e) => {
            state_lock.add_notice(format!("DM trust check failed for '{}': {}", peer, e));
            return;
        }
    };

    if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
        let msg_text =
            String::from_utf8(content).unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string());
        let arrow = if frm == state_lock.me { "→" } else { "←" };

        if frm != state_lock.me {
            state_lock.note_live_activity(frm, &format!("dm:{}", frm));
            let dm_scope = dm_unread_scope(frm);
            state_lock.note_unread_scope(&dm_scope);

            if state_lock.config.notifications.enabled && state_lock.config.notifications.on_dm {
                NotificationService::send(
                    &state_lock.config.notifications,
                    &format!("Direct message from {}", frm),
                    &msg_text,
                    true, // Always force DM notifications
                );
            }
        }

        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: format!("{} {} {}", frm, arrow, msg_text),
            msg_type: MessageType::Dm,
            user: Some(frm.to_string()),
            channel: None,
            edited: false,
        });
    }
}

async fn handle_users_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let users_val = data.get("users").cloned().unwrap_or(serde_json::json!([]));
    let users: Vec<serde_json::Value> = serde_json::from_value(users_val).unwrap_or_default();
    let mut names: Vec<String> = Vec::new();

    let mut state_lock = state.lock().await;
    for user in users {
        if let Some(name) = user.get("u").and_then(|v| v.as_str()) {
            names.push(name.to_string());
            if let Some(pk) = user.get("pk").and_then(|v| v.as_str()) {
                state_lock.observe_peer_pubkey(name, pk, "users_event");
            }
        }
    }
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("Online users: {}", names.join(", ")),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_joined_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let hist_val = data.get("hist").cloned().unwrap_or(serde_json::json!([]));
    let hist: Vec<serde_json::Value> = serde_json::from_value(hist_val).unwrap_or_default();

    let mut state_lock = state.lock().await;
    state_lock.ch = ch.to_string();
    state_lock.chs.insert(ch.to_string(), true);
    state_lock.clear_unread_for_channel(ch);
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("→ #{}", ch),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
    for event in hist {
        let _ = append_event_to_history(&mut state_lock, &event);
    }
}

async fn handle_history_or_search_event(state: &SharedState, data: &JsonMap, t: &str, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let ev_val = data.get("events").cloned().unwrap_or(serde_json::json!([]));
    let events: Vec<serde_json::Value> = serde_json::from_value(ev_val).unwrap_or_default();

    let mut state_lock = state.lock().await;
    let mut count = 0usize;
    for event in events {
        if append_event_to_history(&mut state_lock, &event) {
            count += 1;
        }
    }
    let scope = format_scope_label(ch);
    let summary = if t == "search" {
        let q = data.get("q").and_then(|v| v.as_str()).unwrap_or("");
        format!("Search '{}' in {}: {} event(s)", q, scope, count)
    } else if t == "replay" {
        let from_ts = data
            .get("from_ts")
            .and_then(|v| v.as_f64())
            .map(|v| format!("{:.0}", v))
            .unwrap_or_else(|| "?".to_string());
        format!("Replay from {} in {}: {} event(s)", from_ts, scope, count)
    } else {
        format!("History for {}: {} event(s)", scope, count)
    };
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: summary,
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_bridge_status_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let bridges = data.get("bridges").and_then(|v| v.as_array());
    let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut state_lock = state.lock().await;

    if count == 0 {
        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: "No bridge instances connected.".to_string(),
            msg_type: MessageType::Sys,
            user: None,
            channel: None,
            edited: false,
        });
        return;
    }

    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("Bridge status: {} instance(s) connected", count),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });

    if let Some(bridge_list) = bridges {
        for bridge in bridge_list {
            let username = bridge
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let bridge_type = bridge
                .get("bridge_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let instance_id = bridge
                .get("instance_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let uptime = bridge
                .get("uptime_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let routes = bridge
                .get("route_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let uptime_fmt = format_duration(uptime);
            let id_label = if instance_id.is_empty() {
                String::new()
            } else {
                format!(" id={}", instance_id)
            };

            state_lock.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!(
                    "  • {} ({}){} — routes={} uptime={}",
                    username, bridge_type, id_label, routes, uptime_fmt
                ),
                msg_type: MessageType::Sys,
                user: None,
                channel: None,
                edited: false,
            });
        }
    }
}

async fn handle_info_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let chs: Vec<String> =
        serde_json::from_value(data.get("chs").cloned().unwrap_or(serde_json::json!([])))
            .unwrap_or_default();
    let online = data.get("online").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("Channels: {} | Online: {}", chs.join(", "), online),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_metrics_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let msgs_sent = data.get("messages_sent").and_then(|v| v.as_u64()).unwrap_or(0);
    let msgs_recv = data.get("messages_received").and_then(|v| v.as_u64()).unwrap_or(0);
    let bytes_sent = data.get("bytes_sent").and_then(|v| v.as_u64()).unwrap_or(0);
    let bytes_recv = data.get("bytes_received").and_then(|v| v.as_u64()).unwrap_or(0);
    let errors = data.get("errors").and_then(|v| v.as_u64()).unwrap_or(0);
    let conn_accepted = data.get("connections_accepted").and_then(|v| v.as_u64()).unwrap_or(0);
    let conn_closed = data.get("connections_closed").and_then(|v| v.as_u64()).unwrap_or(0);
    let active = data.get("active_connections").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!(
            "📊 Metrics: sent={} recv={} | bytes: {} ↔ {} | errors={} | conn: {} accepted, {} closed, {} active",
            msgs_sent, msgs_recv,
            format_num(bytes_sent), format_num(bytes_recv),
            errors,
            conn_accepted, conn_closed, active
        ),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_status_update_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("?");
    let (text, emoji) = parse_status_payload(data.get("status"));

    let mut state_lock = state.lock().await;
    if user == state_lock.me {
        state_lock.status.text = text.clone();
        state_lock.status.emoji = emoji;
    }

    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("{} is now {} {}", user, emoji, text),
        msg_type: MessageType::StatusUpdate,
        user: Some(user.to_string()),
        channel: None,
        edited: false,
    });
}

async fn handle_typing_event(state: &SharedState, data: &JsonMap) {
    let typing = data.get("typing").and_then(|v| v.as_bool()).unwrap_or(true);
    let mut state_lock = state.lock().await;

    if let Some(user) = data.get("u").and_then(|v| v.as_str()) {
        if user != state_lock.me {
            let ch =
                normalize_channel(data.get("ch").and_then(|v| v.as_str()).unwrap_or("general"))
                    .unwrap_or_else(|| "general".to_string());
            state_lock.set_typing_presence(user, &format!("#{}", ch), typing);
            state_lock.refresh_dashboard();
        }
        return;
    }

    if let Some(from) = data.get("from").and_then(|v| v.as_str()) {
        if from != state_lock.me {
            state_lock.set_typing_presence(from, &format!("dm:{}", from.to_lowercase()), typing);
            state_lock.refresh_dashboard();
        }
    }
}

async fn handle_vdata_event(state: &SharedState, data: &JsonMap) {
    let from = data.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let payload = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
    let playback_tx = {
        let state_lock = state.lock().await;
        if from == state_lock.me {
            None
        } else {
            state_lock
                .voice_session
                .as_ref()
                .map(|session| session.event_tx.clone())
        }
    };
    if let (Some(tx), Some(frame)) = (playback_tx, decode_voice_frame(payload)) {
        let _ = tx.send(VoiceEvent::Playback(frame));
    }
}

async fn handle_vusers_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let room = data.get("room").and_then(|v| v.as_str()).unwrap_or("");
    let members: Vec<VoiceMemberInfo> = data
        .get("members")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let joined = data.get("joined").and_then(|v| v.as_bool()).unwrap_or(false);
    
    let mut state_lock = state.lock().await;
    
    if !room.is_empty() {
        state_lock.voice_members = members.iter().map(|m| m.user.clone()).collect();
    }
    
    let member_list: Vec<String> = members.iter().map(|m| {
        let status = if m.muted { "🔇" } else if m.deafened { "🔊" } else { "🎤" };
        format!("{}{}", status, m.user)
    }).collect();
    
    if !member_list.is_empty() {
        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: if joined {
                format!("🎙 Voice members: {}", member_list.join(", "))
            } else {
                format!("🎙 Voice updated: {}", member_list.join(", "))
            },
            msg_type: MessageType::Sys,
            user: None,
            channel: None,
            edited: false,
        });
    }
}

async fn handle_vstate_event(state: &SharedState, data: &JsonMap) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let muted = data.get("muted").and_then(|v| v.as_bool());
    let deafened = data.get("deafened").and_then(|v| v.as_bool());
    
    let mut state_lock = state.lock().await;
    if user == state_lock.me {
        if let Some(m) = muted {
            state_lock.voice_muted = m;
        }
        if let Some(d) = deafened {
            state_lock.voice_deafened = d;
        }
    }
    
    if let Some(m) = muted {
        let _ = state_lock.voice_session.as_ref().map(|s| {
            let _ = s.event_tx.send(VoiceEvent::MuteState(m));
        });
    }
}

async fn handle_vspeaking_event(state: &SharedState, data: &JsonMap) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let speaking = data.get("speaking").and_then(|v| v.as_bool()).unwrap_or(false);
    
    let mut state_lock = state.lock().await;
    if user == state_lock.me {
        state_lock.voice_speaking = speaking;
        let _ = state_lock.voice_session.as_ref().map(|s| {
            let _ = s.event_tx.send(VoiceEvent::SpeakingState(speaking));
        });
    }
}

async fn handle_vjoin_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("");
    
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("🎙 {} joined voice", user),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn handle_vleave_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("");
    
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("🎙 {} left voice", user),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
        edited: false,
    });
}

async fn dispatch_incoming_event(state: &SharedState, data: &JsonMap) {
    let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
    let ts = data.get("ts").map(ts_value).unwrap_or(0);
    match t {
        "msg" => handle_msg_event(state, data, ts).await,
        "img" => handle_img_event(state, data, ts).await,
        "file_meta" => handle_file_meta_event(state, data, ts).await,
        "file_chunk" => handle_file_chunk_event(state, data, ts).await,
        "sys" => handle_sys_event(state, data, ts).await,
        "err" => handle_err_event(state, data, ts).await,
        "dm" => handle_dm_event(state, data, ts).await,
        "users" => handle_users_event(state, data, ts).await,
        "joined" => handle_joined_event(state, data, ts).await,
        "history" | "search" | "replay" => handle_history_or_search_event(state, data, t, ts).await,
        "info" => handle_info_event(state, data, ts).await,
        "metrics" => handle_metrics_event(state, data, ts).await,
        "status_update" => handle_status_update_event(state, data, ts).await,
        "bridge_status" => handle_bridge_status_event(state, data, ts).await,
        "typing" => handle_typing_event(state, data).await,
        "vdata" => handle_vdata_event(state, data).await,
        "vusers" => handle_vusers_event(state, data, ts).await,
        "vstate" => handle_vstate_event(state, data).await,
        "vspeaking" => handle_vspeaking_event(state, data).await,
        "vjoin" => handle_vjoin_event(state, data, ts).await,
        "vleave" => handle_vleave_event(state, data, ts).await,
        _ => {}
    }
}

fn encode_voice_frame(frame: &VoiceFrame) -> String {
    let mut out = Vec::with_capacity(6 + frame.samples.len() * 2);
    out.extend_from_slice(&frame.sample_rate.to_le_bytes());
    out.extend_from_slice(&frame.channels.to_le_bytes());
    for s in &frame.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    general_purpose::STANDARD.encode(out)
}

fn decode_voice_frame(payload_b64: &str) -> Option<VoiceFrame> {
    let raw = general_purpose::STANDARD.decode(payload_b64).ok()?;
    if raw.len() < 6 {
        return None;
    }
    let sample_rate = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let channels = u16::from_le_bytes([raw[4], raw[5]]);
    if channels == 0 {
        return None;
    }
    let samples_raw = &raw[6..];
    if samples_raw.len() % 2 != 0 {
        return None;
    }
    let mut samples = Vec::with_capacity(samples_raw.len() / 2);
    for chunk in samples_raw.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Some(VoiceFrame {
        sample_rate,
        channels,
        samples,
    })
}

fn push_pcm_to_chunks(
    pending: &Arc<Mutex<VecDeque<i16>>>,
    pcm: &[i16],
    chunk_samples: usize,
    sample_rate: u32,
    channels: u16,
    tx: &std_mpsc::Sender<VoiceEvent>,
) {
    if chunk_samples == 0 {
        return;
    }
    if let Ok(mut q) = pending.lock() {
        for sample in pcm {
            q.push_back(*sample);
        }
        while q.len() >= chunk_samples {
            let mut chunk = Vec::with_capacity(chunk_samples);
            for _ in 0..chunk_samples {
                if let Some(v) = q.pop_front() {
                    chunk.push(v);
                }
            }
            let _ = tx.send(VoiceEvent::Captured(VoiceFrame {
                sample_rate,
                channels,
                samples: chunk,
            }));
        }
    }
}

fn build_input_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    tx: std_mpsc::Sender<VoiceEvent>,
    sample_rate: u32,
    channels: u16,
    chunk_samples: usize,
) -> ChatifyResult<Stream> {
    let pending = Arc::new(Mutex::new(VecDeque::<i16>::new()));
    let err_fn = |e| warn!("voice input error: {}", e);
    match format {
        SampleFormat::I16 => {
            let pending_i16 = pending.clone();
            let tx_i16 = tx.clone();
            device
                .build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        push_pcm_to_chunks(
                            &pending_i16,
                            data,
                            chunk_samples,
                            sample_rate,
                            channels,
                            &tx_i16,
                        )
                    },
                    err_fn,
                )
                .map_err(|e| {
                    ChatifyError::Audio(format!("failed to build i16 input stream: {}", e))
                })
        }
        SampleFormat::U16 => {
            let pending_u16 = pending.clone();
            let tx_u16 = tx.clone();
            device
                .build_input_stream(
                    config,
                    move |data: &[u16], _| {
                        let converted: Vec<i16> = data
                            .iter()
                            .map(|v| {
                                (*v as i32 - 32768).clamp(i16::MIN as i32, i16::MAX as i32) as i16
                            })
                            .collect();
                        push_pcm_to_chunks(
                            &pending_u16,
                            &converted,
                            chunk_samples,
                            sample_rate,
                            channels,
                            &tx_u16,
                        )
                    },
                    err_fn,
                )
                .map_err(|e| {
                    ChatifyError::Audio(format!("failed to build u16 input stream: {}", e))
                })
        }
        SampleFormat::F32 => {
            let pending_f32 = pending;
            device
                .build_input_stream(
                    config,
                    move |data: &[f32], _| {
                        let converted: Vec<i16> = data
                            .iter()
                            .map(|v| {
                                let clamped = v.clamp(-1.0, 1.0);
                                (clamped * i16::MAX as f32) as i16
                            })
                            .collect();
                        push_pcm_to_chunks(
                            &pending_f32,
                            &converted,
                            chunk_samples,
                            sample_rate,
                            channels,
                            &tx,
                        )
                    },
                    err_fn,
                )
                .map_err(|e| {
                    ChatifyError::Audio(format!("failed to build f32 input stream: {}", e))
                })
        }
    }
}

fn start_voice_session(
    room: String,
    ws_tx: mpsc::UnboundedSender<String>,
) -> ChatifyResult<VoiceSession> {
    let (event_tx, event_rx) = std_mpsc::channel::<VoiceEvent>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<ChatifyResult<()>>();
    let ws_tx_task = ws_tx;
    let room_task = room.clone();
    let event_tx_thread = event_tx.clone();

    let task = thread::spawn(move || {
        let host = cpal::default_host();
        let input_device = match host.default_input_device() {
            Some(d) => d,
            None => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(
                    "no default input device available".to_string(),
                )));
                return;
            }
        };
        let input_supported = match input_device.default_input_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "failed to fetch default input config: {}",
                    e
                ))));
                return;
            }
        };

        let input_format = input_supported.sample_format();
        let input_config: StreamConfig = input_supported.into();
        let sample_rate = input_config.sample_rate.0;
        let channels = input_config.channels;
        let chunk_samples = 320usize
            .saturating_mul(channels as usize)
            .max(channels as usize);

        let (_output_stream, output_handle) = match OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "audio output init failed: {}",
                    e
                ))));
                return;
            }
        };
        let sink = match Sink::try_new(&output_handle) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                    "audio output sink init failed: {}",
                    e
                ))));
                return;
            }
        };

        let input_stream = match build_input_stream(
            &input_device,
            &input_config,
            input_format,
            event_tx_thread,
            sample_rate,
            channels,
            chunk_samples,
        ) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        if let Err(e) = input_stream.play() {
            let _ = ready_tx.send(Err(ChatifyError::Audio(format!(
                "failed to start microphone stream: {}",
                e
            ))));
            return;
        }
        let _ = ready_tx.send(Ok(()));

        while let Ok(event) = event_rx.recv() {
            match event {
                VoiceEvent::Captured(frame) => {
                    let payload = encode_voice_frame(&frame);
                    enqueue_timed(
                        &ws_tx_task,
                        serde_json::json!({
                            "t": "vdata",
                            "r": room_task.clone(),
                            "a": payload
                        }),
                    );
                }
                VoiceEvent::Playback(frame) => {
                    sink.append(SamplesBuffer::new(
                        frame.channels,
                        frame.sample_rate,
                        frame.samples,
                    ));
                }
                VoiceEvent::MuteState(_) | VoiceEvent::SpeakingState(_) => {}
                VoiceEvent::Stop => break,
            }
        }
        sink.stop();
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(VoiceSession {
            room,
            event_tx,
            task,
        }),
        Ok(Err(e)) => {
            let _ = task.join();
            Err(e)
        }
        Err(_) => {
            let _ = task.join();
            Err(ChatifyError::Audio(
                "voice runtime failed to initialize".to_string(),
            ))
        }
    }
}

fn stop_voice_session(state: &mut ClientState, feedback: SessionStopFeedback) {
    if let Some(session) = state.voice_session.take() {
        let room = session.room.clone();
        enqueue_timed(&state.ws_tx, serde_json::json!({"t": "vleave", "r": room}));
        let _ = session.event_tx.send(VoiceEvent::Stop);
        let _ = session.task.join();
        state.voice_active = false;
        if matches!(feedback, SessionStopFeedback::Notify) {
            state.add_notice("Voice session stopped");
        }
    } else {
        state.voice_active = false;
    }
}

fn queue_channel_message(state: &mut ClientState, channel: &str, plaintext: &str) {
    let plaintext = maybe_expand_emoji(state, plaintext);

    enqueue_typing_state(
        &state.ws_tx,
        &TypingScope::Channel(channel.to_string()),
        false,
    );
    let encrypted = match enc_bytes(&state.ckey(channel), plaintext.as_bytes()) {
        Ok(v) => v,
        Err(e) => {
            state.add_notice(format!("encryption failed: {}", e));
            return;
        }
    };
    let encoded = general_purpose::STANDARD.encode(&encrypted);
    state.record_sent(channel, &plaintext);
    enqueue_timed(
        &state.ws_tx,
        serde_json::json!({"t": "msg", "ch": channel, "c": encoded, "p": plaintext}),
    );
}

fn handle_slash_command(
    state: &mut ClientState,
    cmd: &str,
    args: &str,
    shutdown_tx: &watch::Sender<bool>,
) -> CommandFlow {
    let original_cmd = cmd.trim();
    let mut effective_cmd = canonical_command(original_cmd);
    if find_command_spec(&effective_cmd).is_none() {
        if let Some(auto) = unique_autocomplete_command(&effective_cmd) {
            state.add_notice(format!("Auto-completed '{}' -> '{}'.", original_cmd, auto));
            effective_cmd = auto.to_string();
        }
    }

    match effective_cmd.as_str() {
        "/join" | "/switch" => {
            if let Some(ch) = normalize_channel(args.trim()) {
                let previous_channel = state.ch.clone();
                if previous_channel != ch {
                    enqueue_typing_state(
                        &state.ws_tx,
                        &TypingScope::Channel(previous_channel),
                        false,
                    );
                }
                state.ch = ch.clone();
                state.chs.insert(ch.clone(), true);
                state.clear_unread_for_channel(&ch);
                enqueue_timed(&state.ws_tx, serde_json::json!({"t": "join", "ch": ch}));
            } else {
                state.add_notice(
                    "Usage: /join <channel> (or /switch <channel>). Allowed: letters, digits, -, _",
                );
            }
        }
        "/dm" => {
            let mut parts = args.splitn(2, ' ');
            let target = parts.next().unwrap_or("");
            let msg = parts.next().unwrap_or("");
            if target.is_empty() || msg.is_empty() {
                state.add_notice("Usage: /dm <user> <message>");
                return CommandFlow::Continue;
            }
            if target == state.me {
                state.add_notice("Cannot DM yourself.");
                return CommandFlow::Continue;
            }

            if state.trust_store.state_for(target) == TrustState::Unknown {
                state.add_notice(format!(
                    "Trust for '{}' is unknown. Use /fingerprint {} and /trust {} <fingerprint>.",
                    target, target, target
                ));
            }

            let outbound_msg = maybe_expand_emoji(state, msg);

            let encrypted = match state.dmkey(target) {
                Ok(key) => match enc_bytes(&key, outbound_msg.as_bytes()) {
                    Ok(v) => v,
                    Err(e) => {
                        state.add_notice(format!("encryption failed: {}", e));
                        return CommandFlow::Continue;
                    }
                },
                Err(e) => {
                    state.add_notice(format!("DM failed: {}", e));
                    return CommandFlow::Continue;
                }
            };
            let encoded = general_purpose::STANDARD.encode(&encrypted);
            state.record_sent(&format!("dm:{}", target), &outbound_msg);
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "dm", "to": target, "c": encoded, "p": outbound_msg}),
            );
            enqueue_typing_state(&state.ws_tx, &TypingScope::Dm(target.to_string()), false);
            state.clear_unread_for_dm(target);
        }
        "/image" => {
            if args.trim().is_empty() {
                state.add_notice("Usage: /image <path>");
                return CommandFlow::Continue;
            }
            if let Err(e) = queue_media_upload(state, args.trim(), MediaKind::Image) {
                state.add_notice(format!("Image upload failed: {}", e));
            }
        }
        "/video" => {
            if args.trim().is_empty() {
                state.add_notice("Usage: /video <path>");
                return CommandFlow::Continue;
            }
            if let Err(e) = queue_media_upload(state, args.trim(), MediaKind::Video) {
                state.add_notice(format!("Video upload failed: {}", e));
            }
        }
        "/typing" => {
            let mut parts = args.split_whitespace();
            let mode = parts.next().unwrap_or("on").to_lowercase();
            let scope_raw = parts.next().unwrap_or("");
            if parts.next().is_some() {
                state.add_notice("Usage: /typing [on|off] [#channel|dm:user]");
                return CommandFlow::Continue;
            }

            let typing_enabled = match mode.as_str() {
                "on" | "start" => true,
                "off" | "stop" => false,
                _ => {
                    state.add_notice("Usage: /typing [on|off] [#channel|dm:user]");
                    return CommandFlow::Continue;
                }
            };

            let Some(scope) = parse_typing_scope(scope_raw, &state.ch) else {
                state.add_notice("Usage: /typing [on|off] [#channel|dm:user]");
                return CommandFlow::Continue;
            };

            enqueue_typing_state(&state.ws_tx, &scope, typing_enabled);
            state.add_notice(format!(
                "Typing {} for {}.",
                if typing_enabled { "ON" } else { "OFF" },
                scope.label()
            ));
        }
        "/status" => {
            let trimmed = args.trim();
            if trimmed.is_empty() {
                state.add_notice(format!(
                    "Current status: {} {} ({})",
                    state.status.emoji,
                    state.status.text,
                    state.status.presence.as_str()
                ));
                state.add_notice(format!("Avatar: {}", state.status.avatar));
                state.add_notice(format!(
                    "Bio: {}",
                    if state.status.bio.trim().is_empty() {
                        "(none)"
                    } else {
                        state.status.bio.as_str()
                    }
                ));
                state.add_notice("Usage: /status [online|away|busy|invisible] [text]");
                return CommandFlow::Continue;
            }

            let mut parts = trimmed.splitn(2, ' ');
            let first = parts.next().unwrap_or("");
            let tail = parts.next().unwrap_or("").trim();

            if let Some(presence) = PresenceState::from_token(first) {
                state.status.presence = presence;
                state.status.emoji = presence.indicator();
                if !tail.is_empty() {
                    let expanded = maybe_expand_emoji(state, tail);
                    state.status.text = expanded;
                }
            } else {
                let expanded = maybe_expand_emoji(state, trimmed);
                state.status.text = expanded;
            }

            let status_text = state.status.text.clone();
            let status_emoji = state.status.emoji.to_string();

            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({
                    "t": "status",
                    "status": {
                        "text": status_text,
                        "emoji": status_emoji
                    }
                }),
            );
            state.add_notice(format!(
                "Status updated: {} {} ({})",
                state.status.emoji,
                state.status.text,
                state.status.presence.as_str()
            ));
            state.refresh_dashboard();
        }
        "/theme" => {
            let requested = args.trim();
            if requested.is_empty() || requested.eq_ignore_ascii_case("list") {
                let (builtin, custom) = list_available_themes(state);
                state.add_notice(format!("Current theme: {}", state.theme.name));
                state.add_notice(format!("Built-in themes: {}", builtin.join(", ")));
                if custom.is_empty() {
                    state.add_notice("Custom themes: (none)");
                } else {
                    state.add_notice(format!("Custom themes: {}", custom.join(", ")));
                }
                state.add_notice("Usage: /theme <name> | /theme list");
                return CommandFlow::Continue;
            }

            if !theme_name_exists(state, requested) {
                let requested_lower = requested.to_lowercase();
                let (builtin, custom) = list_available_themes(state);
                let mut suggestions: Vec<String> = builtin
                    .into_iter()
                    .chain(custom)
                    .filter(|name| name.to_lowercase().contains(&requested_lower))
                    .take(6)
                    .collect();
                suggestions.sort_unstable();

                if suggestions.is_empty() {
                    state.add_notice(format!(
                        "Unknown theme '{}'. Use /theme list to see available names.",
                        requested
                    ));
                } else {
                    state.add_notice(format!(
                        "Unknown theme '{}'. Did you mean: {}",
                        requested,
                        suggestions.join(", ")
                    ));
                }
                return CommandFlow::Continue;
            }

            state.config.ui.theme = requested.to_string();
            state.theme = state.config.resolve_theme();
            match state.config.save() {
                Ok(()) => state.add_notice(format!("Theme switched to '{}'.", state.theme.name)),
                Err(e) => state.add_notice(format!(
                    "Theme switched to '{}' (save failed: {}).",
                    state.theme.name, e
                )),
            }
            state.refresh_dashboard();
        }
        "/emoji" => {
            let trimmed = args.trim();
            if trimmed.is_empty() {
                state.add_notice(format!(
                    "Emoji shortcodes are {}.",
                    if state.config.ui.enable_emoji {
                        "ENABLED"
                    } else {
                        "DISABLED"
                    }
                ));
                state.add_notice("Usage: /emoji [on|off|list [query]]");
                return CommandFlow::Continue;
            }

            let mut parts = trimmed.splitn(2, ' ');
            let sub = parts.next().unwrap_or("").to_lowercase();
            let tail = parts.next().unwrap_or("").trim();

            match sub.as_str() {
                "on" => {
                    state.config.ui.enable_emoji = true;
                    match state.config.save() {
                        Ok(()) => state.add_notice("Emoji shortcode expansion enabled."),
                        Err(e) => state
                            .add_notice(format!("Emoji enabled, but failed to save config: {}", e)),
                    }
                }
                "off" => {
                    state.config.ui.enable_emoji = false;
                    match state.config.save() {
                        Ok(()) => state.add_notice("Emoji shortcode expansion disabled."),
                        Err(e) => state.add_notice(format!(
                            "Emoji disabled, but failed to save config: {}",
                            e
                        )),
                    }
                }
                "list" => {
                    let entries = if tail.is_empty() {
                        emoji::emoji_list()
                    } else {
                        emoji::search_emoji(tail)
                    };

                    if entries.is_empty() {
                        state.add_notice("No emoji shortcodes matched your query.");
                        return CommandFlow::Continue;
                    }

                    let shown = entries.len().min(24);
                    if tail.is_empty() {
                        state.add_notice(format!(
                            "Emoji shortcodes: showing {} of {}",
                            shown,
                            entries.len()
                        ));
                    } else {
                        state.add_notice(format!(
                            "Emoji search '{}': showing {} of {}",
                            tail,
                            shown,
                            entries.len()
                        ));
                    }
                    for (shortcode, emoji_char) in entries.iter().take(shown) {
                        state.add_notice(format!("{} {}", shortcode, emoji_char));
                    }
                    if entries.len() > shown {
                        state.add_notice(format!("... and {} more", entries.len() - shown));
                    }
                }
                _ => {
                    state.add_notice("Usage: /emoji [on|off|list [query]]");
                }
            }
        }
        "/fingerprint" => {
            let target = args.trim();
            if target.is_empty() {
                let mut peers: Vec<String> = state.trust_store.peers.keys().cloned().collect();
                peers.sort_unstable();
                if peers.is_empty() {
                    state.add_notice("No peer fingerprints observed yet.");
                } else {
                    state.add_notice(format!("Known fingerprints: {} peer(s)", peers.len()));
                    for peer in peers.iter().take(20) {
                        if let Some(line) = state.trust_record_line(peer) {
                            state.add_notice(line);
                        }
                    }
                    if peers.len() > 20 {
                        state.add_notice(format!("... and {} more", peers.len() - 20));
                    }
                }
                return CommandFlow::Continue;
            }

            if !is_valid_username_token(target) {
                state.add_notice("Usage: /fingerprint [user]");
                return CommandFlow::Continue;
            }

            if let Some(pk) = state.users.get(target).cloned() {
                state.observe_peer_pubkey(target, &pk, "fingerprint_cmd");
            }

            if let Some(line) = state.trust_record_line(target) {
                state.add_notice(line);
            } else {
                state.add_notice(format!(
                    "No fingerprint known for '{}'. Request /users or receive a DM first.",
                    target
                ));
            }
        }
        "/trust" => {
            let mut parts = args.split_whitespace();
            let user = parts.next().unwrap_or("");
            let fingerprint = parts.next().unwrap_or("");
            if user.is_empty() || fingerprint.is_empty() || parts.next().is_some() {
                state.add_notice("Usage: /trust <user> <fingerprint>");
                return CommandFlow::Continue;
            }
            if !is_valid_username_token(user) {
                state.add_notice("Usage: /trust <user> <fingerprint>");
                return CommandFlow::Continue;
            }

            if let Some(pk) = state.users.get(user).cloned() {
                state.observe_peer_pubkey(user, &pk, "trust_cmd");
            }

            match state.trust_peer(user, fingerprint) {
                Ok(msg) => state.add_notice(msg),
                Err(e) => state.add_notice(format!("Trust failed: {}", e)),
            }
        }
        "/me" => {
            let action = args.trim();
            if action.is_empty() {
                state.add_notice(format!(
                    "You are '{}' [{}] on #{} (voice: {}).",
                    state.me,
                    state.client_id,
                    state.ch,
                    if state.voice_active { "on" } else { "off" }
                ));
            } else {
                let ch = state.ch.clone();
                let msg = format!("* {} {}", state.me, action);
                queue_channel_message(state, &ch, &msg);
            }
        }
        "/users" => {
            enqueue_timed(&state.ws_tx, serde_json::json!({"t": "users"}));
        }
        "/channels" => {
            enqueue_timed(&state.ws_tx, serde_json::json!({"t": "info"}));
        }
        #[cfg(feature = "bridge-client")]
        "/bridge" => {
            let sub = args.trim().to_lowercase();
            match sub.as_str() {
                "status" | "" => {
                    enqueue_timed(&state.ws_tx, serde_json::json!({"t": "bridge_status"}));
                    state.add_notice("Requesting bridge status from server...");
                }
                _ => {
                    state.add_notice("Usage: /bridge [status]");
                }
            }
        }
        "/screen" => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            let first = parts.first().copied();
            match first {
                Some("list") | Some("sources") => {
                    state.add_notice("Fetching available screen sources...");
                    enqueue_timed(&state.ws_tx, serde_json::json!({"t": "ss_list_sources"}));
                }
                Some("start") => {
                    let mut options = ScreenShareOptions::default();
                    let mut has_audio = false;
                    for part in parts.iter().skip(1) {
                        match *part {
                            "--audio" | "-a" => has_audio = true,
                            "--no-audio" => has_audio = false,
                            "low" => options.quality = QualityPreset::Low,
                            "medium" | "med" => options.quality = QualityPreset::Medium,
                            "high" => options.quality = QualityPreset::High,
                            _ if part.starts_with("source:") => {
                                options.source_id = Some(part.strip_prefix("source:").unwrap().to_string());
                            }
                            _ => {}
                        }
                    }
                    options.audio = has_audio;

                    if !state.voice_active {
                        state.add_notice("Join a voice room first with /voice before screen sharing.");
                        return CommandFlow::Continue;
                    }

                    state.add_notice(format!("Starting screen share (quality: {:?}, audio: {})...", options.quality, options.audio));
                    enqueue_timed(&state.ws_tx, serde_json::json!({
                        "t": "ss_start",
                        "q": options.quality,
                        "a": options.audio,
                        "s": options.source_id
                    }));
                }
                Some("stop") => {
                    state.add_notice("Stopping screen share...");
                    enqueue_timed(&state.ws_tx, serde_json::json!({"t": "ss_stop"}));
                }
                Some("quality") => {
                    if let Some(quality) = parts.get(1) {
                        state.add_notice(format!("Quality preset: {}", quality));
                    } else {
                        state.add_notice("Usage: /screen quality [low|medium|high]");
                    }
                }
                Some("status") => {
                    if let Some(ref ss) = state.screen_share {
                        let sess = ss.session();
                        if let Some(s) = sess {
                            state.add_notice(format!(
                                "Screen sharing: {:?} | Room: {} | {} viewers",
                                s.state, s.room, s.viewer_count
                            ));
                        } else {
                            state.add_notice("No active screen share");
                        }
                    } else {
                        state.add_notice("No active screen share");
                    }
                }
                None | Some("") | Some("help") => {
                    state.add_notice("Screen sharing commands:");
                    state.add_notice("  /screen list        - List available sources");
                    state.add_notice("  /screen start      - Start screen sharing (in voice room)");
                    state.add_notice("  /screen start --audio  - Start with audio");
                    state.add_notice("  /screen stop       - Stop screen sharing");
                    state.add_notice("  /screen quality [low|medium|high]");
                }
                _ => {
                    state.add_notice("Usage: /screen [start|stop|list|quality|status]");
                }
            }
        }
        "/voice" => {
            if state.voice_session.is_some() {
                stop_voice_session(state, SessionStopFeedback::Notify);
            } else {
                let room = normalize_channel(args.trim()).unwrap_or_else(|| state.ch.clone());
                let ws_tx = state.ws_tx.clone();
                match start_voice_session(room.clone(), ws_tx.clone()) {
                    Ok(session) => {
                        state.voice_active = true;
                        state.voice_session = Some(session);
                        enqueue_timed(&ws_tx, serde_json::json!({"t": "vjoin", "r": room}));
                        state.add_notice(format!("Voice started in #{}", room));
                    }
                    Err(e) => {
                        error!("Voice start failed: {}", e);
                        info!(
                            "Check that your microphone/speakers are available and not in use by another app."
                        );
                        state.add_notice(format!("Voice start failed: {}", e));
                    }
                }
            }
        }
        "/clear" => {
            state.message_history.clear();
            state.unread_counts.clear();
            state.unread_separator_scopes.clear();
            state.activity_hint = None;
            state.typing_presence.clear();
            state.refresh_dashboard();
        }
        "/commands" => {
            let lines = command_palette_lines(args);
            state.add_notice_batch(lines);
        }
        "/help" => {
            let requested = args.trim();
            if requested.is_empty() {
                state.add_notice(HELP_TEXT);
                state.add_notice("Tip: /commands for full palette or /help <command> for details.");
            } else {
                let lines = command_help_lines(requested);
                state.add_notice_batch(lines);
            }
        }
        "/metrics" => {
            state.add_notice("Fetching server metrics...");
            enqueue_timed(&state.ws_tx, serde_json::json!({"t": "metrics"}));
        }
        "/edit" => {
            let trimmed = args.trim();
            if trimmed.is_empty() {
                state.add_notice("Usage: /edit <new text>  — edit your last message");
                state.add_notice("       /edit #N <new text> — edit your Nth most recent message");
                return CommandFlow::Continue;
            }

            let ch = state.ch.clone();

            // Parse optional index prefix: "#3 new text"
            let (index, new_text) = if let Some(rest) = trimmed.strip_prefix('#') {
                let mut parts = rest.splitn(2, ' ');
                let num_str = parts.next().unwrap_or("");
                let text = parts.next().unwrap_or("").trim();
                match num_str.parse::<usize>() {
                    Ok(n) if n >= 1 => (n, text),
                    _ => {
                        state.add_notice("Invalid index. Use /edit #N <text> where N >= 1.");
                        return CommandFlow::Continue;
                    }
                }
            } else {
                (1, trimmed)
            };

            if new_text.is_empty() {
                state.add_notice("New text cannot be empty.");
                return CommandFlow::Continue;
            }

            let expanded_new_text = maybe_expand_emoji(state, new_text);

            let old_text = match state.find_recent_sent(&ch, index) {
                Some(sent) => sent.plaintext.clone(),
                None => {
                    if index == 1 {
                        state.add_notice("No recent messages to edit in this channel.");
                    } else {
                        state.add_notice(format!(
                            "No message #{} found in this channel (have {} recent).",
                            index,
                            state
                                .recent_sents
                                .iter()
                                .filter(|m| m.channel == ch)
                                .count()
                        ));
                    }
                    return CommandFlow::Continue;
                }
            };

            // Send edit to server.
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({
                    "t": "edit",
                    "ch": ch,
                    "old_text": old_text,
                    "new_text": expanded_new_text
                }),
            );

            // Update local feed optimistically.
            let me = state.me.clone();
            if state.update_message_in_feed(&me, &ch, &old_text, &expanded_new_text) {
                state.add_notice(format!("Edited message #{}.", index));
            }
            state.refresh_dashboard();
        }
        "/history" => {
            let (scope, window_secs) = match parse_history_args(args, &state.ch) {
                Ok(v) => v,
                Err(msg) => {
                    state.add_notice(msg);
                    state.add_notice("Examples: /history #general 24h, /history dm:alice 7d");
                    return CommandFlow::Continue;
                }
            };

            let mut payload = serde_json::json!({"t": "history", "ch": scope, "limit": 500});
            if let Some(seconds) = window_secs {
                payload["seconds"] = serde_json::json!(seconds);
            }
            enqueue_timed(&state.ws_tx, payload);
        }
        "/search" => {
            let q = args.trim();
            if q.is_empty() {
                state.add_notice("Usage: /search <query>");
                return CommandFlow::Continue;
            }
            let ch = state.ch.clone();
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "search", "ch": ch, "q": q, "limit": 50}),
            );
        }
        "/replay" => {
            let spec = args.trim();
            if spec.is_empty() {
                state.add_notice("Usage: /replay <timestamp>");
                state.add_notice(
                    "Timestamp formats: unix seconds, RFC3339, or 'YYYY-MM-DD HH:MM:SS'",
                );
                return CommandFlow::Continue;
            }

            let Some(from_ts) = parse_replay_timestamp(spec) else {
                state.add_notice(
                    "Invalid timestamp. Use unix seconds, RFC3339, or 'YYYY-MM-DD HH:MM:SS'.",
                );
                return CommandFlow::Continue;
            };

            let ch = state.ch.clone();
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "replay", "ch": ch, "from_ts": from_ts, "limit": 1000}),
            );
        }
        "/rewind" => {
            let mut p = args.split_whitespace();
            let window = p.next().unwrap_or("");
            if window.is_empty() {
                state.add_notice("Usage: /rewind <Ns|Nm|Nh|Nd> [limit]");
                return CommandFlow::Continue;
            }
            let Some(seconds) = parse_duration_secs(window) else {
                state.add_notice("Invalid duration. Use formats like 90s, 15m, 2h, 1d.");
                return CommandFlow::Continue;
            };
            let limit = parse_limit(p.next().unwrap_or(""), 100, 500);
            let ch = state.ch.clone();
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "rewind", "ch": ch, "seconds": seconds, "limit": limit}),
            );
        }
        "/quit" => {
            enqueue_typing_state(&state.ws_tx, &TypingScope::Channel(state.ch.clone()), false);
            stop_voice_session(state, SessionStopFeedback::Silent);
            state.running = false;
            let _ = shutdown_tx.send(true);
            return CommandFlow::Exit;
        }
        _ => {
            let suggestions = suggest_commands(original_cmd);
            if suggestions.is_empty() {
                state.add_notice(format!(
                    "Unknown command: {}. Use /commands to browse available commands.",
                    original_cmd
                ));
            } else {
                state.add_notice(format!(
                    "Unknown command: {}. Did you mean: {}",
                    original_cmd,
                    suggestions.join(", ")
                ));
            }
        }
    }

    CommandFlow::Continue
}

/// Main entry point for the WebSocket client
#[tokio::main]
async fn main() -> ChatifyResult<()> {
    // Load configuration from file (with defaults if not found)
    let config = Config::load();
    NotificationService::init();

    // Parse command line arguments (will override config values)
    let matches = clap::Command::new("chatify-client")
        .version("1.0")
        .author("Chatify Team")
        .about("WebSocket chat client with encryption")
        .arg(
            clap::Arg::new("host")
                .long("host")
                .value_name("HOST")
                .help("Server host (overrides config)"),
        )
        .arg(
            clap::Arg::new("port")
                .long("port")
                .value_name("PORT")
                .help("Server port (overrides config)"),
        )
        .arg(
            clap::Arg::new("tls")
                .long("tls")
                .action(clap::ArgAction::SetTrue)
                .help("Use TLS for secure connection (overrides config)"),
        )
        .arg(
            clap::Arg::new("log")
                .long("log")
                .action(clap::ArgAction::SetTrue)
                .help("Enable debug logging"),
        )
        .arg(
            clap::Arg::new("no-markdown")
                .long("no-markdown")
                .action(clap::ArgAction::SetTrue)
                .help("Disable markdown rendering"),
        )
        .arg(
            clap::Arg::new("no-media")
                .long("no-media")
                .action(clap::ArgAction::SetTrue)
                .help("Disable rich media rendering (images, audio)"),
        )
        .get_matches();

    // Merge config with CLI args (CLI takes precedence)
    let host = matches
        .get_one::<String>("host")
        .map(|s| s.as_str())
        .unwrap_or(&config.connection.default_host);
    let port = matches
        .get_one::<String>("port")
        .and_then(|s| s.parse().ok())
        .unwrap_or(config.connection.default_port);
    let tls = matches.get_flag("tls") || config.connection.use_tls;
    let log_enabled = matches.get_flag("log");
    let markdown_enabled = !matches.get_flag("no-markdown") && config.ui.enable_markdown;
    let media_enabled = !matches.get_flag("no-media") && config.ui.enable_media;

    let scheme = if tls { "wss" } else { "ws" };
    let uri = format!("{}://{}:{}", scheme, host, port);

    // Use remembered username if available
    let username = if config.session.remember_username && !config.session.last_username.is_empty() {
        print!("username [{}]: ", config.session.last_username);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() {
            config.session.last_username.clone()
        } else {
            input.to_string()
        }
    } else {
        read_username()?
    };

    let mut password = rpassword::prompt_password("password: ")?;
    let client_priv_key = new_keypair();
    let client_pub_key =
        pub_b64(&client_priv_key).expect("generated keypair must produce valid public key");
    let client_pub_key_for_id = client_pub_key.clone();

    // Set up logging
    if log_enabled {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    // Connect to WebSocket
    let (ws_stream, _) = connect_async(&uri).await?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Hash password and immediately zeroize the plaintext
    let pw_hash = match pw_hash_client(&password) {
        Ok(hash) => hash,
        Err(e) => {
            error!("Password hashing failed: {}", e);
            password.zeroize();
            return Ok(());
        }
    };
    password.zeroize();

    // Authenticate
    let auth_msg = serde_json::json!({
        "t": "auth",
        "u": username,
        "pw": pw_hash,
        "pk": client_pub_key,
        "status": {"text": "Online", "emoji": "🟢"}
    });
    ws_tx.send(Message::Text(auth_msg.to_string())).await?;

    // Wait for auth response with timeout
    let auth_reply = timeout(Duration::from_secs(10), ws_rx.next()).await;
    let resp = match extract_auth_text(auth_reply) {
        Ok(payload) => payload,
        Err(err) => {
            error!("{}", err);
            return Ok(());
        }
    };

    {
        let auth = match parse_auth_payload(&resp, &username) {
            Ok(v) => v,
            Err(err) => {
                error!("{}", err);
                return Ok(());
            }
        };
        let AuthContext {
            me: auth_me,
            users: auth_users,
        } = auth;
        let client_id = build_client_id(&auth_me, &client_pub_key_for_id);

        // Update config with successful connection details
        let mut config_to_save = config.clone();
        config_to_save.ui.enable_markdown = markdown_enabled;

        if config_to_save.session.remember_username {
            config_to_save.session.last_username = username.clone();
        }
        if config_to_save.session.remember_channel {
            config_to_save.session.last_channel = config_to_save.session.last_channel.clone();
        }
        if let Err(e) = config_to_save.save() {
            log::warn!("Failed to save config: {}", e);
        }

        // Create an mpsc channel for outgoing messages to be queued and sent via WebSocket
        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<String>();

        // Spawn task to forward mpsc messages to WebSocket
        tokio::spawn(async move {
            let mut msg_rx = msg_rx;
            while let Some(msg) = msg_rx.recv().await {
                let _ = ws_tx.send(Message::Text(msg)).await;
            }
        });

        // We'll store the state in an Arc for sharing between tasks
        // Store the client-side hash (not the raw password) for key derivation
        // The hash was already computed before password zeroization
        let state = Arc::new(tokio::sync::Mutex::new(ClientState {
            ws_tx: msg_tx.clone(), // Use mpsc sender instead of WebSocket
            me: auth_me.clone(),
            client_id: client_id.clone(),
            pw: pw_hash,
            ch: config_to_save.session.last_channel.clone(),
            chs: HashMap::from([(config_to_save.session.last_channel.clone(), true)]),
            users: HashMap::new(),
            trust_store: TrustStore::load_default(),
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: client_priv_key,
            running: true,
            voice_active: false,
            voice_session: None,
            theme: config_to_save.resolve_theme(),
            file_transfers: HashMap::new(),
            message_history: VecDeque::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: PresenceState::Online.indicator(),
                presence: PresenceState::Online,
                bio: String::new(),
                avatar: default_avatar(),
            },
            reactions: HashMap::new(),
            unread_counts: HashMap::new(),
            unread_separator_scopes: HashSet::new(),
            activity_hint: None,
            typing_presence: HashMap::new(),
            recent_sents: VecDeque::new(),
            log_enabled,
            config: config_to_save,
            screen_share: None,
            screen_viewing: false,
            voice_members: Vec::new(),
            voice_muted: false,
            voice_deafened: false,
            voice_speaking: false,
            media_enabled,
        }));
        {
            let mut state_lock = state.lock().await;
            state_lock.me = auth_me;
            state_lock.add_notice(format!("Connected to {}", uri));
            state_lock.add_notice(format!("Client ID: {}", client_id));
            state_lock.add_notice("Use /help to list commands.");

            let mut online_users = Vec::new();
            for (name, pk) in auth_users {
                online_users.push(name.clone());
                state_lock.observe_peer_pubkey(&name, &pk, "auth_ok");
            }
            let (unknown, trusted, changed) = state_lock.trust_counts_for_users(&online_users);
            if changed > 0 {
                state_lock.add_notice(format!(
                    "SECURITY WARNING: {} online peer key(s) changed. Use /fingerprint <user>.",
                    changed
                ));
            }
            if unknown > 0 {
                state_lock.add_notice(format!(
                    "Trust pending for {} peer(s). Use /fingerprint [user] and /trust <user> <fingerprint>.",
                    unknown
                ));
            }
            if !online_users.is_empty() {
                state_lock.add_notice(format!(
                    "Trust summary: trusted={}, unknown={}, changed={}",
                    trusted, unknown, changed
                ));
            }
        }

        // Spawn tasks for reading from WebSocket and from stdin
        // and coordinate shutdown so either side can terminate the other.
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let state_clone = state.clone();
        let shutdown_tx_rx = shutdown_tx.clone();
        let mut rx_task = tokio::spawn(async move {
            loop {
                let msg = match ws_rx.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        let mut state = state_clone.lock().await;
                        state.log("WARN", &format!("WebSocket receive error: {}", e));
                        state.add_notice(format!("WebSocket receive error: {}", e));
                        break;
                    }
                    None => {
                        let mut state = state_clone.lock().await;
                        state.log("INFO", "Disconnected: server closed connection");
                        state.add_notice("Disconnected: server closed connection");
                        break;
                    }
                };

                if let Message::Text(text) = msg {
                    if let Ok(data) = serde_json::from_str::<JsonMap>(&text) {
                        dispatch_incoming_event(&state_clone, &data).await;
                    } else {
                        let mut state = state_clone.lock().await;
                        state.log("WARN", "Dropped malformed JSON payload from server");
                        state.add_notice("Dropped malformed JSON payload from server");
                    }
                }
            }
            let _ = shutdown_tx_rx.send(true);
        });

        let state_clone2 = state.clone();
        let shutdown_tx_stdin = shutdown_tx.clone();
        let mut shutdown_rx_stdin = shutdown_rx.clone();
        let mut stdin_task = tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            loop {
                let line = tokio::select! {
                    changed = shutdown_rx_stdin.changed() => {
                        if changed.is_ok() && *shutdown_rx_stdin.borrow() {
                            break;
                        }
                        continue;
                    }
                    line_result = lines.next_line() => {
                        match line_result {
                            Ok(Some(line)) => line,
                            Ok(None) => break,
                            Err(e) => {
                                let mut state = state_clone2.lock().await;
                                state.log("WARN", &format!("stdin read error: {}", e));
                                state.add_notice(format!("stdin read error: {}", e));
                                break;
                            }
                        }
                    }
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with('/') {
                    // Handle commands
                    let mut parts = line.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let args = parts.next().unwrap_or("");

                    let mut state = state_clone2.lock().await;
                    if matches!(
                        handle_slash_command(&mut state, cmd, args, &shutdown_tx_stdin),
                        CommandFlow::Exit
                    ) {
                        break;
                    }
                } else {
                    // Send as a regular message
                    let mut state = state_clone2.lock().await;
                    let channel = state.ch.clone();
                    queue_channel_message(&mut state, &channel, line);
                }
            }
        });

        tokio::select! {
            _ = &mut rx_task => {
                let _ = shutdown_tx.send(true);
                stdin_task.abort();
            }
            _ = &mut stdin_task => {
                let _ = shutdown_tx.send(true);
                rx_task.abort();
            }
        }

        let _ = rx_task.await;
        let _ = stdin_task.await;

        let (ws_tx_cleanup, voice_to_stop) = {
            let mut state = state.lock().await;
            state.running = false;
            state.voice_active = false;
            state.pw.zeroize();
            state.priv_key.zeroize();
            for key in state.chan_keys.values_mut() {
                key.zeroize();
            }
            for key in state.dm_keys.values_mut() {
                key.zeroize();
            }
            (state.ws_tx.clone(), state.voice_session.take())
        };

        if let Some(session) = voice_to_stop {
            enqueue_timed(
                &ws_tx_cleanup,
                serde_json::json!({"t": "vleave", "r": session.room.clone()}),
            );
            let _ = session.event_tx.send(VoiceEvent::Stop);
            let _ = session.task.join();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a `ClientState` with sensible defaults for unit tests.
    /// The returned sender can be used to read outbound messages if needed.
    fn test_client_state() -> (mpsc::UnboundedSender<String>, ClientState) {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let config = Config::default();
        let theme = config.resolve_theme();
        let state = ClientState {
            ws_tx: tx.clone(),
            me: "alice".to_string(),
            client_id: "CID-ABCD-1234-EF90".to_string(),
            pw: "hash".to_string(),
            ch: "general".to_string(),
            chs: HashMap::from([("general".to_string(), true)]),
            users: HashMap::new(),
            trust_store: TrustStore {
                path: PathBuf::from("test-trust.json"),
                peers: HashMap::new(),
            },
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: vec![],
            running: true,
            voice_active: false,
            voice_session: None,
            theme,
            file_transfers: HashMap::new(),
            message_history: VecDeque::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: PresenceState::Online.indicator(),
                presence: PresenceState::Online,
                bio: String::new(),
                avatar: default_avatar(),
            },
            reactions: HashMap::new(),
            unread_counts: HashMap::new(),
            unread_separator_scopes: HashSet::new(),
            activity_hint: None,
            typing_presence: HashMap::new(),
            recent_sents: VecDeque::new(),
            log_enabled: false,
            config,
            screen_share: None,
            screen_viewing: false,
            voice_members: Vec::new(),
            voice_muted: false,
            voice_deafened: false,
            voice_speaking: false,
            media_enabled: true,
        };
        (tx, state)
    }

    #[test]
    fn normalize_fingerprint_accepts_hex_and_colon_formats() {
        let raw = "aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";
        let normalized = normalize_fingerprint_input(raw).expect("normalized fingerprint");
        assert_eq!(normalized.len(), 64);
        assert!(normalized.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn trust_store_transitions_unknown_trusted_changed() {
        let mut store = TrustStore {
            path: PathBuf::from("trust-store-test.json"),
            peers: HashMap::new(),
        };
        let fp1 = canonical_fingerprint_from_hex(&"11".repeat(32));
        let fp2 = canonical_fingerprint_from_hex(&"22".repeat(32));

        let first = store.observe("alice", &fp1, "test", 100);
        assert!(first.persist_required);
        assert_eq!(store.state_for("alice"), TrustState::Unknown);

        store
            .trust_current_fingerprint("alice", &fp1, 110)
            .expect("trust initial key");
        assert_eq!(store.state_for("alice"), TrustState::Trusted);

        let changed = store.observe("alice", &fp2, "test", 120);
        assert!(changed.changed);
        assert_eq!(store.state_for("alice"), TrustState::Changed);

        store
            .trust_current_fingerprint("alice", &fp2, 130)
            .expect("trust rotated key");
        assert_eq!(store.state_for("alice"), TrustState::Trusted);
    }

    #[test]
    fn fingerprint_is_deterministic_for_same_public_key() {
        let keypair = new_keypair();
        let pk = pub_b64(&keypair).expect("public key base64");
        let fp1 = fingerprint_from_pubkey(&pk).expect("first fingerprint");
        let fp2 = fingerprint_from_pubkey(&pk).expect("second fingerprint");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn command_aliases_resolve_to_canonical_names() {
        assert_eq!(canonical_command("/q"), "/quit");
        assert_eq!(canonical_command("/w"), "/dm");
        assert_eq!(canonical_command("palette"), "/commands");
    }

    #[test]
    fn command_autocomplete_resolves_unique_prefix() {
        assert_eq!(unique_autocomplete_command("/swi"), Some("/switch"));
        assert_eq!(unique_autocomplete_command("/re"), None);
    }

    #[test]
    fn command_suggestions_return_useful_matches() {
        let suggestions = suggest_commands("/his");
        assert!(suggestions.contains(&"/history"));

        let typo_suggestions = suggest_commands("/hstory");
        assert!(typo_suggestions.contains(&"/history"));
    }

    #[test]
    fn generated_guest_username_is_valid() {
        let username = generate_guest_username();
        assert!(username.starts_with("guest-"));
        assert!(is_valid_username_token(&username));
        assert!(username.len() <= 32);
    }

    #[test]
    fn client_id_is_deterministic_and_grouped() {
        let first = build_client_id("alice", "cHVibGljLWtleS1iYXNlNjQ=");
        let second = build_client_id("alice", "cHVibGljLWtleS1iYXNlNjQ=");
        assert_eq!(first, second);
        assert!(first.starts_with("CID-"));
        assert_eq!(first.matches('-').count(), 3);
        assert_eq!(first.len(), 18);
    }

    #[test]
    fn grouped_feed_collapses_consecutive_channel_messages() {
        let messages = [
            DisplayedMessage {
                time: "10:00".to_string(),
                text: "first line".to_string(),
                msg_type: MessageType::Msg,
                user: Some("alice".to_string()),
                channel: Some("general".to_string()),
                edited: false,
            },
            DisplayedMessage {
                time: "10:01".to_string(),
                text: "second line".to_string(),
                msg_type: MessageType::Msg,
                user: Some("alice".to_string()),
                channel: Some("general".to_string()),
                edited: false,
            },
            DisplayedMessage {
                time: "10:02".to_string(),
                text: "system note".to_string(),
                msg_type: MessageType::Sys,
                user: None,
                channel: None,
                edited: false,
            },
        ];

        let rendered = render_grouped_feed_lines(messages.iter(), 80, 20, false, false);
        let header_count = rendered
            .iter()
            .filter(|line| line.contains("alice  #general"))
            .count();
        assert_eq!(header_count, 1);
        assert!(rendered
            .iter()
            .any(|line| line.starts_with("  > ") && line.contains("first line")));
        assert!(rendered
            .iter()
            .any(|line| line.starts_with("    ") && line.contains("second line")));
    }

    #[test]
    fn dashboard_header_lines_include_key_chips() {
        let (_tx, state) = test_client_state();
        let mut state = state;
        state.users.insert("alice".to_string(), "pk".to_string());

        let (header, subtitle) = dashboard_header_lines(&state);
        assert!(header.contains("[ONLINE:1]"));
        assert!(header.contains("[CHANNELS:1]"));
        assert!(header.contains("[UNREAD:0]"));
        assert!(header.contains("[TYPING:0]"));
        assert!(subtitle.contains("[ROOM:#general]"));
        assert!(subtitle.contains("[LIVE:idle]"));
        assert!(subtitle.contains("[CLIENT:CID-ABCD-1234-EF90]"));
    }

    #[test]
    fn unread_tracking_counts_and_clears_scopes() {
        let (_tx, mut state) = test_client_state();

        state.note_unread_scope("ops");
        state.note_unread_scope("ops");
        state.note_unread_scope(&dm_unread_scope("bob"));

        assert_eq!(state.unread_total(), 3);
        assert_eq!(state.unread_for_channel("ops"), 2);
        assert_eq!(state.unread_dm_total(), 1);
        assert!(state.unread_separator_scopes.contains("ops"));
        assert!(state.unread_separator_scopes.contains("dm:bob"));

        let markers: Vec<&DisplayedMessage> = state
            .message_history
            .iter()
            .filter(|m| m.msg_type == MessageType::UnreadMarker)
            .collect();
        assert_eq!(markers.len(), 2);
        assert!(markers
            .iter()
            .any(|m| m.text == "----- new activity in #ops -----"));
        assert!(markers
            .iter()
            .any(|m| m.text == "----- new DM from bob -----"));

        state.clear_unread_for_channel("ops");
        assert_eq!(state.unread_total(), 1);
        assert!(!state.unread_separator_scopes.contains("ops"));
        state.clear_unread_for_dm("bob");
        assert_eq!(state.unread_total(), 0);
        assert!(state.unread_separator_scopes.is_empty());
    }

    #[test]
    fn parse_typing_scope_supports_channel_and_dm() {
        let ch_scope = parse_typing_scope("#Ops", "general").expect("channel typing scope");
        match ch_scope {
            TypingScope::Channel(ch) => assert_eq!(ch, "ops"),
            TypingScope::Dm(_) => panic!("expected channel scope"),
        }

        let dm_scope = parse_typing_scope("dm:Bob", "general").expect("dm typing scope");
        match dm_scope {
            TypingScope::Dm(peer) => assert_eq!(peer, "bob"),
            TypingScope::Channel(_) => panic!("expected dm scope"),
        }
    }

    #[test]
    fn media_kind_inference_detects_image_and_video() {
        assert_eq!(infer_media_kind("preview.png", None), MediaKind::Image);
        assert_eq!(infer_media_kind("clip.mp4", None), MediaKind::Video);
        assert_eq!(infer_media_kind("payload.bin", None), MediaKind::File);
        assert_eq!(
            parse_media_kind_hint(Some("image"), "payload.bin", None),
            MediaKind::Image
        );
    }

    #[test]
    fn media_helpers_sanitize_and_unquote_paths() {
        assert_eq!(
            trim_wrapped_quotes("\"C:/Users/me/test image.png\""),
            "C:/Users/me/test image.png"
        );
        assert_eq!(trim_wrapped_quotes("'demo.mp4'"), "demo.mp4");
        assert_eq!(
            sanitize_filename(" ../My Clip (v1).mp4 "),
            "..My_Clip_v1.mp4"
        );
    }

    #[test]
    fn typing_presence_summary_tracks_active_entries() {
        let (_tx, mut state) = test_client_state();

        state.set_typing_presence("bob", "#ops", true);
        assert_eq!(state.typing_count(), 1);
        assert!(state.typing_summary(2).contains("bob@#ops"));

        state.set_typing_presence("bob", "#ops", false);
        assert_eq!(state.typing_count(), 0);
        assert_eq!(state.typing_summary(2), "none");
    }

    #[test]
    fn edit_command_expands_shortcodes_when_emoji_enabled() {
        let (_tx, mut state) = test_client_state();
        let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<String>();
        state.ws_tx = ws_tx;
        state.config.ui.enable_emoji = true;

        state.record_sent("general", "hello");
        state.push_message(DisplayedMessage {
            time: "10:00".to_string(),
            text: "hello".to_string(),
            msg_type: MessageType::Msg,
            user: Some(state.me.clone()),
            channel: Some("general".to_string()),
            edited: false,
        });

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let flow = handle_slash_command(&mut state, "/edit", "updated :rocket:", &shutdown_tx);
        assert!(matches!(flow, CommandFlow::Continue));

        let payload = ws_rx.try_recv().expect("missing outbound edit payload");
        let json: serde_json::Value = serde_json::from_str(&payload).expect("invalid JSON");
        assert_eq!(json["t"], "edit");
        assert_eq!(json["new_text"], "updated 🚀");
        assert!(state
            .message_history
            .iter()
            .any(|msg| msg.user.as_deref() == Some("alice") && msg.text == "updated 🚀"));
    }
}
