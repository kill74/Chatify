use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection, Error as SqlError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, RwLock};
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
    #[arg(long, default_value = "chatify.db")]
    db: String,
}

const HISTORY_CAP: usize = 50;
const MAX_BYTES: usize = 16_000;
const MAX_AUTH_BYTES: usize = 4_096;
const CURRENT_SCHEMA_VERSION: i64 = 1;
const DEFAULT_HISTORY_LIMIT: usize = 50;
const DEFAULT_SEARCH_LIMIT: usize = 30;
const DEFAULT_REWIND_SECONDS: u64 = 3600;
const DEFAULT_REWIND_LIMIT: usize = 100;
const PROTOCOL_VERSION: u64 = 1;
const MAX_USERNAME_LEN: usize = 32;
const MAX_PASSWORD_FIELD_LEN: usize = 256;
const MAX_PUBLIC_KEY_FIELD_LEN: usize = 256;
const MAX_CLOCK_SKEW_SECS: f64 = 300.0;
const MAX_NONCE_LEN: usize = 64;
const NONCE_CACHE_CAP: usize = 256;

struct AuthInfo {
    username: String,
    status: Value,
    pubkey: String,
}

#[derive(Clone)]
struct EventStore {
    path: String,
}

impl EventStore {
    fn new(path: String) -> Self {
        let store = Self { path };
        if let Err(e) = store.init() {
            eprintln!("Failed to initialize event store: {}", e);
        }
        store
    }

    fn init(&self) -> rusqlite::Result<()> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;

        let version = Self::schema_version(&conn)?;
        self.migrate(&conn, version)?;
        Ok(())
    }

    fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
        let value: rusqlite::Result<String> = conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        );
        match value {
            Ok(v) => Ok(v.parse::<i64>().unwrap_or(0)),
            Err(SqlError::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn set_schema_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO schema_meta(key, value)
             VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![version.to_string()],
        )?;
        Ok(())
    }

    fn migrate(&self, conn: &Connection, from_version: i64) -> rusqlite::Result<()> {
        let mut version = from_version;

        if version < 1 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts REAL NOT NULL,
                    event_type TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    sender TEXT,
                    target TEXT,
                    payload TEXT NOT NULL,
                    search_text TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_channel_ts ON events(channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_search ON events(search_text);
                ",
            )?;
            version = 1;
            Self::set_schema_version(conn, version)?;
        }

        if version > CURRENT_SCHEMA_VERSION {
            eprintln!(
                "Database schema version {} is newer than supported version {}",
                version, CURRENT_SCHEMA_VERSION
            );
        }

        Ok(())
    }

    fn open_conn(&self) -> Option<Connection> {
        match Connection::open(&self.path) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("Event store open failed: {}", e);
                None
            }
        }
    }

    fn decode_rows(rows: Vec<String>) -> Vec<Value> {
        rows.into_iter()
            .filter_map(|payload| serde_json::from_str::<Value>(&payload).ok())
            .collect()
    }

    fn query_events<P>(&self, sql: &str, params: P) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let Some(conn) = self.open_conn() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Event query prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = match stmt.query_map(params, |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Event query execute failed: {}", e);
                return Vec::new();
            }
        };

        let mut out = Self::decode_rows(rows.filter_map(|r| r.ok()).collect());
        out.reverse();
        out
    }

    fn persist(
        &self,
        event_type: &str,
        channel: &str,
        sender: &str,
        target: Option<&str>,
        payload: &Value,
        search_text: &str,
    ) {
        let Some(conn) = self.open_conn() else {
            return;
        };
        let payload_json = payload.to_string();
        if let Err(e) = conn.execute(
            "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                now(),
                event_type,
                channel,
                sender,
                target,
                payload_json,
                search_text.to_lowercase()
            ],
        ) {
            eprintln!("Event persist failed: {}", e);
        }
    }

    fn history(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    fn search(&self, channel: &str, query: &str, limit: usize) -> Vec<Value> {
        let like = format!("%{}%", query.to_lowercase());
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1 AND search_text LIKE ?2
             ORDER BY ts DESC
             LIMIT ?3",
            params![channel, like, limit as i64],
        )
    }

    fn rewind(&self, channel: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1 AND ts >= ?2
             ORDER BY ts DESC
             LIMIT ?3",
            params![channel, cutoff, limit as i64],
        )
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

struct State {
    channels: DashMap<String, Channel>,
    voice: DashMap<String, broadcast::Sender<String>>,
    user_statuses: DashMap<String, Value>,
    user_pubkeys: DashMap<String, String>,
    recent_nonces: DashMap<String, VecDeque<String>>,
    file_transfers: DashMap<String, Value>,
    store: EventStore,
    log_enabled: bool,
}

impl State {
    fn new(log_enabled: bool, db_path: String) -> Arc<Self> {
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            recent_nonces: DashMap::new(),
            file_transfers: DashMap::new(),
            store: EventStore::new(db_path),
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

fn clamp_limit(raw: Option<u64>, default: usize, max: usize) -> usize {
    raw.map(|v| v as usize).unwrap_or(default).clamp(1, max)
}

fn send_out_json(out_tx: &mpsc::UnboundedSender<String>, payload: Value) {
    let _ = out_tx.send(payload.to_string());
}

fn spawn_broadcast_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: mpsc::UnboundedSender<String>,
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

async fn handle_event(
    d: &Value,
    state: &Arc<State>,
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    voice_room: &mut Option<String>,
) {
    let t = d["t"].as_str().unwrap_or("");
    if requires_fresh_protection(t) {
        if let Err(e) = validate_timestamp_skew(d) {
            send_out_json(
                out_tx,
                serde_json::json!({"t":"err","m":format!("protocol validation failed: {}", e)}),
            );
            return;
        }
        if let Err(e) = validate_and_register_nonce(state, username, d) {
            send_out_json(
                out_tx,
                serde_json::json!({"t":"err","m":format!("protocol validation failed: {}", e)}),
            );
            return;
        }
    }
    match t {
        "msg" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let c = d["c"].as_str().unwrap_or("").to_string();
            let p = d["p"].as_str().unwrap_or("").to_string();
            if c.is_empty() {
                return;
            }
            let entry = serde_json::json!({"t":"msg","ch":ch,"u":username,"c":c,"ts":now()});
            let chan = state.chan(&ch);
            chan.push(entry.clone()).await;
            let searchable = if p.is_empty() { c.clone() } else { p };
            state
                .store
                .persist("msg", &ch, username, None, &entry, &searchable);
            let _ = chan.tx.send(entry.to_string());
        }
        "img" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let a = d["a"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                return;
            }
            let _ = state.chan(&ch).tx.send(
                serde_json::json!({"t":"img","ch":ch,"u":username,"a":a,"ts":now()}).to_string(),
            );
        }
        "dm" => {
            let target = d["to"].as_str().unwrap_or("").to_string();
            let c = d["c"].as_str().unwrap_or("").to_string();
            let ptxt = d["p"].as_str().unwrap_or("").to_string();
            if c.is_empty() || target.is_empty() {
                return;
            }
            let sender_pk = state
                .user_pubkeys
                .get(username)
                .map(|v| v.value().clone())
                .unwrap_or_default();
            let event = serde_json::json!({"t":"dm","from":username,"to":target,"c":c,"pk":sender_pk,"ts":now()});
            let p = event.to_string();
            state.store.persist(
                "dm",
                &format!("__dm__{}", target),
                username,
                Some(&target),
                &event,
                &ptxt,
            );
            let _ = state.chan(&format!("__dm__{}", target)).tx.send(p.clone());
            let _ = state.chan(&format!("__dm__{}", username)).tx.send(p);
        }
        "join" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let chan = state.chan(&ch);
            let mut hist = state.store.history(&ch, HISTORY_CAP);
            if hist.is_empty() {
                hist = chan.hist().await;
            }
            spawn_broadcast_forwarder(chan.tx.subscribe(), out_tx.clone());
            send_out_json(
                out_tx,
                serde_json::json!({"t":"joined","ch":ch,"hist":hist}),
            );
            let join_msg = serde_json::json!({"t":"sys","m":format!("→ {} joined #{}", username, ch),"ts":now()});
            state.store.persist(
                "sys",
                &ch,
                username,
                None,
                &join_msg,
                &format!("{} joined", username),
            );
            let _ = chan.tx.send(join_msg.to_string());
        }
        "history" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_HISTORY_LIMIT,
                200,
            );
            let events = state.store.history(&ch, limit);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":ch,"events":events,"ts":now()}),
            );
        }
        "search" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let q = d["q"].as_str().unwrap_or("").trim().to_string();
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_SEARCH_LIMIT,
                200,
            );
            let events = if q.is_empty() {
                Vec::new()
            } else {
                state.store.search(&ch, &q, limit)
            };
            send_out_json(
                out_tx,
                serde_json::json!({"t":"search","ch":ch,"q":q,"events":events,"ts":now()}),
            );
        }
        "rewind" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let seconds = d
                .get("seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_REWIND_SECONDS)
                .clamp(1, 31 * 24 * 3600);
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_REWIND_LIMIT,
                500,
            );
            let events = state.store.rewind(&ch, seconds, limit);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":ch,"events":events,"ts":now()}),
            );
        }
        "users" => {
            send_out_json(
                out_tx,
                serde_json::json!({"t":"users","users":state.users_with_keys_json()}),
            );
        }
        "info" => {
            let chs: Vec<String> = state
                .channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| e.key().clone())
                .collect();
            send_out_json(
                out_tx,
                serde_json::json!({"t":"info","chs":chs,"online":state.online_count()}),
            );
        }
        "vjoin" => {
            let room = safe_ch(d["r"].as_str().unwrap_or("general"));
            let vtx = state.voice_tx(&room);
            spawn_broadcast_forwarder(vtx.subscribe(), out_tx.clone());
            *voice_room = Some(room.clone());
            let join_voice = serde_json::json!({"t":"sys","m":format!("🎙 {} joined voice #{}", username, room),"ts":now()});
            state.store.persist(
                "sys",
                &room,
                username,
                None,
                &join_voice,
                &format!("{} voice joined", username),
            );
            let _ = state.chan(&room).tx.send(join_voice.to_string());
        }
        "vleave" => {
            if let Some(ref room) = voice_room.take() {
                let leave_voice = serde_json::json!({"t":"sys","m":format!("🎙 {} left voice #{}", username, room),"ts":now()});
                state.store.persist(
                    "sys",
                    room,
                    username,
                    None,
                    &leave_voice,
                    &format!("{} voice left", username),
                );
                let _ = state.chan(room).tx.send(leave_voice.to_string());
            }
        }
        "vdata" => {
            let a = d["a"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                return;
            }
            if let Some(ref room) = voice_room {
                if let Some(vtx) = state.voice.get(room) {
                    let _ = vtx
                        .send(serde_json::json!({"t":"vdata","from":username,"a":a}).to_string());
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
                return;
            }
            let chan = state.chan(&ch);
            let mut h = chan.history.write().await;
            if let Some(pos) = h.iter().rposition(|m| {
                m.get("t") == Some(&Value::from("msg"))
                    && m.get("u") == Some(&Value::from(username.to_string()))
                    && m.get("c") == Some(&Value::from(old_text.clone()))
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
            });
            state
                .store
                .persist("edit", &ch, username, None, &edit_msg, &new_text);
            let _ = chan.tx.send(edit_msg.to_string());
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
            });
            state.store.persist(
                "file_meta",
                &ch,
                username,
                None,
                &file_announce,
                file_announce
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            let _ = state.chan(&ch).tx.send(file_announce.to_string());
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
                    .insert(username.to_string(), status_val.clone());
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
            });
            state
                .store
                .persist("reaction", &ch, username, None, &reaction_msg, &emoji);
            let _ = state.chan(&ch).tx.send(reaction_msg.to_string());
        }
        _ => {}
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
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
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

/// Create JSON response for successful connection
fn create_ok_response(username: &str, state: &Arc<State>, hist: Vec<Value>) -> String {
    serde_json::json!({
        "t": "ok",
        "u": username,
        "users": state.users_with_keys_json(),
        "channels": state.channels_json(),
        "hist": hist,
        "proto": {
            "v": PROTOCOL_VERSION,
            "max_payload_bytes": MAX_BYTES
        }
    })
    .to_string()
}

fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_valid_pubkey_b64(pk: &str) -> bool {
    if pk.is_empty() || pk.len() > MAX_PUBLIC_KEY_FIELD_LEN {
        return false;
    }
    match general_purpose::STANDARD.decode(pk) {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

fn validate_auth_payload(d: &Value) -> Result<AuthInfo, String> {
    if !d.is_object() {
        return Err("invalid auth frame".to_string());
    }
    if d.get("t").and_then(|v| v.as_str()) != Some("auth") {
        return Err("first frame must be auth".to_string());
    }

    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing username".to_string())?
        .to_string();
    if !is_valid_username(&username) {
        return Err("invalid username".to_string());
    }

    let pw = d
        .get("pw")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing password hash".to_string())?;
    if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
        return Err("invalid password hash".to_string());
    }

    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing public key".to_string())?
        .to_string();
    if !is_valid_pubkey_b64(&pubkey) {
        return Err("invalid public key".to_string());
    }

    let status = d
        .get("status")
        .cloned()
        .unwrap_or(serde_json::json!({"text": "Online", "emoji": "🟢"}));

    Ok(AuthInfo {
        username,
        status,
        pubkey,
    })
}

fn requires_fresh_protection(event_type: &str) -> bool {
    matches!(
        event_type,
        "msg"
            | "img"
            | "dm"
            | "vdata"
            | "edit"
            | "file_meta"
            | "file_chunk"
            | "status"
            | "reaction"
    )
}

fn validate_timestamp_skew(d: &Value) -> Result<(), String> {
    let Some(ts) = d
        .get("ts")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|u| u as f64)))
    else {
        // Backward-compatible: accept events from clients that don't send ts.
        return Ok(());
    };

    if !ts.is_finite() || ts < 0.0 {
        return Err("invalid timestamp".to_string());
    }

    if (now() - ts).abs() > MAX_CLOCK_SKEW_SECS {
        return Err("timestamp outside allowed clock skew".to_string());
    }

    Ok(())
}

fn validate_and_register_nonce(state: &State, username: &str, d: &Value) -> Result<(), String> {
    let Some(nonce) = d.get("n").and_then(|v| v.as_str()) else {
        // Backward-compatible: accept events without nonce.
        return Ok(());
    };

    if nonce.is_empty() || nonce.len() > MAX_NONCE_LEN {
        return Err("invalid nonce".to_string());
    }
    if !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid nonce format".to_string());
    }

    let mut user_nonces = state.recent_nonces.entry(username.to_string()).or_default();

    if user_nonces.iter().any(|n| n == nonce) {
        return Err("replayed nonce".to_string());
    }

    user_nonces.push_back(nonce.to_string());
    if user_nonces.len() > NONCE_CACHE_CAP {
        let _ = user_nonces.pop_front();
    }

    Ok(())
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
        Some(Ok(_)) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"first frame must be text auth"}).to_string(),
                ))
                .await;
            return;
        }
        _ => return,
    };
    if raw.len() > MAX_AUTH_BYTES {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"auth frame too large"}).to_string(),
            ))
            .await;
        return;
    }
    let d: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"invalid auth JSON"}).to_string(),
                ))
                .await;
            return;
        }
    };

    let auth = match validate_auth_payload(&d) {
        Ok(a) => a,
        Err(msg) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":msg}).to_string(),
                ))
                .await;
            return;
        }
    };

    let username = auth.username;

    // Track user status
    state.user_statuses.insert(username.clone(), auth.status);
    state.user_pubkeys.insert(username.clone(), auth.pubkey);

    // Get general channel and send welcome response
    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let mut hist = state.store.history("general", HISTORY_CAP);
    if hist.is_empty() {
        hist = general.hist().await;
    }

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
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload exceeds max size"}),
            );
            continue;
        }

        // Parse message
        let d: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                send_out_json(
                    &out_tx,
                    serde_json::json!({"t":"err","m":"invalid JSON payload"}),
                );
                continue;
            }
        };
        if !d.is_object() {
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload must be a JSON object"}),
            );
            continue;
        }

        handle_event(&d, &state, &username, &out_tx, &mut voice_room).await;
    }

    // User disconnected - cleanup
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    state.recent_nonces.remove(&username);
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

    let state = State::new(args.log, args.db.clone());

    println!(" Chatify running on ws://{}", addr);
    println!(" Encryption: None (testing) |   IP Privacy: On");
    println!(" Event store: {}", args.db);
    println!("⏹  Press Ctrl+C to stop\n");

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
