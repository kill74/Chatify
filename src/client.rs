//! Chatify WebSocket Client
//!
//! A real-time chat client with support for channels, direct messages, voice,
//! file transfers, message editing, reactions, and user status tracking.

use clicord_server::crypto::{
    channel_key, dec_bytes, dh_key, enc_bytes, new_keypair, pub_b64, pw_hash_client,
};
use clicord_server::error::{ChatifyError, ChatifyResult};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
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
use log::{debug, error, info, warn};

const MAX_HISTORY: usize = 100;
const INVALID_UTF8_PLACEHOLDER: &str = "[Invalid UTF-8]";
const HELP_TEXT: &str = "Available commands: /join, /dm, /me, /users, /channels, /voice [room], /history [limit], /search <query>, /rewind <Ns|Nm|Nh|Nd> [limit], /clear, /edit, /help, /quit";
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
const PROFILE_PANEL_ROWS: usize = 6;
const COMMANDS_PANEL_ROWS: usize = 6;
const MIN_CHANNEL_PANEL_ROWS: usize = 3;
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_HEADER: &str = "\x1b[38;5;147m";
const ANSI_LEFT: &str = "\x1b[38;5;117m";
const ANSI_RIGHT: &str = "\x1b[38;5;120m";
const ANSI_HINT: &str = "\x1b[38;5;220m";
const ANSI_DIM: &str = "\x1b[38;5;245m";
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

// Enumeration of supported message types in the protocol
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum MessageType {
    Msg,
    Img,
    Dm,
    Sys,
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
}

/// File transfer metadata
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileTransfer {
    /// Filename for display
    filename: String,
    /// Total file size in bytes
    size: u64,
    /// Received chunks
    chunks: Vec<String>,
    /// Number of bytes received so far
    received: u64,
}

/// User status information
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Status {
    /// Status text (e.g., "Online", "Away")
    text: String,
    /// Status emoji (e.g., '🟢')
    emoji: char,
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
    /// Password (stored in memory for key derivation)
    pw: String,
    /// Current channel
    ch: String,
    /// Map of subscribed channels
    chs: HashMap<String, bool>,
    /// Known users and their public keys
    users: HashMap<String, String>,
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
    /// Current theme
    theme: String,
    /// Active file transfers
    file_transfers: HashMap<String, FileTransfer>,
    /// Message history for display
    message_history: Vec<DisplayedMessage>,
    /// Current user status
    status: Status,
    /// Message reactions (msg_id -> emoji -> count)
    reactions: HashMap<String, HashMap<String, u32>>,
    /// Enable debug logging
    log_enabled: bool,
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

    /// Add a message to the history, keeping only the last 100 messages
    #[allow(dead_code)]
    fn add_message(&mut self, msg: DisplayedMessage) {
        self.message_history.push(msg);
        if self.message_history.len() > MAX_HISTORY {
            self.message_history.remove(0);
        }
        let _ = render_terminal_dashboard(self);
    }

    fn add_notice(&mut self, text: impl Into<String>) {
        self.add_message(DisplayedMessage {
            time: format_time(now_secs()),
            text: text.into(),
            msg_type: MessageType::Sys,
            user: None,
            channel: None,
        });
    }
}

// Helper functions

/// Format a Unix timestamp as HH:MM
fn format_time(ts: u64) -> String {
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or_else(Utc::now);
    datetime.format("%H:%M").to_string()
}

/// Placeholder for image to ASCII conversion
fn img_to_ascii(_: &[u8], _: u16) -> String {
    "[Image sending not yet implemented in Rust client]".to_string()
}

/// Normalize channel name to match server rules.
fn normalize_channel(raw: &str) -> Option<String> {
    let s: String = raw
        .to_lowercase()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn truncate_with_ellipsis(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
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
    let clipped_len = clipped.chars().count();
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

    for word in input.split_whitespace() {
        let current_len = current.chars().count();
        let word_len = word.chars().count();
        let next_len = if current_len == 0 {
            word_len
        } else {
            current_len + 1 + word_len
        };

        if next_len > width {
            if !current.is_empty() {
                lines.push(current);
            }
            current = truncate_with_ellipsis(word, width);
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
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
            format!("{} {}: {}", stamp, user, msg.text)
        }
        MessageType::Dm => {
            let user = msg.user.as_deref().unwrap_or("?");
            format!("{} DM {} {}", stamp, user, msg.text)
        }
        MessageType::Edit => format!("{} edit {}", stamp, msg.text),
        MessageType::Sys => format!("{} • {}", stamp, msg.text),
        _ => format!("{} {}", stamp, msg.text),
    }
}

fn collect_recent_feed_lines(state: &ClientState, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let start = state
        .message_history
        .len()
        .saturating_sub(FEED_LOOKBACK_MESSAGES);
    for msg in &state.message_history[start..] {
        let wrapped = wrap_words(&format_feed_entry(msg), width, max_lines);
        for line in wrapped {
            lines.push(line);
        }
    }

    if lines.is_empty() {
        lines.push("No messages yet. Join a channel and start chatting.".to_string());
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

    let profile_lines = vec![
        format!("{} {}", state.me, state.status.emoji),
        format!("status: {}", state.status.text),
        format!("channel: #{}", state.ch),
        format!("voice: {}", if state.voice_active { "ON" } else { "OFF" }),
    ];

    let mut command_lines = vec![
        "/join <channel>".to_string(),
        "/dm <user> <msg>".to_string(),
        "/users /channels".to_string(),
        "/history /search".to_string(),
    ];
    if let Some(last) = state.message_history.last() {
        command_lines.push(truncate_with_ellipsis(
            &format!("last: {}", format_feed_entry(last)),
            width.saturating_sub(2),
        ));
    }

    let mut online_users: Vec<String> = state.users.keys().cloned().collect();
    online_users.sort_unstable();
    let online_lines = if online_users.is_empty() {
        vec!["(waiting for users feed)".to_string()]
    } else {
        online_users
            .into_iter()
            .map(|u| format!("• {}", u))
            .collect()
    };

    let mut channels: Vec<String> = state.chs.keys().cloned().collect();
    channels.sort_unstable();
    let channel_lines = if channels.is_empty() {
        vec!["(none)".to_string()]
    } else {
        channels
            .into_iter()
            .map(|ch| {
                if ch == state.ch {
                    format!("* #{}", ch)
                } else {
                    format!("  #{}", ch)
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
            "MESSAGES",
            &command_lines,
            width,
            commands_h.min(height.saturating_sub(out.len())),
        ));
    }
    if out.len() < height {
        out.extend(build_panel(
            "NOW ONLINE",
            &online_lines,
            width,
            online_h.min(height.saturating_sub(out.len())),
        ));
    }
    if out.len() < height {
        out.extend(build_panel(
            "YOUR CHANNELS",
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
    writeln!(
        out,
        "{}{}{}",
        ANSI_HEADER,
        fit_to_width("// CHATIFY //", width),
        ANSI_RESET
    )?;
    writeln!(
        out,
        "{}{}{}",
        ANSI_DIM,
        fit_to_width(
            "Terminal is small. Expand window for full dashboard mode.",
            width
        ),
        ANSI_RESET
    )?;

    let max_feed_lines = height.saturating_sub(DASHBOARD_HEADER_ROWS + DASHBOARD_FOOTER_ROWS);
    for msg in state
        .message_history
        .iter()
        .rev()
        .take(max_feed_lines)
        .rev()
    {
        writeln!(out, "{}", fit_to_width(&format_feed_entry(msg), width))?;
    }

    writeln!(
        out,
        "{}{}{}",
        ANSI_HINT,
        fit_to_width("-> /help for commands", width),
        ANSI_RESET
    )
}

fn render_full_dashboard(
    out: &mut io::Stdout,
    state: &ClientState,
    geom: DashboardGeometry,
) -> io::Result<()> {
    let header = format!(
        "[ {} online ] [ {} channels ] [ {} events ]   // CHATIFY RETRO FEED //",
        state.users.len(),
        state.chs.len(),
        state.message_history.len()
    );
    let subtitle = format!(
        "#{}   mode:{}   voice:{}",
        state.ch,
        state.theme,
        if state.voice_active { "on" } else { "off" }
    );

    writeln!(
        out,
        "{}{}{}",
        ANSI_HEADER,
        fit_to_width(&header, geom.width),
        ANSI_RESET
    )?;
    writeln!(
        out,
        "{}{}{}",
        ANSI_DIM,
        fit_to_width(&subtitle, geom.width),
        ANSI_RESET
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
            ANSI_LEFT, left_line, ANSI_RESET, ANSI_RIGHT, right_line, ANSI_RESET
        )?;
    }

    writeln!(
        out,
        "{}{}{}",
        ANSI_HINT,
        fit_to_width(
            "-> Type + Enter | /join #channel | /dm user msg | /help | /quit",
            geom.width
        ),
        ANSI_RESET
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

fn fresh_nonce_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn read_username() -> ChatifyResult<String> {
    print!("username: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let username = input.trim();
    if username.is_empty() {
        println!("Empty username provided, using 'anon'.");
        Ok("anon".to_string())
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
            });
            true
        }
        "edit" => {
            let user = event.get("u").and_then(|v| v.as_str()).unwrap_or("?");
            let old_text = event.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
            let new_text = event.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
            state.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!("{} edited: '{}' -> '{}'", user, old_text, new_text),
                msg_type: MessageType::Edit,
                user: Some(user.to_string()),
                channel: event
                    .get("ch")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
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
        state_lock.add_message(DisplayedMessage {
            time: format_time(ts),
            text: String::from_utf8(content)
                .unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string()),
            msg_type: MessageType::Msg,
            user: Some(u.to_string()),
            channel: Some(ch.to_string()),
        });
    }
}

async fn handle_img_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let a = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
    let encrypted = match general_purpose::STANDARD.decode(a) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let ascii_art = img_to_ascii(&encrypted, 70);

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("{} sent an image:\n{}", u, ascii_art),
        msg_type: MessageType::Img,
        user: Some(u.to_string()),
        channel: Some(ch.to_string()),
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
    });
}

async fn handle_dm_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
    if let Some(pk) = data.get("pk").and_then(|v| v.as_str()) {
        if !pk.is_empty() {
            state
                .lock()
                .await
                .users
                .insert(frm.to_string(), pk.to_string());
        }
    }
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let mut state_lock = state.lock().await;
    let peer = if frm == state_lock.me { to } else { frm };
    if let Ok(dm_key) = state_lock.dmkey(peer) {
        if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
            let arrow = if frm == state_lock.me { "→" } else { "←" };
            state_lock.add_message(DisplayedMessage {
                time: format_time(ts),
                text: format!(
                    "{} {} {}",
                    frm,
                    arrow,
                    String::from_utf8(content)
                        .unwrap_or_else(|_| INVALID_UTF8_PLACEHOLDER.to_string())
                ),
                msg_type: MessageType::Dm,
                user: Some(frm.to_string()),
                channel: None,
            });
        }
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
                if !pk.is_empty() {
                    state_lock.users.insert(name.to_string(), pk.to_string());
                }
            }
        }
    }
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("Online users: {}", names.join(", ")),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
    });
}

async fn handle_joined_event(state: &SharedState, data: &JsonMap, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let hist_val = data.get("hist").cloned().unwrap_or(serde_json::json!([]));
    let hist: Vec<serde_json::Value> = serde_json::from_value(hist_val).unwrap_or_default();

    let mut state_lock = state.lock().await;
    state_lock.ch = ch.to_string();
    state_lock.chs.insert(ch.to_string(), true);
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: format!("→ #{}", ch),
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
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
    let summary = if t == "search" {
        let q = data.get("q").and_then(|v| v.as_str()).unwrap_or("");
        format!("Search '{}' in #{}: {} event(s)", q, ch, count)
    } else {
        format!("History for #{}: {} event(s)", ch, count)
    };
    state_lock.add_message(DisplayedMessage {
        time: format_time(ts),
        text: summary,
        msg_type: MessageType::Sys,
        user: None,
        channel: None,
    });
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
    });
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

async fn dispatch_incoming_event(state: &SharedState, data: &JsonMap) {
    let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
    let ts = data.get("ts").map(ts_value).unwrap_or(0);
    match t {
        "msg" => handle_msg_event(state, data, ts).await,
        "img" => handle_img_event(state, data, ts).await,
        "sys" => handle_sys_event(state, data, ts).await,
        "dm" => handle_dm_event(state, data, ts).await,
        "users" => handle_users_event(state, data, ts).await,
        "joined" => handle_joined_event(state, data, ts).await,
        "history" | "search" => handle_history_or_search_event(state, data, t, ts).await,
        "info" => handle_info_event(state, data, ts).await,
        "vdata" => handle_vdata_event(state, data).await,
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
    let encrypted = match enc_bytes(&state.ckey(channel), plaintext.as_bytes()) {
        Ok(v) => v,
        Err(e) => {
            state.add_notice(format!("encryption failed: {}", e));
            return;
        }
    };
    let encoded = general_purpose::STANDARD.encode(&encrypted);
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
    match cmd {
        "/join" => {
            if let Some(ch) = normalize_channel(args.trim()) {
                state.ch = ch.clone();
                state.chs.insert(ch.clone(), true);
                enqueue_timed(&state.ws_tx, serde_json::json!({"t": "join", "ch": ch}));
            } else {
                state.add_notice("Usage: /join <channel>. Allowed: letters, digits, -, _");
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

            let encrypted = match state.dmkey(target) {
                Ok(key) => match enc_bytes(&key, msg.as_bytes()) {
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
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "dm", "to": target, "c": encoded, "p": msg}),
            );
        }
        "/me" => {
            if !args.is_empty() {
                let ch = state.ch.clone();
                let msg = format!("* {} {}", state.me, args);
                queue_channel_message(state, &ch, &msg);
            }
        }
        "/users" => {
            enqueue_timed(&state.ws_tx, serde_json::json!({"t": "users"}));
        }
        "/channels" => {
            enqueue_timed(&state.ws_tx, serde_json::json!({"t": "info"}));
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
            let _ = render_terminal_dashboard(state);
        }
        "/help" => {
            state.add_notice(HELP_TEXT);
        }
        "/edit" => {
            state.add_notice("Edit command not yet implemented in Rust client");
        }
        "/history" => {
            let limit = parse_limit(args, 50, 200);
            let ch = state.ch.clone();
            enqueue_timed(
                &state.ws_tx,
                serde_json::json!({"t": "history", "ch": ch, "limit": limit}),
            );
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
        "/quit" | "/exit" | "/q" => {
            stop_voice_session(state, SessionStopFeedback::Silent);
            state.running = false;
            let _ = shutdown_tx.send(true);
            return CommandFlow::Exit;
        }
        _ => {
            state.add_notice(format!("Unknown command: {}", cmd));
        }
    }

    CommandFlow::Continue
}

/// Main entry point for the WebSocket client
#[tokio::main]
async fn main() -> ChatifyResult<()> {
    // Parse command line arguments
    let matches = clap::Command::new("chatify-client")
        .version("1.0")
        .author("Chatify Team")
        .about("WebSocket chat client with encryption")
        .arg(
            clap::Arg::new("host")
                .long("host")
                .value_name("HOST")
                .default_value("127.0.0.1")
                .help("Server host"),
        )
        .arg(
            clap::Arg::new("port")
                .long("port")
                .value_name("PORT")
                .default_value("8765")
                .help("Server port"),
        )
        .arg(
            clap::Arg::new("tls")
                .long("tls")
                .action(clap::ArgAction::SetTrue)
                .help("Use TLS for secure connection"),
        )
        .arg(
            clap::Arg::new("log")
                .long("log")
                .action(clap::ArgAction::SetTrue)
                .help("Enable debug logging"),
        )
        .get_matches();

    let host = matches.get_one::<String>("host").unwrap();
    let port = matches.get_one::<String>("port").unwrap();
    let tls = matches.get_flag("tls");
    let log_enabled = matches.get_flag("log");

    let scheme = if tls { "wss" } else { "ws" };
    let uri = format!("{}://{}:{}", scheme, host, port);

    let username = read_username()?;

    let mut password = rpassword::prompt_password("password: ")?;
    let client_priv_key = new_keypair();
    let client_pub_key =
        pub_b64(&client_priv_key).expect("generated keypair must produce valid public key");

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
            me: auth.me.clone(),
            pw: pw_hash,
            ch: "general".to_string(),
            chs: HashMap::from([("general".to_string(), true)]),
            users: auth.users,
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: client_priv_key,
            running: true,
            voice_active: false,
            voice_session: None,
            theme: "retro-grid".to_string(),
            file_transfers: HashMap::new(),
            message_history: Vec::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: '🟢',
            },
            reactions: HashMap::new(),
            log_enabled,
        }));
        {
            let mut state_lock = state.lock().await;
            state_lock.me = auth.me;
            state_lock.add_notice(format!("Connected to {}", uri));
            state_lock.add_notice("Use /help to list commands.");
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
