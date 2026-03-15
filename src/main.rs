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

// ── cli ───────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "clicord-server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0")] host: String,
    #[arg(long, default_value_t = 8765)]    port: u16,
    #[arg(long)] log: bool,
}

// ── constants ─────────────────────────────────────────────────────────────────

const HISTORY_CAP: usize = 50;
const MAX_BYTES:   usize = 16_000;

// ── channel ───────────────────────────────────────────────────────────────────

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

// ── state ─────────────────────────────────────────────────────────────────────

struct State {
    pw_hash:    String,
    users:      DashMap<String, String>,                      // name → pubkey
    channels:   DashMap<String, Channel>,
    voice:      DashMap<String, broadcast::Sender<String>>,   // room → audio tx
    user_statuses: DashMap<String, Value>,                    // user → status
    file_transfers: DashMap<String, Value>,                   // file_id → metadata
    log_enabled: bool,
}

impl State {
    fn new(pw_hash: String, log_enabled: bool) -> Arc<Self> {
        let s = Arc::new(Self {
            pw_hash,
            users:    DashMap::new(),
            channels: DashMap::new(),
            voice:    DashMap::new(),
            user_statuses: DashMap::new(),
            file_transfers: DashMap::new(),
            log_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }
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

// ── helpers ───────────────────────────────────────────────────────────────────

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

// ── connection handler ────────────────────────────────────────────────────────

async fn handle(stream: TcpStream, _addr: SocketAddr, state: Arc<State>) {
    let ws = match accept_async(stream).await { Ok(w) => w, Err(_) => return };
    let (mut sink, mut stream) = ws.split();

    // auth
    let raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        _ => return,
    };
    let d: Value = match serde_json::from_str(&raw) { Ok(v) => v, Err(_) => return };

    if d["t"].as_str() != Some("auth") || d["pw"].as_str() != Some(state.pw_hash.as_str()) {
        let _ = sink.send(Message::Text(r#"{"t":"err","m":"bad auth"}"#.into())).await;
        return;
    }

    let base = d["u"].as_str().unwrap_or("anon").chars().take(24).collect::<String>();
    let base = base.trim();
    let base = if base.is_empty() { "anon" } else { base };
    let username = state.unique_name(base);
    let pubkey = d["pk"].as_str().unwrap_or("").to_string();
    let status = d.get("status").cloned().unwrap_or(serde_json::json!({"text": "Online", "emoji": "🟢"}));

    state.users.insert(username.clone(), pubkey.clone());
    state.user_statuses.insert(username.clone(), status);

    // subscribe to #general
    let general = state.chan("general");
    let mut gen_rx = general.tx.subscribe();
    let hist = general.hist().await;

    let ok = serde_json::json!({
        "t":"ok","u":username,"users":state.users_json(),"hist":hist
    }).to_string();

    if sink.send(Message::Text(ok)).await.is_err() {
        state.users.remove(&username);
        state.user_statuses.remove(&username);
        return;
    }

    // announce join
    for e in state.channels.iter() { 
        let _ = e.tx.send(sys(&format!("→ {} joined", username))); 
    }

     log(&state, &format!("+ {}", username));

    // mpsc queue → sink (all subscribed channels funnel here)
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // forward #general broadcasts to sink
    {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match gen_rx.recv().await {
                    Ok(m) => { if tx.send(m).is_err() { break; } }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(_) => {} // lagged
                }
            }
        });
    }

    // writer task
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() { break; }
        }
    });

    // subscribe to own DM channel
    {
        let dm_ch = format!("__dm__{}", username);
        let chan = state.chan(&dm_ch);
        let mut rx = chan.tx.subscribe();
        let tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(m) => { if tx.send(m).is_err() { break; } }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(_) => {}
                }
            }
        });
    }

    let mut voice_room: Option<String> = None;

    // main recv loop
    while let Some(Ok(msg)) = stream.next().await {
        let raw = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        if raw.len() > MAX_BYTES { continue; }
        let d: Value = match serde_json::from_str(&raw) { Ok(v) => v, Err(_) => continue };
        let t = d["t"].as_str().unwrap_or("");

        match t {
            "msg" => {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let c = d["c"].as_str().unwrap_or("").to_string();
                if c.is_empty() { continue; }
                let entry = serde_json::json!({"t":"msg","ch":ch,"u":username,"c":c,"ts":now()});
                let s = entry.to_string();
                let chan = state.chan(&ch);
                chan.push(entry).await;
                let _ = chan.tx.send(s);
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
                            Err(broadcast::error::RecvError::Closed) => break,
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
                            Err(broadcast::error::RecvError::Closed) => break,
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
                 // Find the most recent message matching old_text from this user (simplistic)
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
                
                // Forward chunk to channel
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
                    
                    // Broadcast status update
                    let status_update = serde_json::json!({
                        "t": "status_update",
                        "user": username,
                        "status": status_val
                    }).to_string();
                    
                    // Send to all channels
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

    // cleanup
    state.users.remove(&username);
    state.user_statuses.remove(&username);
    let leave = sys(&format!("✖ {} left", username));
    for e in state.channels.iter() { let _ = e.tx.send(leave.clone()); }
     log(&state, &format!("- {}", username));
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let pw = read_password("server password: ");
    if pw.is_empty() { eprintln!("password can't be empty"); std::process::exit(1); }

     let state = State::new(hash_pw(&pw), args.log);
    let addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&addr).await.expect("bind failed");

    println!("clicord running on ws://{addr}");
    println!("encryption: chacha20-poly1305 + x25519 | ip privacy: on");
    println!("enhanced features: file transfer, themes, reactions, status");
    println!("ctrl+c to stop\n");

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

// silent password input (no echo)
fn read_password(prompt: &str) -> String {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();

    #[cfg(unix)]
    unsafe {
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&std::io::stdin());
        let mut t: Termios = std::mem::zeroed();
        tcgetattr(fd, &mut t);
        let orig_lflag = t.c_lflag;
        t.c_lflag &= !ECHO;
        tcsetattr(fd, 0, &t);
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        t.c_lflag = orig_lflag; tcsetattr(fd, 0, &t);
        println!();
        return line.trim().to_string();
    }
    #[cfg(not(unix))]
    {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        line.trim().to_string()
    }
}

#[cfg(unix)]
#[repr(C)]
struct Termios { c_iflag: u32, c_oflag: u32, c_cflag: u32, c_lflag: u32, c_line: u8, c_cc: [u8;32], c_ispeed: u32, c_ospeed: u32 }
#[cfg(unix)] const ECHO: u32 = 0o10;
#[cfg(unix)] extern "C" { fn tcgetattr(fd: i32, t: *mut Termios) -> i32; fn tcsetattr(fd: i32, a: i32, t: *const Termios) -> i32; }
