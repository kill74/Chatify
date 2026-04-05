use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[derive(Parser)]
#[command(name = "clicord-server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    #[arg(long, default_value_t = 8765)]
    port: u16,
    #[arg(long)]
    log: bool,
}

const HISTORY_CAP: usize = 50;
const MAX_CONTROL_BYTES: usize = 16_000;
const MAX_SCREEN_BYTES: usize = 512_000;
const SNAPSHOT_VERSION: u32 = 1;
const AUTOSAVE_INTERVAL_SECS: u64 = 5;

#[derive(Debug, Serialize, Deserialize)]
struct PersistedState {
    version: u32,
    saved_at: f64,
    channels: HashMap<String, Vec<Value>>,
}

impl PersistedState {
    fn new(channels: HashMap<String, Vec<Value>>) -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            saved_at: now(),
            channels,
        }
    }
}

#[derive(Clone)]
struct Channel {
    history: Arc<RwLock<VecDeque<Value>>>,
    tx: broadcast::Sender<String>,
}

impl Channel {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))),
            tx,
        }
    }
    async fn push(&self, entry: Value) {
        let mut h = self.history.write().await;
        if h.len() >= HISTORY_CAP {
            h.pop_front();
        }
        h.push_back(entry);
    }
    async fn hist(&self) -> Vec<Value> {
        self.history.read().await.iter().cloned().collect()
    }
}

/// User presence information
#[derive(Clone, Debug)]
struct UserPresence {
    /// Custom status message set by the user
    status_message: Option<String>,
    /// Current online state: "online", "idle", "offline"
    state: String,
    /// Timestamp of last activity (for idle detection)
    last_activity: f64,
    /// Public key for encryption
    public_key: Option<String>,
}

impl UserPresence {
    fn new(public_key: Option<String>) -> Self {
        Self {
            status_message: None,
            state: "online".to_string(),
            last_activity: now(),
            public_key,
        }
    }

    fn update_activity(&mut self) {
        self.last_activity = now();
        if self.state == "idle" {
            self.state = "online".to_string();
        }
    }

    fn to_json(&self, username: &str) -> Value {
        serde_json::json!({
            "u": username,
            "state": self.state,
            "status": self.status_message,
            "pk": self.public_key.as_ref().unwrap_or(&String::new())
        })
    }
}

struct State {
    channels: DashMap<String, Channel>,
    voice: DashMap<String, broadcast::Sender<String>>,
    screens: DashMap<String, broadcast::Sender<String>>,
    /// Maps username -> UserPresence
    user_presence: DashMap<String, UserPresence>,
    user_statuses: DashMap<String, Value>,
    user_pubkeys: DashMap<String, String>,
    file_transfers: DashMap<String, Value>,
    log_enabled: bool,
}

impl State {
    fn new(log_enabled: bool) -> Arc<Self> {
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            screens: DashMap::new(),
            user_presence: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            file_transfers: DashMap::new(),
            log_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    fn chan(&self, name: &str) -> Channel {
        self.channels
            .entry(name.into())
            .or_insert_with(Channel::new)
            .clone()
    }

    fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.voice
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone()
    }

    fn screen_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.screens
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(64);
                tx
            })
            .clone()
    }

    /// Get the count of online users
    fn online_count(&self) -> usize {
        self.user_presence.len()
    }

    /// Get all channels as JSON array (excluding DM channels)
    fn channels_json(&self) -> Value {
        Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| Value::String(e.key().clone()))
                .collect(),
        )
    }

    /// Get online users with their presence info as JSON array
    fn users_with_keys_json(&self) -> Value {
        Value::Array(
            self.user_presence
                .iter()
                .map(|entry| entry.value().to_json(entry.key()))
                .collect(),
        )
    }

    /// Update user activity timestamp
    fn touch_user(&self, username: &str) {
        if let Some(mut presence) = self.user_presence.get_mut(username) {
            presence.update_activity();
        }
    }
}

/// Get current Unix timestamp as f64
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Hash a password using SHA256
#[allow(dead_code)]
fn hash_pw(pw: &str) -> String {
    hex::encode(Sha256::digest(pw.as_bytes()))
}

/// Sanitize a channel name: lowercase, remove special chars, limit to 32 chars
fn safe_ch(raw: &str) -> String {
    let s: String = raw
        .to_lowercase()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if s.is_empty() {
        "general".into()
    } else {
        s
    }
}

/// Create a system message JSON string
fn sys(text: &str) -> String {
    serde_json::json!({"t":"sys","m":text,"ts":now()}).to_string()
}

/// Get current time formatted as HH:MM:SS
fn hms() -> String {
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        (s % 86400) / 3600,
        (s % 3600) / 60,
        s % 60
    )
}

fn sanitize_cache_component(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".into()
    } else {
        cleaned
    }
}

fn cache_file_path(args: &Args) -> PathBuf {
    let base_cache = if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        std::env::temp_dir()
    };

    let host = sanitize_cache_component(&args.host);
    let filename = format!("server-state-{}-{}.json", host, args.port);
    base_cache.join("chatify").join(filename)
}

async fn build_snapshot(state: &Arc<State>) -> PersistedState {
    let channel_entries: Vec<(String, Channel)> = state
        .channels
        .iter()
        .map(|entry| (entry.key().clone(), entry.value().clone()))
        .collect();

    let mut channels = HashMap::with_capacity(channel_entries.len());
    for (name, channel) in channel_entries {
        channels.insert(name, channel.hist().await);
    }

    PersistedState::new(channels)
}

async fn write_snapshot(path: &Path, snapshot: &PersistedState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let payload = serde_json::to_vec(snapshot)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

async fn persist_state_to_disk(state: &Arc<State>, path: &Path) -> io::Result<()> {
    let snapshot = build_snapshot(state).await;
    write_snapshot(path, &snapshot).await
}

async fn restore_state_from_disk(state: &Arc<State>, path: &Path) {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(data) => data,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(err) => {
            eprintln!("Failed to read cache file {}: {}", path.display(), err);
            return;
        }
    };

    let snapshot: PersistedState = match serde_json::from_str(&raw) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            eprintln!("Failed to parse cache file {}: {}", path.display(), err);
            return;
        }
    };

    if snapshot.version != SNAPSHOT_VERSION {
        eprintln!(
            "Ignoring cache file {} due to version mismatch (found {}, expected {})",
            path.display(),
            snapshot.version,
            SNAPSHOT_VERSION
        );
        return;
    }

    for (name, mut history) in snapshot.channels {
        if history.len() > HISTORY_CAP {
            let keep_from = history.len().saturating_sub(HISTORY_CAP);
            history.drain(0..keep_from);
        }

        let channel = state.chan(&name);
        let mut stored = channel.history.write().await;
        stored.clear();
        stored.extend(history.into_iter());
    }

    log(
        state,
        &format!("Restored server state cache from {}", path.display()),
    );
}

async fn autosave_loop(state: Arc<State>, path: PathBuf) {
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(AUTOSAVE_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;

    loop {
        interval.tick().await;
        if let Err(err) = persist_state_to_disk(&state, &path).await {
            log(
                &state,
                &format!("State autosave failed at {}: {}", path.display(), err),
            );
        }
    }
}

/// Log a message with timestamp if logging is enabled
fn log(state: &State, msg: &str) {
    if state.log_enabled {
        println!("[{}] {}", hms(), msg);
    }
}

/// Send a system message to all channels
async fn broadcast_system_msg(state: &Arc<State>, msg: &str) {
    let sys_msg = sys(msg);
    for e in state.channels.iter() {
        let _ = e.tx.send(sys_msg.clone());
    }
}

fn spawn_broadcast_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if out_tx.send(m).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {}
            }
        }
    })
}

/// Create JSON response for successful connection
fn create_ok_response(username: &str, state: &Arc<State>, hist: Vec<Value>) -> String {
    serde_json::json!({
        "t": "ok",
        "u": username,
        "users": state.users_with_keys_json(),
        "channels": state.channels_json(),
        "hist": hist
    })
    .to_string()
}

/// Main client connection handler
async fn handle(stream: TcpStream, _addr: SocketAddr, state: Arc<State>) {
    // Accept WebSocket connection
    let ws = match accept_async(stream).await {
        Ok(w) => w,
        Err(_) => return,
    };
    let (mut sink, mut stream) = ws.split();

    // Read initial authentication message
    let raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        _ => return,
    };
    let d: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Extract username from message
    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .unwrap_or("anon")
        .to_string();

    // Extract public key and initialize user presence
    let pk = d.get("pk").and_then(|v| v.as_str()).map(String::from);
    let presence = UserPresence::new(pk.clone());

    // Legacy status support (for backward compatibility)
    let status = d
        .get("status")
        .cloned()
        .unwrap_or(serde_json::json!({"text": "Online", "emoji": "🟢"}));
    state.user_statuses.insert(username.clone(), status);

    if let Some(ref public_key) = pk {
        if !public_key.is_empty() {
            state
                .user_pubkeys
                .insert(username.clone(), public_key.clone());
        }
    }

    // Insert user presence
    state.user_presence.insert(username.clone(), presence);

    // Get general channel and send welcome response
    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let hist = general.hist().await;

    let ok = create_ok_response(&username, &state, hist);
    if sink.send(Message::Text(ok)).await.is_err() {
        return;
    }

    // Announce user join
    broadcast_system_msg(&state, &format!("→ {} joined", username)).await;
    log(&state, &format!("+ {}", username));

    // Create channel for sending messages to client
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Track per-channel forwarders to avoid duplicate subscriptions on repeated joins.
    let mut channel_forwarders: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    channel_forwarders.insert(
        "general".to_string(),
        spawn_broadcast_forwarder(gen_rx, out_tx.clone()),
    );

    // Spawn task to send queued messages to WebSocket
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() {
                break;
            }
        }
    });

    let mut voice_room: Option<String> = None;
    let mut voice_forwarder: Option<tokio::task::JoinHandle<()>> = None;
    let mut screen_sub_room: Option<String> = None;
    let mut screen_share_room: Option<String> = None;
    let mut screen_forwarder: Option<tokio::task::JoinHandle<()>> = None;

    // Main message handling loop
    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                log(&state, &format!("ws recv error for {}: {}", username, e));
                break;
            }
            None => break,
        };
        let raw = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        // Validate message size. Screen payloads are allowed to be larger than control messages.
        if raw.len() > MAX_SCREEN_BYTES {
            continue;
        }

        // Parse message
        let d: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Update user activity timestamp for idle detection
        state.touch_user(&username);

        let t = d["t"].as_str().unwrap_or("");

        if t != "sdata" && raw.len() > MAX_CONTROL_BYTES {
            continue;
        }

        // Route message to appropriate handlers
        match t {
            "msg" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let c = d["c"].as_str().unwrap_or("").to_string();
                if c.is_empty() {
                    continue;
                }
                let entry = serde_json::json!({"t":"msg","ch":ch,"u":username,"c":c,"ts":now()});
                let chan = state.chan(&ch);
                chan.push(entry.clone()).await;
                let _ = chan.tx.send(entry.to_string());
            }
            "img" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let a = d["a"].as_str().unwrap_or("").to_string();
                if a.is_empty() {
                    continue;
                }
                let _ = state.chan(&ch).tx.send(
                    serde_json::json!({"t":"img","ch":ch,"u":username,"a":a,"ts":now()})
                        .to_string(),
                );
            }
            "dm" => {
                let target = d["to"].as_str().unwrap_or("").to_string();
                let c = d["c"].as_str().unwrap_or("").to_string();
                if c.is_empty() || target.is_empty() {
                    continue;
                }
                let sender_pk = state
                    .user_pubkeys
                    .get(&username)
                    .map(|v| v.value().clone())
                    .unwrap_or_default();
                let p = serde_json::json!({"t":"dm","from":username,"to":target,"c":c,"pk":sender_pk,"ts":now()})
                    .to_string();
                let _ = state.chan(&format!("__dm__{}", target)).tx.send(p.clone());
                let _ = state.chan(&format!("__dm__{}", username)).tx.send(p);
            }
            "join" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let chan = state.chan(&ch);
                let hist = chan.hist().await;
                if !channel_forwarders.contains_key(&ch) {
                    let rx = chan.tx.subscribe();
                    let handle = spawn_broadcast_forwarder(rx, out_tx.clone());
                    channel_forwarders.insert(ch.clone(), handle);
                }
                let _ =
                    out_tx.send(serde_json::json!({"t":"joined","ch":ch,"hist":hist}).to_string());
                let _ = chan.tx.send(sys(&format!("→ {} joined #{}", username, ch)));
            }
            "users" => {
                let _ = out_tx.send(
                    serde_json::json!({"t":"users","users":state.users_with_keys_json()})
                        .to_string(),
                );
            }
            "info" => {
                let chs: Vec<String> = state
                    .channels
                    .iter()
                    .filter(|e| !e.key().starts_with("__dm__"))
                    .map(|e| e.key().clone())
                    .collect();
                let _ = out_tx.send(
                    serde_json::json!({"t":"info","chs":chs,"online":state.online_count()})
                        .to_string(),
                );
            }
            "vjoin" => {
                let room = safe_ch(d["r"].as_str().unwrap_or("general"));
                if voice_room.as_deref() == Some(room.as_str()) {
                    continue;
                }
                if let Some(ref old_room) = voice_room {
                    let _ = state
                        .chan(old_room)
                        .tx
                        .send(sys(&format!("🎙 {} left voice #{}", username, old_room)));
                }
                if let Some(handle) = voice_forwarder.take() {
                    handle.abort();
                }

                let vtx = state.voice_tx(&room);
                let vrx = vtx.subscribe();
                voice_forwarder = Some(spawn_broadcast_forwarder(vrx, out_tx.clone()));
                voice_room = Some(room.clone());
                let _ = state
                    .chan(&room)
                    .tx
                    .send(sys(&format!("🎙 {} joined voice #{}", username, room)));
            }
            "vleave" => {
                if let Some(handle) = voice_forwarder.take() {
                    handle.abort();
                }
                if let Some(ref room) = voice_room.take() {
                    let _ = state
                        .chan(room)
                        .tx
                        .send(sys(&format!("🎙 {} left voice #{}", username, room)));
                }
            }
            "vdata" => {
                let a = d["a"].as_str().unwrap_or("").to_string();
                if a.is_empty() {
                    continue;
                }
                if let Some(ref room) = voice_room {
                    if let Some(vtx) = state.voice.get(room) {
                        let _ = vtx.send(
                            serde_json::json!({"t":"vdata","from":username,"a":a}).to_string(),
                        );
                    }
                }
            }
            "sjoin" => {
                let room = safe_ch(d["r"].as_str().unwrap_or("general"));
                let mode = d.get("mode").and_then(|v| v.as_str()).unwrap_or("share");
                let viewing_only = mode.eq_ignore_ascii_case("view");

                let current_sub = screen_sub_room.as_deref();
                let is_currently_sharing = screen_share_room.is_some();
                if current_sub == Some(room.as_str())
                    && ((viewing_only && !is_currently_sharing)
                        || (!viewing_only && is_currently_sharing))
                {
                    continue;
                }

                if let Some(ref old_room) = screen_share_room.take() {
                    let _ = state.chan(old_room).tx.send(sys(&format!(
                        "🖥 {} stopped screen share #{}",
                        username, old_room
                    )));
                }

                if screen_sub_room.as_deref() != Some(room.as_str()) {
                    if let Some(handle) = screen_forwarder.take() {
                        handle.abort();
                    }
                    let stx = state.screen_tx(&room);
                    let srx = stx.subscribe();
                    screen_forwarder = Some(spawn_broadcast_forwarder(srx, out_tx.clone()));
                    screen_sub_room = Some(room.clone());
                }

                if !viewing_only {
                    screen_share_room = Some(room.clone());
                    let _ = state.chan(&room).tx.send(sys(&format!(
                        "🖥 {} started screen share #{}",
                        username, room
                    )));
                }
            }
            "sleave" => {
                if let Some(handle) = screen_forwarder.take() {
                    handle.abort();
                }
                screen_sub_room = None;
                if let Some(ref room) = screen_share_room.take() {
                    let _ = state.chan(room).tx.send(sys(&format!(
                        "🖥 {} stopped screen share #{}",
                        username, room
                    )));
                }
            }
            "sdata" => {
                let a = d["a"].as_str().unwrap_or("").to_string();
                if a.is_empty() {
                    continue;
                }
                if let Some(ref room) = screen_share_room {
                    if let Some(stx) = state.screens.get(room) {
                        let codec = d["codec"].as_str().unwrap_or("raw");
                        let seq = d["seq"].as_u64().unwrap_or(0);
                        let chunk = d["chunk"].as_u64().unwrap_or(0);
                        let total = d["total"].as_u64().unwrap_or(1);
                        let width = d["w"].as_u64().unwrap_or(0);
                        let height = d["h"].as_u64().unwrap_or(0);
                        let keyframe = d["kf"].as_bool().unwrap_or(false);

                        let _ = stx.send(
                            serde_json::json!({
                                "t": "sdata",
                                "from": username,
                                "r": room,
                                "a": a,
                                "codec": codec,
                                "seq": seq,
                                "chunk": chunk,
                                "total": total,
                                "w": width,
                                "h": height,
                                "kf": keyframe,
                                "ts": now()
                            })
                            .to_string(),
                        );
                    }
                }
            }
            "ping" => {
                let _ = out_tx.send(r#"{"t":"pong"}"#.into());
            }
            "edit" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let old_text = d["old_text"].as_str().unwrap_or("").to_string();
                let new_text = d["new_text"].as_str().unwrap_or("").to_string();
                if old_text.is_empty() || new_text.is_empty() {
                    continue;
                }
                let chan = state.chan(&ch);
                let mut h = chan.history.write().await;
                let username_clone = username.clone();
                let old_text_clone = old_text.clone();
                if let Some(pos) = h.iter().rposition(|m| {
                    m.get("t") == Some(&Value::from("msg"))
                        && m.get("u") == Some(&Value::from(username_clone.clone()))
                        && m.get("c") == Some(&Value::from(old_text_clone.clone()))
                }) {
                    h[pos]["c"] = Value::from(new_text.clone());
                    h[pos]["ts"] = Value::from(now());
                }
                let edit_msg = serde_json::json!({
                    "t": "edit",
                    "ch": ch,
                    "u": username,
                    "old_text": old_text,
                    "new_text": new_text,
                    "ts": now()
                })
                .to_string();
                let _ = chan.tx.send(edit_msg);
            }
            "file_meta" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let filename = d["filename"].as_str().unwrap_or("unknown").to_string();
                let size = d["size"].as_u64().unwrap_or(0);
                let file_id = d["file_id"]
                    .as_str()
                    .unwrap_or(&format!("{}_{}", username, now()))
                    .to_string();
                state.file_transfers.insert(file_id.clone(), d.clone());
                let file_announce = serde_json::json!({
                    "t": "file_meta",
                    "from": username,
                    "filename": filename,
                    "size": size,
                    "file_id": file_id,
                    "ch": ch,
                    "ts": now()
                })
                .to_string();
                let _ = state.chan(&ch).tx.send(file_announce);
            }
            "file_chunk" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let file_id = d["file_id"].as_str().unwrap_or("").to_string();
                let chunk_data = d["data"].as_str().unwrap_or("").to_string();
                let index = d["index"].as_u64().unwrap_or(0);
                let chunk_msg = serde_json::json!({
                    "t": "file_chunk",
                    "from": username,
                    "file_id": file_id,
                    "data": chunk_data,
                    "index": index,
                    "ch": ch,
                    "ts": now()
                })
                .to_string();
                let _ = state.chan(&ch).tx.send(chunk_msg);
            }
            "status" => {
                // Handle custom status message update
                let status_msg = d.get("msg").and_then(|v| v.as_str()).map(String::from);

                if let Some(mut presence) = state.user_presence.get_mut(&username) {
                    presence.status_message = status_msg.clone();
                }

                // Legacy status support
                if let Some(status_val) = d.get("status") {
                    state
                        .user_statuses
                        .insert(username.clone(), status_val.clone());
                }

                // Broadcast status update to all channels
                let status_update = serde_json::json!({
                    "t": "status_update",
                    "user": username,
                    "msg": status_msg,
                    "users": state.users_with_keys_json()
                })
                .to_string();

                for chan_entry in state.channels.iter() {
                    let _ = chan_entry.tx.send(status_update.clone());
                }
            }
            "reaction" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let emoji = d["emoji"].as_str().unwrap_or("👍").to_string();
                let msg_id = d["msg_id"].as_str().unwrap_or("unknown").to_string();
                let reaction_msg = serde_json::json!({
                    "t": "reaction",
                    "user": username,
                    "emoji": emoji,
                    "msg_id": msg_id,
                    "ch": ch,
                    "ts": now()
                })
                .to_string();
                let _ = state.chan(&ch).tx.send(reaction_msg);
            }
            _ => {}
        }
    }

    // User disconnected - cleanup
    if let Some(handle) = voice_forwarder.take() {
        handle.abort();
    }
    if let Some(handle) = screen_forwarder.take() {
        handle.abort();
    }
    for (_, handle) in channel_forwarders.drain() {
        handle.abort();
    }
    state.user_presence.remove(&username);
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    if let Some(ref room) = voice_room {
        let _ = state
            .chan(room)
            .tx
            .send(sys(&format!("🎙 {} left voice #{}", username, room)));
    }
    if let Some(ref room) = screen_share_room {
        let _ = state.chan(room).tx.send(sys(&format!(
            "🖥 {} stopped screen share #{}",
            username, room
        )));
    }
    broadcast_system_msg(&state, &format!("✖ {} left", username)).await;
    log(&state, &format!("- {}", username));
}

/// Idle detection background task
/// Checks every 30 seconds for users who haven't been active in 5+ minutes
async fn idle_detection_loop(state: Arc<State>) {
    const IDLE_THRESHOLD: f64 = 300.0; // 5 minutes in seconds
    const CHECK_INTERVAL: u64 = 30; // Check every 30 seconds

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(CHECK_INTERVAL)).await;

        let current_time = now();
        let mut updates = Vec::new();

        // Check each user's last activity
        for mut entry in state.user_presence.iter_mut() {
            let username = entry.key().clone();
            let presence = entry.value_mut();

            let time_since_activity = current_time - presence.last_activity;

            // Mark as idle if inactive for more than threshold and currently online
            if time_since_activity >= IDLE_THRESHOLD && presence.state == "online" {
                presence.state = "idle".to_string();
                updates.push((username.clone(), presence.clone()));
            }
        }

        // Broadcast idle state updates
        for (username, presence) in updates {
            let status_update = serde_json::json!({
                "t": "status_update",
                "user": username,
                "msg": presence.status_message,
                "state": presence.state,
                "users": state.users_with_keys_json()
            })
            .to_string();

            for chan_entry in state.channels.iter() {
                let _ = chan_entry.tx.send(status_update.clone());
            }
        }
    }
}

/// Main entry point
#[tokio::main]
async fn main() {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);
    let cache_path = cache_file_path(&args);

    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("Failed to bind {}: {}", addr, e);
            return;
        }
    };

    let state = State::new(args.log);
    restore_state_from_disk(&state, &cache_path).await;

    // Spawn idle detection background task
    let state_clone = state.clone();
    tokio::spawn(async move {
        idle_detection_loop(state_clone).await;
    });

    // Periodically persist server state so restarts can restore channel history.
    let autosave_state = state.clone();
    let autosave_path = cache_path.clone();
    tokio::spawn(async move {
        autosave_loop(autosave_state, autosave_path).await;
    });

    println!("📡 Chatify running on ws://{}", addr);
    println!("🔒 Encryption: None (testing) | 🛡️  IP Privacy: On");
    println!("⏹️  Press Ctrl+C to stop\n");

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                println!("\n⏻ Shutdown requested, saving server cache...");
                break;
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, addr)) => {
                        let s = state.clone();
                        tokio::spawn(handle(stream, addr, s));
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    if let Err(err) = persist_state_to_disk(&state, &cache_path).await {
        eprintln!(
            "Failed to persist server cache at shutdown ({}): {}",
            cache_path.display(),
            err
        );
    } else {
        log(
            &state,
            &format!("Saved server state cache to {}", cache_path.display()),
        );
    }

    if let Some(parent) = cache_path.parent() {
        println!("💾 State cache directory: {}", parent.display());
    } else {
        println!("💾 State cache file: {}", cache_path.display());
    }

    println!("👋 Chatify server stopped.");
}
