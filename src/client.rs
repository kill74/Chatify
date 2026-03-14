use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{self, engine::general_purpose, Engine as _};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use chacha20poly1305::aead::{Aead, NewAead};
use dashmap::DashMap;
use hex::{self, FromHex};
use pbkdf2::pbkdf2;
use rand::rngs::OsRng;
use rand::RngCore;
use rodio::{self, OutputStream, OutputStreamHandle, Sink};
use sha2::Sha256;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::tungstenite::Error as WsError;
use x25519_dalek::{PublicKey, StaticSecret};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, DisableLineWrap, EnableLineWrap},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};

use chrono::{DateTime, Utc};

// Utility functions (ported from utils.py)
fn channel_key(password: &str, channel: &str) -> Vec<u8> {
    let mut key = [0u8; 32];
    pbkdf2::<Sha256>(password.as_bytes(), format!("chatify:{}", channel).as_bytes(), 120000, &mut key);
    key.to_vec()
}

fn dh_key(priv: &StaticSecret, pubkey_b64: &str) -> Vec<u8> {
    let pubkey_bytes = general_purpose::STANDARD.decode(pubkey_b64).expect("Invalid base64");
    let pubkey = PublicKey::from(pubkey_bytes.as_slice());
    let secret = priv.diffie_hellman(&pubkey);
    secret.as_bytes().to_vec()
}

fn enc_bytes(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let nonce = chaCha20_nonce();
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("Key length must be 32 bytes");
    let ciphertext = cipher.encrypt(nonce.as_ref(), plaintext).expect("Encryption failure");
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    combined
}

fn dec_bytes(key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    if ciphertext.len() < 12 {
        panic!("Ciphertext too short");
    }
    let (nonce, ciphertext) = ciphertext.split_at(12);
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("Key length must be 32 bytes");
    let plaintext = cipher.decrypt(nonce, ciphertext).expect("Decryption failure");
    plaintext.to_vec()
}

fn chaCha20_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn pw_hash(password: &str) -> String {
    let mut hash = [0u8; 32];
    pbkdf2::<Sha256>(password.as_bytes(), b"chatify", 120000, &mut hash);
    hex::encode(hash)
}

fn new_keypair() -> StaticSecret {
    StaticSecret::new(OsRng)
}

fn pub_b64(priv: &StaticSecret) -> String {
    let pubkey = PublicKey::from(priv);
    general_purpose::STANDARD.encode(pubkey.as_bytes())
}

// Message types
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
struct IncomingMessage {
    t: MessageType,
    data: HashMap<String, String>,
    ts: u64,
}

struct ClientState {
    ws_tx: mpsc::UnboundedSender<Message>,
    me: String,
    pw: String, // Note: we store the raw password for key derivation (in memory only)
    ch: String,
    chs: HashMap<String, bool>,
    users: HashMap<String, String>, // name -> pubkey_b64
    chan_keys: HashMap<String, Vec<u8>>,
    dm_keys: HashMap<String, Vec<u8>>,
    priv_key: StaticSecret,
    running: bool,
    voice_active: bool,
    theme: String, // Simple theme name for now
    file_transfers: HashMap<String, FileTransfer>,
    message_history: Vec<DisplayedMessage>,
    status: Status,
    reactions: HashMap<String, HashMap<String, u32>>,
    log_enabled: bool,
}

#[derive(Debug, Clone)]
struct DisplayedMessage {
    time: String,
    text: String,
    msg_type: MessageType,
    user: Option<String>,
    channel: Option<String>,
}

#[derive(Debug, Clone)]
struct FileTransfer {
    filename: String,
    size: u64,
    chunks: Vec<String>,
    received: u64,
}

#[derive(Debug, Clone)]
struct Status {
    text: String,
    emoji: char,
}

impl ClientState {
    fn new(ws_tx: mpsc::UnboundedSender<Message>, password: String, log_enabled: bool) -> Self {
        let priv_key = StaticSecret::new(OsRng);
        let mut chs = HashMap::new();
        chs.insert("general".to_string(), true);
        Self {
            ws_tx,
            me: String::new(),
            pw: password,
            ch: "general".to_string(),
            chs,
            users: HashMap::new(),
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key,
            running: true,
            voice_active: false,
            theme: "default".to_string(),
            file_transfers: HashMap::new(),
            message_history: Vec::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: '🟢',
            },
            reactions: HashMap::new(),
            log_enabled,
        }
    }

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

    fn dmkey(&mut self, name: &str) -> Vec<u8> {
        if !self.dm_keys.contains_key(name) {
            let pk = self.users.get(name).expect("User not found");
            let pk_bytes = Vec::from_hex(pk).expect("Invalid pubkey");
            let key = dh_key(&self.priv_key, &pk_bytes);
            self.dm_keys.insert(name.to_string(), key.clone());
            key
        } else {
            self.dm_keys[name].clone()
        }
    }
}

// Helper to format timestamp
fn format_time(ts: u64) -> String {
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or_else(|| Utc::now());
    datetime.format("%H:%M").to_string()
}

// Simple image to ASCII conversion (placeholder)
fn img_to_ascii(_: &[u8], _: u16) -> String {
    "[Image sending not yet implemented in Rust client]".to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let matches = clap::Command::new("clicord-client")
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
                .help("Use TLS"),
        )
        .arg(
            clap::Arg::new("log")
                .long("log")
                .action(clap::ArgAction::SetTrue)
                .help("Enable logging"),
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
        input.trim().to_string()
    };

    let password = rpassword::prompt_password("password: ").unwrap();

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
        "pk": pub_b64(&new_keypair()),
        "status": {"text": "Online", "emoji": "🟢"}
    });
    ws_tx.send(Message::Text(auth_msg.to_string())).await?;

    // Wait for auth response
    if let Some(Ok(Message::Text(resp))) = ws_rx.next().await {
        let resp_val: serde_json::Value = serde_json::from_str(&resp)?;
        if resp_val["t"] == "err" {
            eprintln!("Authentication failed: {}", resp_val["m"]);
            return Ok(());
        }
        // Update state with server response
        let me = resp_val["u"].as_str().unwrap_or(&username).to_string();
        let users: Vec<serde_json::Value> = serde_json::from_value(resp_val["users"].clone())?;
        let mut user_map = HashMap::new();
        for u in users {
            if let Some(name) = u["u"].as_str() {
                if let Some(pk) = u["pk"].as_str() {
                    user_map.insert(name.to_string(), pk.to_string());
                }
            }
        }
        // We'll store the state in an Arc for sharing between tasks
        let state = Arc::new(tokio::sync::Mutex::new(ClientState {
            ws_tx: mpsc::UnboundedSender::clone(&ws_tx), // Clone the sender
            me: me.clone(),
            pw: password,
            ch: "general".to_string(),
            chs: HashMap::from([("general".to_string(), true)]),
            users: user_map,
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: new_keypair(),
            running: true,
            voice_active: false,
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
            while let Some(Ok(msg)) = ws_rx.next().await {
                match msg {
                    Message::Text(text) => {
                        if let Ok(data) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) {
                            // Process message and update state
                            let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
                            let ts = data.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
                            match t {
                                "msg" => {
                                    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                                    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
                                    let encrypted = match general_purpose::STANDARD.decode(c) {
                                        Ok(bytes) => bytes,
                                        Err(_) => continue,
                                    };
                                    if let Ok(content) = dec_bytes(&state_clone.lock().await.ckey(ch), &encrypted) {
                                        let mut state = state_clone.lock().await;
                                        state.message_history.push(DisplayedMessage {
                                            time: format_time(ts),
                                            text: String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string()),
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
                                    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                                    let a = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
                                    let encrypted = match general_purpose::STANDARD.decode(a) {
                                        Ok(bytes) => bytes,
                                        Err(_) => continue,
                                    };
                                    if let Ok(ascii_art) = img_to_ascii(&encrypted, 70) {
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
                                    if let Ok(content) = dec_bytes(&state_clone.lock().await.dmkey(if frm == state_clone.lock().await.me { &to } else { &frm }), &encrypted) {
                                        let mut state = state_clone.lock().await;
                                        let arrow = if frm == state_clone.lock().await.me { "→" } else { "←" };
                                        state.message_history.push(DisplayedMessage {
                                            time: format_time(ts),
                                            text: format!("{} {} {}", frm, arrow, content),
                                            msg_type: MessageType::Dm,
                                            user: Some(frm.to_string()),
                                            channel: None,
                                        });
                                        if state.message_history.len() > 100 {
                                            state.message_history.remove(0);
                                        }
                                    }
                                }
                                "users" => {
                                    let mut state = state_clone.lock().await;
                                    let names: Vec<String> = serde_json::from_value(data.get("users").unwrap_or(&serde_json::json!([])).clone())?;
                                    state.message_history.push(DisplayedMessage {
                                        time: format_time(ts),
                                        text: format!("Online users: {}", names.join(", ")),
                                        msg_type: MessageType::Sys,
                                        user: None,
                                        channel: None,
                                    });
                                    if state.message_history.len() > 100 {
                                        state.message_history.remove(0);
                                    }
                                }
                                "joined" => {
                                    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                    let mut state = state_clone.lock().await;
                                    state.ch = ch.clone();
                                    state.chs.insert(ch.clone(), true);
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
                                    let _ = state.ws_tx.send(Message::Text(
                                        serde_json::json!({
                                            "t": "join",
                                            "ch": ch,
                                            "ts": SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs()
                                        })
                                        .to_string(),
                                    ));
                                }
                                "info" => {
                                    let mut state = state_clone.lock().await;
                                    let chs: Vec<String> = serde_json::from_value(data.get("chs").unwrap_or(&serde_json::json!([])).clone())?;
                                    let online = data.get("online").as_u64().unwrap_or(0);
                                    state.message_history.push(DisplayedMessage {
                                        time: format_time(ts),
                                        text: format!("Channels: {} | Online: {}", chs.join(", "), online),
                                        msg_type: MessageType::Sys,
                                        user: None,
                                        channel: None,
                                    });
                                    if state.message_history.len() > 100 {
                                        state.message_history.remove(0);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        let state_clone2 = state.clone();
        let stdin_task = tokio::spawn(async move {
            let stdin = tokio_io::BufReader::new(tokio_io::stdin());
            let mut lines = stdin.lines();
            while let Ok(Some(line)) = lines.next_line().await {
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
                            let ch = args.trim().trim_start_matches('#');
                            if !ch.is_empty() {
                                state.ch = ch.to_string();
                                state.chs.insert(ch.to_string(), true);
                                // Request history for the channel
                                let _ = state.ws_tx.send(Message::Text(
                                    serde_json::json!({
                                        "t": "join",
                                        "ch": ch,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                ));
                            }
                        }
                        "/dm" => {
                            let mut parts = args.splitn(2, ' ');
                            let target = parts.next().unwrap_or("");
                            let msg = parts.next().unwrap_or("");
                            if !target.is_empty() && !msg.is_empty() {
                                let encrypted = enc_bytes(&state.dmkey(target), msg.as_bytes());
                                let encoded = general_purpose::STANDARD.encode(&encrypted);
                                let _ = state.ws_tx.send(Message::Text(
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
                                ));
                            }
                        }
                        "/me" => {
                            let action = args;
                            if !action.is_empty() {
                                let msg = format!("* {} {}", state.me, action);
                                let encrypted = enc_bytes(&state.ckey(&state.ch), msg.as_bytes());
                                let encoded = general_purpose::STANDARD.encode(&encrypted);
                                let _ = state.ws_tx.send(Message::Text(
                                    serde_json::json!({
                                        "t": "msg",
                                        "ch": state.ch,
                                        "c": encoded,
                                        "ts": SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                    })
                                    .to_string(),
                                ));
                            }
                        }
                        "/users" => {
                            let _ = state.ws_tx.send(Message::Text(
                                serde_json::json!({
                                    "t": "users",
                                    "ts": SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                                .to_string(),
                            ));
                        }
                        "/channels" => {
                            let _ = state.ws_tx.send(Message::Text(
                                serde_json::json!({
                                    "t": "info",
                                    "ts": SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                                .to_string(),
                            ));
                        }
                        "/voice" => {
                            // Placeholder for voice toggle
                            state.log("INFO", "Voice command not yet implemented in Rust client");
                        }
                        "/clear" => {
                            // Clear the terminal by printing many newlines
                            for _ in 0..50 {
                                println!();
                            }
                        }
                        "/help" => {
                            state.log("INFO", "Available commands: /join, /dm, /me, /users, /channels, /voice, /clear, /edit, /help, /quit");
                        }
                        "/edit" => {
                            // Placeholder for edit
                            state.log("INFO", "Edit command not yet implemented in Rust client");
                        }
                        "/quit" | "/exit" | "/q" => {
                            state.running = false;
                            break;
                        }
                        _ => {
                            state.log("INFO", &format!("Unknown command: {}", cmd));
                        }
                    }
                } else {
                    // Send as a regular message
                    let encrypted = enc_bytes(&state.ckey(&state.ch), line.as_bytes());
                    let encoded = general_purpose::STANDARD.encode(&encrypted);
                    let _ = state.ws_tx.send(Message::Text(
                        serde_json::json!({
                            "t": "msg",
                            "ch": state.ch,
                            "c": encoded,
                            "ts": SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                            .as_secs()
                        })
                        .to_string(),
                    ));
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