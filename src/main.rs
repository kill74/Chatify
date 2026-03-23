use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
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
const MAX_BYTES: usize = 16_000;

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

struct State {
    channels: DashMap<String, Channel>,
    voice: DashMap<String, broadcast::Sender<String>>,
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

    /// Get the count of online users
    fn online_count(&self) -> usize {
        self.user_statuses.len()
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

    /// Get online users and their public keys as JSON array.
    fn users_with_keys_json(&self) -> Value {
        Value::Array(
            self.user_pubkeys
                .iter()
                .map(|e| serde_json::json!({"u": e.key().clone(), "pk": e.value().clone()}))
                .collect(),
        )
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
) {
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
    });
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

    // Track user status
    let status = d
        .get("status")
        .cloned()
        .unwrap_or(serde_json::json!({"text": "Online", "emoji": "🟢"}));
    state.user_statuses.insert(username.clone(), status);
    if let Some(pk) = d.get("pk").and_then(|v| v.as_str()) {
        if !pk.is_empty() {
            state.user_pubkeys.insert(username.clone(), pk.to_string());
        }
    }

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

    // Spawn task to forward general channel messages to output channel
    spawn_broadcast_forwarder(gen_rx, out_tx.clone());

    // Spawn task to send queued messages to WebSocket
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() {
                break;
            }
        }
    });

    let mut voice_room: Option<String> = None;

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

        // Validate message size
        if raw.len() > MAX_BYTES {
            continue;
        }

        // Parse message
        let d: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let t = d["t"].as_str().unwrap_or("");

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
                let rx = chan.tx.subscribe();
                spawn_broadcast_forwarder(rx, out_tx.clone());
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
                let vtx = state.voice_tx(&room);
                let vrx = vtx.subscribe();
                spawn_broadcast_forwarder(vrx, out_tx.clone());
                voice_room = Some(room.clone());
                let _ = state
                    .chan(&room)
                    .tx
                    .send(sys(&format!("🎙 {} joined voice #{}", username, room)));
            }
            "vleave" => {
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
                if let Some(status_val) = d.get("status") {
                    state
                        .user_statuses
                        .insert(username.clone(), status_val.clone());
                    let status_update = serde_json::json!({
                        "t": "status_update",
                        "user": username,
                        "status": status_val
                    })
                    .to_string();
                    for chan_entry in state.channels.iter() {
                        let _ = chan_entry.tx.send(status_update.clone());
                    }
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
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    broadcast_system_msg(&state, &format!("✖ {} left", username)).await;
    log(&state, &format!("- {}", username));
}

/// Main entry point
#[tokio::main]
async fn main() {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);

    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("Failed to bind {}: {}", addr, e);
            return;
        }
    };

    let state = State::new(args.log);

    println!("📡 Chatify running on ws://{}", addr);
    println!("🔒 Encryption: None (testing) | 🛡️  IP Privacy: On");
    println!("⏹️  Press Ctrl+C to stop\n");

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let s = state.clone();
                tokio::spawn(handle(stream, addr, s));
            }
            Err(_) => continue,
        }
    }
}
