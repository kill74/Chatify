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
#[cfg(feature = "screen-capture")]
use std::time::{Duration as StdDuration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{self, engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
#[cfg(feature = "screen-capture")]
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
#[cfg(feature = "screen-capture")]
use image::{DynamicImage, RgbImage};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};
#[cfg(feature = "screen-capture")]
use scrap::{Capturer, Display};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use chrono::{DateTime, Utc};
use futures_util::SinkExt;
use futures_util::StreamExt;

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
    Sdata,
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

/// Runtime state for an active screen sharing session.
struct ScreenSession {
    room: String,
    stop_tx: std_mpsc::Sender<()>,
    task: thread::JoinHandle<()>,
}

enum VoiceEvent {
    Captured(VoiceFrame),
    Playback(VoiceFrame),
    Stop,
}

#[derive(Debug, Clone)]
struct PendingScreenFrame {
    codec: String,
    width: u32,
    height: u32,
    total: usize,
    keyframe: bool,
    created_at: u64,
    chunks: Vec<Option<String>>,
}

#[cfg(feature = "screen-capture")]
const SCREEN_TARGET_WIDTH: u32 = 960;
#[cfg(feature = "screen-capture")]
const SCREEN_FPS: u64 = 8;
#[cfg(feature = "screen-capture")]
const SCREEN_CHUNK_BYTES: usize = 32 * 1024;
#[cfg(feature = "screen-capture")]
const SCREEN_JPEG_QUALITY: u8 = 55;
const SCREEN_PREVIEW_WIDTH: u16 = 56;
const SCREEN_MAX_PENDING_FRAMES: usize = 64;
const SCREEN_PENDING_TTL_SECS: u64 = 8;

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
    /// Whether we're actively sharing screen
    screen_active: bool,
    /// Room currently used for screen stream subscription
    screen_room: Option<String>,
    /// Active screen capture runtime (if any)
    screen_session: Option<ScreenSession>,
    /// Reassembly cache for incoming chunked screen frames
    pending_screen_frames: HashMap<String, PendingScreenFrame>,
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
        if self.message_history.len() > 100 {
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

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert an image payload to a compact ASCII preview for terminal rendering.
fn img_to_ascii(bytes: &[u8], width: u16) -> String {
    let img = match image::load_from_memory(bytes) {
        Ok(img) => img,
        Err(_) => return "[invalid image frame]".to_string(),
    };
    let gray = img.grayscale().to_luma8();
    let (src_w, src_h) = gray.dimensions();
    if src_w == 0 || src_h == 0 {
        return "[empty image frame]".to_string();
    }

    let target_w = u32::from(width).max(1).min(src_w.max(1));
    // Approximate terminal character aspect ratio.
    let target_h = ((src_h as f32 / src_w as f32) * target_w as f32 * 0.5)
        .round()
        .max(1.0) as u32;

    let resized = image::imageops::resize(&gray, target_w, target_h, FilterType::Triangle);
    let chars: &[u8] = b" .:-=+*#%@";
    let mut out = String::with_capacity((target_w as usize + 1) * target_h as usize);
    for y in 0..target_h {
        for x in 0..target_w {
            let px = resized.get_pixel(x, y)[0] as usize;
            let idx = px * (chars.len() - 1) / 255;
            out.push(chars[idx] as char);
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(feature = "screen-capture")]
fn encode_screen_frame_jpeg(
    frame_bgra: &[u8],
    width: usize,
    height: usize,
) -> Option<(Vec<u8>, u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let px_count = width.checked_mul(height)?;
    if frame_bgra.len() < px_count.checked_mul(4)? {
        return None;
    }

    let mut rgb = Vec::with_capacity(px_count * 3);
    for px in frame_bgra.chunks_exact(4).take(px_count) {
        // scrap provides BGRA.
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }

    let rgb_image = RgbImage::from_raw(width as u32, height as u32, rgb)?;
    let target_w = SCREEN_TARGET_WIDTH.min(rgb_image.width()).max(1);
    let target_h = ((rgb_image.height() as f32 / rgb_image.width() as f32) * target_w as f32)
        .round()
        .max(1.0) as u32;
    let resized = image::imageops::resize(&rgb_image, target_w, target_h, FilterType::Triangle);

    let mut encoded = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut encoded, SCREEN_JPEG_QUALITY);
    let dyn_img = DynamicImage::ImageRgb8(resized);
    if encoder.encode_image(&dyn_img).is_err() {
        return None;
    }

    Some((encoded, target_w, target_h))
}

#[cfg(feature = "screen-capture")]
fn send_screen_frame_chunks(
    ws_tx: &mpsc::UnboundedSender<String>,
    room: &str,
    frame: &[u8],
    seq: u64,
    width: u32,
    height: u32,
) {
    if frame.is_empty() {
        return;
    }
    let total = (frame.len() + SCREEN_CHUNK_BYTES - 1) / SCREEN_CHUNK_BYTES;
    for (idx, chunk) in frame.chunks(SCREEN_CHUNK_BYTES).enumerate() {
        let payload = general_purpose::STANDARD.encode(chunk);
        let _ = ws_tx.send(
            serde_json::json!({
                "t": "sdata",
                "r": room,
                "a": payload,
                "codec": "jpeg",
                "seq": seq,
                "chunk": idx,
                "total": total,
                "w": width,
                "h": height,
                "kf": seq % 24 == 0,
                "ts": unix_now_secs()
            })
            .to_string(),
        );
    }
}

#[cfg(feature = "screen-capture")]
fn start_screen_session(
    room: String,
    ws_tx: mpsc::UnboundedSender<String>,
) -> Result<ScreenSession, String> {
    let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
    let room_task = room.clone();

    let task = thread::spawn(move || {
        let display = match Display::primary() {
            Ok(display) => display,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("failed to get primary display: {}", e)));
                return;
            }
        };

        let mut capturer = match Capturer::new(display) {
            Ok(capturer) => capturer,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("failed to initialize capture: {}", e)));
                return;
            }
        };

        let (cap_w, cap_h) = (capturer.width(), capturer.height());
        let _ = ready_tx.send(Ok(()));

        let frame_interval = StdDuration::from_millis((1000 / SCREEN_FPS.max(1)).max(1));
        let mut next_tick = Instant::now();
        let mut seq: u64 = 0;

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            let now_tick = Instant::now();
            if now_tick < next_tick {
                thread::sleep(next_tick - now_tick);
            }
            next_tick = Instant::now() + frame_interval;

            let frame = match capturer.frame() {
                Ok(frame) => frame,
                Err(e) => {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        thread::sleep(StdDuration::from_millis(6));
                        continue;
                    }
                    eprintln!("screen capture error: {}", e);
                    break;
                }
            };

            if let Some((jpeg, out_w, out_h)) = encode_screen_frame_jpeg(&frame, cap_w, cap_h) {
                send_screen_frame_chunks(&ws_tx, &room_task, &jpeg, seq, out_w, out_h);
                seq = seq.wrapping_add(1);
            }
        }
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(ScreenSession {
            room,
            stop_tx,
            task,
        }),
        Ok(Err(e)) => {
            let _ = task.join();
            Err(e)
        }
        Err(_) => {
            let _ = task.join();
            Err("screen runtime failed to initialize".to_string())
        }
    }
}

#[cfg(not(feature = "screen-capture"))]
fn start_screen_session(
    _room: String,
    _ws_tx: mpsc::UnboundedSender<String>,
) -> Result<ScreenSession, String> {
    Err(
        "screen capture backend not enabled. Rebuild with --features screen-capture and install xcb development libraries"
            .to_string(),
    )
}

fn stop_screen_session(session: ScreenSession) {
    let _ = session.stop_tx.send(());
    let _ = session.task.join();
}

fn cleanup_pending_screen_frames(pending: &mut HashMap<String, PendingScreenFrame>) {
    let now = unix_now_secs();
    pending.retain(|_, frame| now.saturating_sub(frame.created_at) <= SCREEN_PENDING_TTL_SECS);

    if pending.len() <= SCREEN_MAX_PENDING_FRAMES {
        return;
    }

    let mut keys_by_age: Vec<(String, u64)> = pending
        .iter()
        .map(|(k, v)| (k.clone(), v.created_at))
        .collect();
    keys_by_age.sort_by_key(|(_, created_at)| *created_at);
    let overflow = pending.len().saturating_sub(SCREEN_MAX_PENDING_FRAMES);
    for (key, _) in keys_by_age.into_iter().take(overflow) {
        pending.remove(&key);
    }
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

/// Parse users payload and format for display with status
fn parse_users_payload(users_val: &serde_json::Value) -> (Vec<String>, HashMap<String, String>) {
    let users: Vec<serde_json::Value> =
        serde_json::from_value(users_val.clone()).unwrap_or_default();
    let mut names = Vec::new();
    let mut keys = HashMap::new();

    for user in users {
        if let Some(name) = user.get("u").and_then(|v| v.as_str()) {
            let name = name.to_string();
            names.push(name.clone());
            if let Some(pk) = user.get("pk").and_then(|v| v.as_str()) {
                if !pk.is_empty() {
                    keys.insert(name, pk.to_string());
                }
            }
        }
    }

    (names, keys)
}

/// Format user roster with state and status information
fn format_user_roster(users_val: &serde_json::Value) -> String {
    let users: Vec<serde_json::Value> =
        serde_json::from_value(users_val.clone()).unwrap_or_default();

    if users.is_empty() {
        return "No users online".to_string();
    }

    let mut online_users = Vec::new();
    let mut idle_users = Vec::new();

    for user in users {
        if let Some(name) = user.get("u").and_then(|v| v.as_str()) {
            let state = user
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("online");
            let status = user.get("status").and_then(|v| v.as_str());

            let display = if let Some(status_msg) = status {
                if !status_msg.is_empty() {
                    format!("{} ({})", name, status_msg)
                } else {
                    name.to_string()
                }
            } else {
                name.to_string()
            };

            match state {
                "idle" => idle_users.push(format!("💤 {}", display)),
                _ => online_users.push(format!("🟢 {}", display)),
            }
        }
    }

    let mut result = String::from("━━━ LIVE ROSTER ━━━\n");

    if !online_users.is_empty() {
        result.push_str(&format!("Online ({}):\n", online_users.len()));
        for user in online_users {
            result.push_str(&format!("  {}\n", user));
        }
    }

    if !idle_users.is_empty() {
        result.push_str(&format!("Away ({}):\n", idle_users.len()));
        for user in idle_users {
            result.push_str(&format!("  {}\n", user));
        }
    }

    result.trim_end().to_string()
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
                    let _ = ws_tx_task.send(
                        serde_json::json!({
                            "t": "vdata",
                            "r": room_task.clone(),
                            "a": payload,
                            "ts": SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                        })
                        .to_string(),
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

fn read_stdin_line(prompt: &str) -> io::Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
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
        .arg(
            clap::Arg::new("username")
                .long("username")
                .value_name("USER")
                .help("Username (skip prompt)"),
        )
        .arg(
            clap::Arg::new("password")
                .long("password")
                .value_name("PASSWORD")
                .help("Password (skip hidden prompt; use only for local testing)"),
        )
        .get_matches();

    let host = matches.get_one::<String>("host").unwrap();
    let port = matches.get_one::<String>("port").unwrap();
    let tls = matches.get_flag("tls");
    let log_enabled = matches.get_flag("log");

    let scheme = if tls { "wss" } else { "ws" };
    let uri = format!("{}://{}:{}", scheme, host, port);

    let username = match matches.get_one::<String>("username") {
        Some(user) if !user.trim().is_empty() => user.trim().to_string(),
        Some(_) => {
            println!("Empty username provided, using 'anon'.");
            "anon".to_string()
        }
        None => {
            let u = read_stdin_line("username: ")?;
            if u.is_empty() {
                println!("Empty username provided, using 'anon'.");
                "anon".to_string()
            } else {
                u
            }
        }
    };

    let password = if let Some(password) = matches.get_one::<String>("password") {
        password.clone()
    } else {
        println!("Password input is hidden. Type it and press Enter.");
        match rpassword::prompt_password("password: ") {
            Ok(password) => password,
            Err(err) => {
                eprintln!(
                    "Hidden password prompt unavailable: {}. Falling back to visible input.",
                    err
                );
                read_stdin_line("password (visible): ")?
            }
        }
    };
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
        let (_, user_map) = parse_users_payload(&resp_val["users"]);
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
            screen_active: false,
            screen_room: None,
            screen_session: None,
            pending_screen_frames: HashMap::new(),
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
                    if let Ok(data) =
                        serde_json::from_str::<HashMap<String, serde_json::Value>>(&text)
                    {
                        // Process message and update state
                        let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
                        let ts = data.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
                        match t {
                            "msg" => {
                                let ch =
                                    data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                                let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
                                let encrypted = match general_purpose::STANDARD.decode(c) {
                                    Ok(bytes) => bytes,
                                    Err(_) => continue,
                                };
                                if let Ok(content) =
                                    dec_bytes(&state_clone.lock().await.ckey(ch), &encrypted)
                                {
                                    let mut state = state_clone.lock().await;
                                    state.message_history.push(DisplayedMessage {
                                        time: format_time(ts),
                                        text: String::from_utf8(content)
                                            .unwrap_or_else(|_| "[Invalid UTF-8]".to_string()),
                                        msg_type: MessageType::Msg,
                                        user: Some(u.to_string()),
                                        channel: Some(ch.to_string()),
                                    });
                                    // Keep only last 100 messages
                                    if state.message_history.len() > 100 {
                                        state.message_history.remove(0);
                                    }
                                }
                            }
                            "img" => {
                                let ch =
                                    data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                                let a = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
                                let encrypted = match general_purpose::STANDARD.decode(a) {
                                    Ok(bytes) => bytes,
                                    Err(_) => continue,
                                };
                                let ascii_art = img_to_ascii(&encrypted, 70);
                                {
                                    let mut state = state_clone.lock().await;
                                    state.message_history.push(DisplayedMessage {
                                        time: format_time(ts),
                                        text: format!(
                                            "{} sent an image:\n{}",
                                            data.get("u").and_then(|v| v.as_str()).unwrap_or("?"),
                                            ascii_art
                                        ),
                                        msg_type: MessageType::Img,
                                        user: Some(u.to_string()),
                                        channel: Some(ch.to_string()),
                                    });
                                    // Keep only last 100 messages
                                    if state.message_history.len() > 100 {
                                        state.message_history.remove(0);
                                    }
                                }
                            }
                            "sys" => {
                                let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
                                let mut state = state_clone.lock().await;
                                state.message_history.push(DisplayedMessage {
                                    time: format_time(ts),
                                    text: m.to_string(),
                                    msg_type: MessageType::Sys,
                                    user: None,
                                    channel: None,
                                });
                                if state.message_history.len() > 100 {
                                    state.message_history.remove(0);
                                }
                            }
                            "dm" => {
                                let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                                let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
                                let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
                                let encrypted = match general_purpose::STANDARD.decode(c) {
                                    Ok(bytes) => bytes,
                                    Err(_) => continue,
                                };
                                let mut state_lock = state_clone.lock().await;

                                if let Some(pk) = data.get("pk").and_then(|v| v.as_str()) {
                                    if !pk.is_empty() {
                                        state_lock.users.insert(frm.to_string(), pk.to_string());
                                        // Invalidate cached key to avoid decrypting with a stale peer key.
                                        state_lock.dm_keys.remove(frm);
                                    }
                                }

                                let peer = if frm == state_lock.me { to } else { frm };
                                if let Ok(dm_key) = state_lock.dmkey(peer) {
                                    if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
                                        let arrow =
                                            if frm == state_lock.me { "→" } else { "←" };
                                        state_lock.message_history.push(DisplayedMessage {
                                            time: format_time(ts),
                                            text: format!(
                                                "{} {} {}",
                                                frm,
                                                arrow,
                                                String::from_utf8(content).unwrap_or_else(|_| {
                                                    "[Invalid UTF-8]".to_string()
                                                })
                                            ),
                                            msg_type: MessageType::Dm,
                                            user: Some(frm.to_string()),
                                            channel: None,
                                        });
                                        if state_lock.message_history.len() > 100 {
                                            state_lock.message_history.remove(0);
                                        }
                                    }
                                }
                            }
                            "users" => {
                                let mut state = state_clone.lock().await;
                                let empty_users = serde_json::json!([]);
                                let users_val = data.get("users").unwrap_or(&empty_users);
                                let (_names, key_map) = parse_users_payload(users_val);
                                for (name, pk) in key_map {
                                    state.users.insert(name, pk);
                                }
                                state.message_history.push(DisplayedMessage {
                                    time: format_time(ts),
                                    text: format_user_roster(users_val),
                                    msg_type: MessageType::Sys,
                                    user: None,
                                    channel: None,
                                });
                                if state.message_history.len() > 100 {
                                    state.message_history.remove(0);
                                }
                            }
                            "joined" => {
                                let ch =
                                    data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                let mut state = state_clone.lock().await;
                                state.ch = ch.to_string();
                                state.chs.insert(ch.to_string(), true);
                                state.message_history.push(DisplayedMessage {
                                    time: format_time(ts),
                                    text: format!("→ #{}", ch),
                                    msg_type: MessageType::Sys,
                                    user: None,
                                    channel: None,
                                });
                                if state.message_history.len() > 100 {
                                    state.message_history.remove(0);
                                }
                                // Send request for history
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "join",
                                        "ch": ch,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                );
                            }
                            "info" => {
                                let mut state = state_clone.lock().await;
                                let chs: Vec<String> = serde_json::from_value(
                                    data.get("chs").unwrap_or(&serde_json::json!([])).clone(),
                                )
                                .unwrap_or_default();
                                let online =
                                    data.get("online").and_then(|v| v.as_u64()).unwrap_or(0);
                                state.message_history.push(DisplayedMessage {
                                    time: format_time(ts),
                                    text: format!(
                                        "Channels: {} | Online: {}",
                                        chs.join(", "),
                                        online
                                    ),
                                    msg_type: MessageType::Sys,
                                    user: None,
                                    channel: None,
                                });
                                if state.message_history.len() > 100 {
                                    state.message_history.remove(0);
                                }
                            }
                            "vdata" => {
                                let from = data.get("from").and_then(|v| v.as_str()).unwrap_or("");
                                let payload = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
                                let playback_tx = {
                                    let state = state_clone.lock().await;
                                    if from == state.me {
                                        None
                                    } else {
                                        state
                                            .voice_session
                                            .as_ref()
                                            .map(|session| session.event_tx.clone())
                                    }
                                };
                                if let (Some(tx), Some(frame)) =
                                    (playback_tx, decode_voice_frame(payload))
                                {
                                    let _ = tx.send(VoiceEvent::Playback(frame));
                                }
                            }
                            "sdata" => {
                                let from = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                                let room = data
                                    .get("r")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("general")
                                    .to_string();
                                let codec = data
                                    .get("codec")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("raw")
                                    .to_string();
                                let width =
                                    data.get("w").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let height =
                                    data.get("h").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let seq = data.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
                                let chunk_idx =
                                    data.get("chunk").and_then(|v| v.as_u64()).unwrap_or(0);
                                let total_chunks =
                                    data.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                                let keyframe =
                                    data.get("kf").and_then(|v| v.as_bool()).unwrap_or(false);
                                let payload = data.get("a").and_then(|v| v.as_str()).unwrap_or("");

                                if payload.is_empty() || total_chunks == 0 {
                                    continue;
                                }
                                let total_chunks = total_chunks as usize;
                                let chunk_idx = chunk_idx as usize;
                                if total_chunks > 256 || chunk_idx >= total_chunks {
                                    continue;
                                }

                                let frame_key = format!("{}:{}:{}", from, room, seq);
                                let completed = {
                                    let mut state = state_clone.lock().await;
                                    if from == state.me {
                                        continue;
                                    }
                                    cleanup_pending_screen_frames(&mut state.pending_screen_frames);

                                    let entry = state
                                        .pending_screen_frames
                                        .entry(frame_key.clone())
                                        .or_insert_with(|| PendingScreenFrame {
                                            codec: codec.clone(),
                                            width,
                                            height,
                                            total: total_chunks,
                                            keyframe,
                                            created_at: unix_now_secs(),
                                            chunks: vec![None; total_chunks],
                                        });

                                    if entry.total != total_chunks
                                        || entry.codec != codec
                                        || entry.width != width
                                        || entry.height != height
                                    {
                                        *entry = PendingScreenFrame {
                                            codec: codec.clone(),
                                            width,
                                            height,
                                            total: total_chunks,
                                            keyframe,
                                            created_at: unix_now_secs(),
                                            chunks: vec![None; total_chunks],
                                        };
                                    }

                                    if entry.chunks[chunk_idx].is_none() {
                                        entry.chunks[chunk_idx] = Some(payload.to_string());
                                    }
                                    entry.chunks.iter().all(|part| part.is_some())
                                };

                                if !completed {
                                    continue;
                                }

                                let pending = {
                                    let mut state = state_clone.lock().await;
                                    state.pending_screen_frames.remove(&frame_key)
                                };
                                let Some(pending) = pending else {
                                    continue;
                                };

                                let mut bytes = Vec::new();
                                let mut decode_ok = true;
                                for chunk in pending.chunks {
                                    let Some(chunk_b64) = chunk else {
                                        decode_ok = false;
                                        break;
                                    };
                                    match general_purpose::STANDARD.decode(chunk_b64) {
                                        Ok(decoded) => bytes.extend_from_slice(&decoded),
                                        Err(_) => {
                                            decode_ok = false;
                                            break;
                                        }
                                    }
                                }
                                if !decode_ok || bytes.is_empty() {
                                    continue;
                                }

                                // Throttle textual preview updates to keep terminal usable.
                                if !(pending.keyframe || seq % 6 == 0) {
                                    continue;
                                }

                                let preview = if pending.codec == "jpeg" {
                                    img_to_ascii(&bytes, SCREEN_PREVIEW_WIDTH)
                                } else {
                                    format!(
                                        "[{} frame | {} bytes | {} chunks]",
                                        pending.codec,
                                        bytes.len(),
                                        pending.total
                                    )
                                };

                                let mut state = state_clone.lock().await;
                                state.message_history.push(DisplayedMessage {
                                    time: format_time(ts),
                                    text: format!(
                                        "🖥 {} streaming #{} [{} {}x{} frame={}{}]\n{}",
                                        from,
                                        room,
                                        pending.codec,
                                        pending.width,
                                        pending.height,
                                        seq,
                                        if pending.keyframe { " keyframe" } else { "" },
                                        preview
                                    ),
                                    msg_type: MessageType::Sdata,
                                    user: Some(from.to_string()),
                                    channel: Some(room),
                                });
                                if state.message_history.len() > 100 {
                                    state.message_history.remove(0);
                                }
                            }
                            "status_update" => {
                                // Handle user status updates and refresh user list
                                let mut state = state_clone.lock().await;
                                let empty_users = serde_json::json!([]);
                                let users_val = data.get("users").unwrap_or(&empty_users);
                                let (_, key_map) = parse_users_payload(users_val);

                                // Update user keys cache
                                for (name, pk) in key_map {
                                    state.users.insert(name, pk);
                                }

                                // Optionally show notification when someone changes status
                                if let Some(user) = data.get("user").and_then(|v| v.as_str()) {
                                    if let Some(msg) = data.get("msg").and_then(|v| v.as_str()) {
                                        if !msg.is_empty() {
                                            state.message_history.push(DisplayedMessage {
                                                time: format_time(ts),
                                                text: format!("📍 {} → {}", user, msg),
                                                msg_type: MessageType::Sys,
                                                user: None,
                                                channel: None,
                                            });
                                            if state.message_history.len() > 100 {
                                                state.message_history.remove(0);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
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
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "join",
                                        "ch": ch,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
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
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "dm",
                                        "to": target,
                                        "c": encoded,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
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
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "msg",
                                        "ch": ch,
                                        "c": encoded,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                );
                            }
                        }
                        "/users" => {
                            let _ = state.ws_tx.send(
                                serde_json::json!({
                                    "t": "users",
                                    "ts": SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                                .to_string(),
                            );
                        }
                        "/channels" => {
                            let _ = state.ws_tx.send(
                                serde_json::json!({
                                    "t": "info",
                                    "ts": SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                                .to_string(),
                            );
                        }
                        "/voice" => {
                            if state.voice_session.is_some() {
                                if let Some(session) = state.voice_session.take() {
                                    state.voice_active = false;
                                    let _ = state.ws_tx.send(
                                        serde_json::json!({
                                            "t": "vleave",
                                            "r": session.room,
                                            "ts": SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs()
                                        })
                                        .to_string(),
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
                                        let _ = ws_tx.send(
                                            serde_json::json!({
                                                "t": "vjoin",
                                                "r": room,
                                                "ts": SystemTime::now()
                                                    .duration_since(UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_secs()
                                            })
                                            .to_string(),
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
                        "/screen" => {
                            let args_trimmed = args.trim();
                            let mut words = args_trimmed.split_whitespace();
                            let action = words.next().unwrap_or("");

                            if action.eq_ignore_ascii_case("stop") {
                                let mut room_to_leave = state.screen_room.take();
                                if let Some(session) = state.screen_session.take() {
                                    if room_to_leave.is_none() {
                                        room_to_leave = Some(session.room.clone());
                                    }
                                    stop_screen_session(session);
                                }

                                if let Some(room) = room_to_leave {
                                    let _ = state.ws_tx.send(
                                        serde_json::json!({
                                            "t": "sleave",
                                            "r": room,
                                            "ts": SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs()
                                        })
                                        .to_string(),
                                    );
                                    println!("Screen share stopped");
                                } else {
                                    state.log(
                                        "INFO",
                                        "No active screen stream. Use /screen start [room]",
                                    );
                                }
                                state.screen_active = false;
                                continue;
                            }

                            let viewing_only = action.eq_ignore_ascii_case("view");
                            let room_hint = if action.eq_ignore_ascii_case("start")
                                || action.eq_ignore_ascii_case("view")
                            {
                                words.next().unwrap_or("")
                            } else {
                                args_trimmed
                            };
                            let room = if room_hint.is_empty() {
                                state.ch.clone()
                            } else {
                                normalize_channel(room_hint).unwrap_or_else(|| state.ch.clone())
                            };

                            let mut room_to_leave = state.screen_room.clone();
                            if let Some(session) = state.screen_session.take() {
                                room_to_leave = Some(session.room.clone());
                                stop_screen_session(session);
                            }

                            if let Some(current_room) = room_to_leave {
                                if current_room != room {
                                    let _ = state.ws_tx.send(
                                        serde_json::json!({
                                            "t": "sleave",
                                            "r": current_room,
                                            "ts": SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs()
                                        })
                                        .to_string(),
                                    );
                                }
                            }

                            state.screen_room = Some(room.clone());
                            let _ = state.ws_tx.send(
                                serde_json::json!({
                                    "t": "sjoin",
                                    "r": room,
                                    "ts": SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                                .to_string(),
                            );

                            if viewing_only {
                                state.screen_active = false;
                                println!("Subscribed to screen stream in #{}", room);
                            } else {
                                let ws_tx = state.ws_tx.clone();
                                match start_screen_session(room.clone(), ws_tx.clone()) {
                                    Ok(session) => {
                                        state.screen_active = true;
                                        state.screen_session = Some(session);
                                        println!("Screen share started in #{}", room);
                                    }
                                    Err(e) => {
                                        state.screen_active = false;
                                        state.screen_room = None;
                                        let _ = ws_tx.send(
                                            serde_json::json!({
                                                "t": "sleave",
                                                "r": room,
                                                "ts": SystemTime::now()
                                                    .duration_since(UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_secs()
                                            })
                                            .to_string(),
                                        );
                                        state.log("WARN", &format!("Screen share failed: {}", e));
                                        eprintln!("Screen share failed: {}", e);
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
                        "/status" => {
                            let status_msg = args.trim();
                            if status_msg.is_empty() {
                                state.log(
                                    "INFO",
                                    "Usage: /status <message> (e.g., 'In a meeting', 'AFK')",
                                );
                            } else {
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "status",
                                        "msg": status_msg,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                );
                                state.log("INFO", &format!("Status updated to: {}", status_msg));
                            }
                        }
                        "/help" => {
                            state.log("INFO", "Available commands: /join, /dm, /me, /users, /channels, /voice [room], /screen [room]|start [room]|view [room]|stop, /status <msg>, /clear, /edit, /help, /quit");
                        }
                        "/edit" => {
                            // Placeholder for edit
                            state.log("INFO", "Edit command not yet implemented in Rust client");
                        }
                        "/quit" | "/exit" | "/q" => {
                            let mut room_to_leave = state.screen_room.take();
                            if let Some(session) = state.screen_session.take() {
                                if room_to_leave.is_none() {
                                    room_to_leave = Some(session.room.clone());
                                }
                                stop_screen_session(session);
                            }

                            if let Some(session) = state.voice_session.take() {
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "vleave",
                                        "r": session.room,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                );
                                let _ = session.event_tx.send(VoiceEvent::Stop);
                                let _ = session.task.join();
                            }
                            if let Some(room) = room_to_leave {
                                let _ = state.ws_tx.send(
                                    serde_json::json!({
                                        "t": "sleave",
                                        "r": room,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                );
                            }
                            state.voice_active = false;
                            state.screen_active = false;
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
                    let msg = serde_json::json!({
                        "t": "msg",
                        "ch": channel,
                        "c": encoded,
                        "ts": SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                                        .as_secs()
                    })
                    .to_string();
                    let _ = state_clone2.lock().await.ws_tx.send(msg);
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
