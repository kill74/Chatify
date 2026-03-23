//! Chatify WebSocket Client
//!
//! A real-time chat client with support for channels, direct messages, voice,
//! file transfers, message editing, reactions, and user status tracking.

use clicord_server::crypto::{
    channel_key, dec_bytes, dh_key, enc_bytes, new_keypair, pub_b64, pw_hash,
};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{self, engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use chrono::{DateTime, Utc};
use futures_util::SinkExt;
use futures_util::StreamExt;

const MAX_HISTORY: usize = 100;
const INVALID_UTF8_PLACEHOLDER: &str = "[Invalid UTF-8]";
const HELP_TEXT: &str = "Available commands: /join, /dm, /me, /users, /channels, /voice [room], /history [limit], /search <query>, /rewind <Ns|Nm|Nh|Nd> [limit], /clear, /edit, /help, /quit";
type SharedState = Arc<tokio::sync::Mutex<ClientState>>;
type JsonMap = HashMap<String, serde_json::Value>;

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
        if self.log_enabled {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            println!("[{}] {}: {}", timestamp, level, msg);
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

    fn dmkey(&mut self, name: &str) -> Result<Vec<u8>, String> {
        if !self.dm_keys.contains_key(name) {
            let pk = self
                .users
                .get(name)
                .ok_or_else(|| format!("User {} not found", name))?;
            let key = dh_key(&self.priv_key, pk);
            if key.len() != 32 {
                return Err(format!("Invalid public key for user {}", name));
            }
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

fn enqueue_json(ws_tx: &mpsc::UnboundedSender<String>, payload: serde_json::Value) {
    let _ = ws_tx.send(payload.to_string());
}

fn enqueue_timed(ws_tx: &mpsc::UnboundedSender<String>, mut payload: serde_json::Value) {
    if payload.get("ts").is_none() {
        payload["ts"] = serde_json::json!(now_secs());
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
) -> Result<Stream, String> {
    let pending = Arc::new(Mutex::new(VecDeque::<i16>::new()));
    let err_fn = |e| eprintln!("voice input error: {}", e);
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
                .map_err(|e| format!("failed to build i16 input stream: {}", e))
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
                .map_err(|e| format!("failed to build u16 input stream: {}", e))
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
                .map_err(|e| format!("failed to build f32 input stream: {}", e))
        }
    }
}

fn start_voice_session(
    room: String,
    ws_tx: mpsc::UnboundedSender<String>,
) -> Result<VoiceSession, String> {
    let (event_tx, event_rx) = std_mpsc::channel::<VoiceEvent>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
    let ws_tx_task = ws_tx;
    let room_task = room.clone();
    let event_tx_thread = event_tx.clone();

    let task = thread::spawn(move || {
        let host = cpal::default_host();
        let input_device = match host.default_input_device() {
            Some(d) => d,
            None => {
                let _ = ready_tx.send(Err("no default input device available".to_string()));
                return;
            }
        };
        let input_supported = match input_device.default_input_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("failed to fetch default input config: {}", e)));
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
                let _ = ready_tx.send(Err(format!("audio output init failed: {}", e)));
                return;
            }
        };
        let sink = match Sink::try_new(&output_handle) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("audio output sink init failed: {}", e)));
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
            let _ = ready_tx.send(Err(format!("failed to start microphone stream: {}", e)));
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
            Err("voice runtime failed to initialize".to_string())
        }
    }
}

/// Main entry point for the WebSocket client
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let username = {
        print!("username: ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let u = input.trim().to_string();
        if u.is_empty() {
            println!("Empty username provided, using 'anon'.");
            "anon".to_string()
        } else {
            u
        }
    };

    let password = rpassword::prompt_password("password: ").unwrap();
    let client_priv_key = new_keypair();
    let client_pub_key = pub_b64(&client_priv_key);

    // Set up logging
    if log_enabled {
        env_logger::init();
    }

    // Connect to WebSocket
    let (ws_stream, _) = connect_async(&uri).await?;
    println!("Connected to server");
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Authenticate
    let auth_msg = serde_json::json!({
        "t": "auth",
        "u": username,
        "pw": pw_hash(&password),
        "pk": client_pub_key,
        "status": {"text": "Online", "emoji": "🟢"}
    });
    ws_tx.send(Message::Text(auth_msg.to_string())).await?;

    // Wait for auth response with timeout
    let auth_reply = timeout(Duration::from_secs(10), ws_rx.next()).await;
    let resp = match auth_reply {
        Ok(Some(Ok(Message::Text(resp)))) => resp,
        Ok(Some(Ok(_))) => {
            eprintln!("Authentication failed: server sent unexpected frame");
            return Ok(());
        }
        Ok(Some(Err(e))) => {
            eprintln!("Authentication failed: websocket error: {}", e);
            return Ok(());
        }
        Ok(None) => {
            eprintln!("Authentication failed: connection closed by server");
            return Ok(());
        }
        Err(_) => {
            eprintln!("Authentication failed: timeout waiting for server response");
            return Ok(());
        }
    };

    {
        let resp_val: serde_json::Value = match serde_json::from_str(&resp) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Authentication failed: invalid JSON from server: {}", e);
                return Ok(());
            }
        };
        if resp_val["t"] == "err" {
            eprintln!("Authentication failed: {}", resp_val["m"]);
            return Ok(());
        }
        // Update state with server response
        let me = resp_val["u"].as_str().unwrap_or(&username).to_string();
        let users: Vec<serde_json::Value> =
            serde_json::from_value(resp_val["users"].clone()).unwrap_or_default();
        let mut user_map = HashMap::new();
        for u in users {
            if let Some(name) = u["u"].as_str() {
                if let Some(pk) = u["pk"].as_str() {
                    user_map.insert(name.to_string(), pk.to_string());
                }
            }
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
        let state = Arc::new(tokio::sync::Mutex::new(ClientState {
            ws_tx: msg_tx.clone(), // Use mpsc sender instead of WebSocket
            me: me.clone(),
            pw: password,
            ch: "general".to_string(),
            chs: HashMap::from([("general".to_string(), true)]),
            users: user_map,
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: client_priv_key,
            running: true,
            voice_active: false,
            voice_session: None,
            theme: "default".to_string(),
            file_transfers: HashMap::new(),
            message_history: Vec::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: '🟢',
            },
            reactions: HashMap::new(),
            log_enabled,
        }));
        state.lock().await.me = me;

        // Spawn tasks for reading from WebSocket and from stdin
        let state_clone = state.clone();
        let rx_task = tokio::spawn(async move {
            loop {
                let msg = match ws_rx.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        let state = state_clone.lock().await;
                        state.log("WARN", &format!("WebSocket receive error: {}", e));
                        break;
                    }
                    None => {
                        let state = state_clone.lock().await;
                        state.log("INFO", "Disconnected: server closed connection");
                        break;
                    }
                };

                if let Message::Text(text) = msg {
                    if let Ok(data) = serde_json::from_str::<JsonMap>(&text) {
                        dispatch_incoming_event(&state_clone, &data).await;
                    } else {
                        let state = state_clone.lock().await;
                        state.log("WARN", "Dropped malformed JSON payload from server");
                    }
                }
            }
        });

        let state_clone2 = state.clone();
        let stdin_task = tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            loop {
                let line = match lines.next_line().await {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(e) => {
                        let state = state_clone2.lock().await;
                        state.log("WARN", &format!("stdin read error: {}", e));
                        break;
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
                    match cmd {
                        "/join" => {
                            if let Some(ch) = normalize_channel(args.trim()) {
                                state.ch = ch.to_string();
                                state.chs.insert(ch.to_string(), true);
                                // Request history for the channel
                                enqueue_timed(
                                    &state.ws_tx,
                                    serde_json::json!({"t": "join", "ch": ch}),
                                );
                            } else {
                                state.log(
                                    "INFO",
                                    "Usage: /join <channel>. Allowed: letters, digits, -, _",
                                );
                            }
                        }
                        "/dm" => {
                            let mut parts = args.splitn(2, ' ');
                            let target = parts.next().unwrap_or("");
                            let msg = parts.next().unwrap_or("");
                            if target.is_empty() || msg.is_empty() {
                                state.log("INFO", "Usage: /dm <user> <message>");
                                continue;
                            }
                            if target == state.me {
                                state.log("INFO", "Cannot DM yourself.");
                                continue;
                            }
                            {
                                let encrypted = match state.dmkey(target) {
                                    Ok(key) => enc_bytes(&key, msg.as_bytes()),
                                    Err(e) => {
                                        state.log("WARN", &format!("DM failed: {}", e));
                                        continue;
                                    }
                                };
                                let encoded = general_purpose::STANDARD.encode(&encrypted);
                                enqueue_timed(
                                    &state.ws_tx,
                                    serde_json::json!({"t": "dm", "to": target, "c": encoded, "p": msg}),
                                );
                            }
                        }
                        "/me" => {
                            let action = args;
                            if !action.is_empty() {
                                let ch = state.ch.clone();
                                let msg = format!("* {} {}", state.me, action);
                                let encrypted = enc_bytes(&state.ckey(&ch), msg.as_bytes());
                                let encoded = general_purpose::STANDARD.encode(&encrypted);
                                enqueue_timed(
                                    &state.ws_tx,
                                    serde_json::json!({"t": "msg", "ch": ch, "c": encoded, "p": msg}),
                                );
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
                                if let Some(session) = state.voice_session.take() {
                                    state.voice_active = false;
                                    enqueue_timed(
                                        &state.ws_tx,
                                        serde_json::json!({"t": "vleave", "r": session.room}),
                                    );
                                    let _ = session.event_tx.send(VoiceEvent::Stop);
                                    let _ = session.task.join();
                                    println!("Voice stopped");
                                    state.log("INFO", "Voice session stopped");
                                }
                            } else {
                                let room = normalize_channel(args.trim())
                                    .unwrap_or_else(|| state.ch.clone());
                                let ws_tx = state.ws_tx.clone();
                                match start_voice_session(room.clone(), ws_tx.clone()) {
                                    Ok(session) => {
                                        state.voice_active = true;
                                        state.voice_session = Some(session);
                                        enqueue_timed(
                                            &ws_tx,
                                            serde_json::json!({"t": "vjoin", "r": room}),
                                        );
                                        println!("Voice started in #{}", room);
                                        state.log("INFO", "Voice session started");
                                    }
                                    Err(e) => {
                                        eprintln!("Voice start failed: {}", e);
                                        eprintln!("Check that your microphone/speakers are available and not in use by another app.");
                                        state.log("WARN", &format!("Voice start failed: {}", e));
                                    }
                                }
                            }
                        }
                        "/clear" => {
                            // Clear the terminal by printing many newlines
                            for _ in 0..50 {
                                println!();
                            }
                        }
                        "/help" => {
                            state.log("INFO", HELP_TEXT);
                        }
                        "/edit" => {
                            // Placeholder for edit
                            state.log("INFO", "Edit command not yet implemented in Rust client");
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
                                state.log("INFO", "Usage: /search <query>");
                                continue;
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
                                state.log("INFO", "Usage: /rewind <Ns|Nm|Nh|Nd> [limit]");
                                continue;
                            }
                            let Some(seconds) = parse_duration_secs(window) else {
                                state.log(
                                    "INFO",
                                    "Invalid duration. Use formats like 90s, 15m, 2h, 1d.",
                                );
                                continue;
                            };
                            let limit = parse_limit(p.next().unwrap_or(""), 100, 500);
                            let ch = state.ch.clone();
                            enqueue_timed(
                                &state.ws_tx,
                                serde_json::json!({"t": "rewind", "ch": ch, "seconds": seconds, "limit": limit}),
                            );
                        }
                        "/quit" | "/exit" | "/q" => {
                            if let Some(session) = state.voice_session.take() {
                                enqueue_timed(
                                    &state.ws_tx,
                                    serde_json::json!({"t": "vleave", "r": session.room}),
                                );
                                let _ = session.event_tx.send(VoiceEvent::Stop);
                                let _ = session.task.join();
                            }
                            state.voice_active = false;
                            state.running = false;
                            break;
                        }
                        _ => {
                            state.log("INFO", &format!("Unknown command: {}", cmd));
                        }
                    }
                } else {
                    // Send as a regular message
                    let (channel, pw) = {
                        let state = state_clone2.lock().await;
                        (state.ch.clone(), state.pw.clone())
                    };
                    let key = channel_key(&pw, &channel);
                    let encrypted = enc_bytes(&key, line.as_bytes());
                    let encoded = general_purpose::STANDARD.encode(&encrypted);
                    let state = state_clone2.lock().await;
                    enqueue_timed(
                        &state.ws_tx,
                        serde_json::json!({"t": "msg", "ch": channel, "c": encoded, "p": line}),
                    );
                }
            }
        });

        // Wait for tasks to complete
        let _ = rx_task.await;
        let _ = stdin_task.await;

        // Clean up
        let mut state = state.lock().await;
        state.running = false;
    }

    Ok(())
}
