use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream>;
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[derive(Parser)]
#[command(name = "clicord-server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0")] host: String,
    #[arg(long, default_value_t = 8765)]    port: u16,
    #[arg(long)] log: bool,
}

const HISTORY_CAP: usize = 50;
const MAX_BYTES:   usize = 16_000;

#[derive(Clone)]
struct Channel {
    history: Arc<RwLock<VecDeque<Value>>>,
    tx: broadcast::Sender<String>,
}

impl Channel {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))), tx }
    }
    async fn push(&self, entry: Value) {
        let mut h = self.history.write().await;
        if h.len() >= HISTORY_CAP { h.pop_front(); }
        h.push_back(entry);
    }
    async fn hist(&self) -> Vec<Value> {
        self.history.read().await.iter().cloned().collect()
    }
}

struct State {
    channels:   DashMap<String, Channel>,
    voice:      DashMap<String, broadcast::Sender<String>>,
    user_statuses: DashMap<String, Value>,
    file_transfers: DashMap<String, Value>,
    log_enabled: bool,
}

impl State {
    fn new(log_enabled: bool) -> Arc<Self> {
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice:    DashMap::new(),
            user_statuses: DashMap::new(),
            file_transfers: DashMap::new(),
            log_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    fn chan(&self, name: &str) -> Channel {
        self.channels.entry(name.into()).or_insert_with(Channel::new).clone()
    }

    fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.voice.entry(room.into()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        }).clone()
    }

    fn users_json(&self) -> Value {
        Value::Array(
            self.users.iter()
                .map(|e| {
                    let mut user_obj = serde_json::json!({"u": e.key(), "pk": e.value()});
                    if let Some(status) = self.user_statuses.get(e.key()) {
                        user_obj["status"] = status.clone();
                    }
                    user_obj
                })
                .collect()
        )
    }

    fn unique_name(&self, base: &str) -> String {
        let mut name = base.to_string();
        let mut n = 1usize;
        while self.users.contains_key(&name) { name = format!("{}_{}", base, n); n += 1; }
        name
    }
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

fn hash_pw(pw: &str) -> String {
    hex::encode(Sha256::digest(pw.as_bytes()))
}

fn safe_ch(raw: &str) -> String {
    let s: String = raw.to_lowercase().trim_start_matches('#')
        .chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').take(32).collect();
    if s.is_empty() { "general".into() } else { s }
}

fn sys(text: &str) -> String {
    serde_json::json!({"t":"sys","m":text,"ts":now()}).to_string()
}

fn hms() -> String {
    let s = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{:02}:{:02}:{:02}", (s%86400)/3600, (s%3600)/60, s%60)
}

fn log(state: &State, msg: &str) {
    if state.log_enabled {
        println!("[{}] {}", hms(), msg);
    }
}

async fn handle(stream: TcpStream, _addr: SocketAddr, state: Arc<State>) {
    let ws = match accept_async(stream).await { Ok(w) => w, Err(_) => return };
    let (mut sink, mut stream) = ws.split();

    let raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        _ => return,
    };
    let d: Value = match serde_json::from_str(&raw) { Ok(v) => v, Err(_) => return };

    // We don't do authentication for now, just use the username from the message
    let username = d.get("u").and_then(|v| v.as_str()).unwrap_or("anon").to_string();
    let status = d.get("status").cloned().unwrap_or(serde_json::json!({"text": "Online", "emoji": "🟢"}));

    // We don't store users for now, just use the username
    // state.users.insert(username.clone(), pubkey.clone());
    // state.user_statuses.insert(username.clone(), status);

    let general = state.chan("general");
    let mut gen_rx = general.tx.subscribe();
    let hist = general.hist().await;

    let ok = serde_json::json!({
        "t":"ok","u":username,"users":state.users_json(),"hist":hist
    }).to_string();

    if sink.send(Message::Text(ok)).await.is_err() {
        return;
    }

    for e in state.channels.iter() {
        let _ = e.tx.send(sys(&format!("→ {} joined", username)));
    }

    log(&state, &format!("+ {}", username));

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let tx = out_tx.clone();
    tokio::spawn(async move {
        loop {
            match gen_rx.recv().await {
                Ok(m) => { if tx.send(m).is_err() { break; } }
                Err(broadcast::error::RecvError::Closed) => break;
                Err(_) => {}
            }
        }
    });

    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() { break; }
        }
    });

    let mut voice_room: Option<String> = None;

    while let Some(Ok(msg)) = stream.next().await {
        let raw = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        if raw.len() > MAX_BYTES { continue; }
        let d: Value = match serde_json::from_str(&raw) { Ok(v) => v, Err(_) => continue };
        let t = d["t"].as_str().unwrap_or("");
        let ts = d["ts"].as_u64().unwrap_or(0);

        match t {
            "msg" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let c = d["c"].as_str().unwrap_or("").to_string();
                if c.is_empty() { continue; }
                let entry = serde_json::json!({"t":"msg","ch":ch,"u":username,"c":c,"ts":now()});
                let chan = state.chan(&ch);
                chan.push(entry).await;
                let _ = chan.tx.send(entry.to_string());
            }
            "img" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let a = d["a"].as_str().unwrap_or("").to_string();
                if a.is_empty() { continue; }
                let _ = state.chan(&ch).tx.send(
                    serde_json::json!({"t":"img","ch":ch,"u":username,"a":a,"ts":now()}).to_string()
                );
            }
            "dm" => {
                let target = d["to"].as_str().unwrap_or("").to_string();
                let c = d["c"].as_str().unwrap_or("").to_string();
                if c.is_empty() || target.is_empty() { continue; }
                let p = serde_json::json!({"t":"dm","from":username,"to":target,"c":c,"ts":now()}).to_string();
                let _ = state.chan(&format!("__dm__{}", target)).tx.send(p.clone());
                let _ = state.chan(&format!("__dm__{}", username)).tx.send(p);
            }
            "join" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let chan = state.chan(&ch);
                let hist = chan.hist().await;
                let mut rx = chan.tx.subscribe();
                let tx2 = out_tx.clone();
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(m) => { if tx2.send(m).is_err() { break; } }
                            Err(broadcast::error::RecvError::Closed) => break;
                            Err(_) => {}
                        }
                    }
                });
                let _ = out_tx.send(serde_json::json!({"t":"joined","ch":ch,"hist":hist}).to_string());
                let _ = chan.tx.send(sys(&format!("→ {} joined #{}", username, ch)));
            }
            "users" => {
                let _ = out_tx.send(serde_json::json!({"t":"users","users":state.users_json()}).to_string());
            }
            "info" => {
                let chs: Vec<String> = state.channels.iter()
                    .filter(|e| !e.key().starts_with("__dm__"))
                    .map(|e| e.key().clone()).collect();
                let _ = out_tx.send(serde_json::json!({"t":"info","chs":chs,"online":state.users.len()}).to_string());
            }
            "vjoin" => {
                let room = safe_ch(d["r"].as_str().unwrap_or("general"));
                let vtx = state.voice_tx(&room);
                let mut vrx = vtx.subscribe();
                let tx2 = out_tx.clone();
                tokio::spawn(async move {
                    loop {
                        match vrx.recv().await {
                            Ok(m) => { if tx2.send(m).is_err() { break; } }
                            Err(broadcast::error::RecvError::Closed) => break;
                            Err(_) => {}
                        }
                    }
                });
                voice_room = Some(room.clone());
                let _ = state.chan(&room).tx.send(sys(&format!("🎙 {} joined voice #{}", username, room)));
            }
            "vleave" => {
                if let Some(ref room) = voice_room.take() {
                    let _ = state.chan(room).tx.send(sys(&format!("🎙 {} left voice #{}", username, room)));
                }
            }
            "vdata" => {
                let a = d["a"].as_str().unwrap_or("").to_string();
                if a.is_empty() { continue; }
                if let Some(ref room) = voice_room {
                    if let Some(vtx) = state.voice.get(room) {
                        let _ = vtx.send(serde_json::json!({"t":"vdata","from":username,"a":a}).to_string());
                    }
                }
            }
            "ping" => { let _ = out_tx.send(r#"{"t":"pong"}"#.into()); }
            "edit" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let old_text = d["old_text"].as_str().unwrap_or("").to_string();
                let new_text = d["new_text"].as_str().unwrap_or("").to_string();
                if old_text.is_empty() || new_text.is_empty() { continue; }
                let chan = state.chan(&ch);
                let mut h = chan.history.write().await;
                if let Some(pos) = h.iter().rposition(|m| {
                    m.get("t") == Some(&Value::from("msg"))
                        && m.get("u") == Some(&Value::from(username))
                        && m.get("c") == Some(&Value::from(old_text))
                }) {
                    h[pos]["c"] = Value::from(new_text);
                    h[pos]["ts"] = Value::from(now());
                }
                let edit_msg = serde_json::json!({
                    "t": "edit",
                    "ch": ch,
                    "u": username,
                    "old_text": old_text,
                    "new_text": new_text,
                    "ts": now()
                }).to_string();
                let _ = chan.tx.send(edit_msg);
            }
            "file_meta" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let filename = d["filename"].as_str().unwrap_or("unknown").to_string();
                let size = d["size"].as_u64().unwrap_or(0);
                let file_id = d["file_id"].as_str().unwrap_or(&format!("{}_{}", username, now())).to_string();
                state.file_transfers.insert(file_id.clone(), d.clone());
                let file_announce = serde_json::json!({
                    "t": "file_meta",
                    "from": username,
                    "filename": filename,
                    "size": size,
                    "file_id": file_id,
                    "ch": ch,
                    "ts": now()
                }).to_string();
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
                }).to_string();
                let _ = state.chan(&ch).tx.send(chunk_msg);
            }
            "status" => {
                if let Some(status_val) = d.get("status") {
                    state.user_statuses.insert(username.clone(), status_val.clone());
                    let status_update = serde_json::json!({
                        "t": "status_update",
                        "user": username,
                        "status": status_val
                    }).to_string();
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
                }).to_string();
                let _ = state.chan(&ch).tx.send(reaction_msg);
            }
            _ => {}
        }
    }

    // We don't remove the user because we are not storing them
    // state.users.remove(&username);
    // state.user_statuses.remove(&username);
    let leave = sys(&format!("✖ {} left", username));
    for e in state.channels.iter() { let _ = e.tx.send(leave.clone()); }
    log(&state, &format!("- {}", username));
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    println!("clicord running on ws://{addr}");
    println!("encryption: none (testing) | ip privacy: on");
    println!("ctrl+c to stop\n");
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let s = State::new(args.log);
                tokio::spawn(handle(stream, addr, s));
            }
            Err(_) => continue,
        }
    }
}