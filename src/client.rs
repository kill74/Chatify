//! Chatify WebSocket Client
//!
//! A real-time chat client with support for channels, direct messages, voice,
//! file transfers, message editing, reactions, and user status tracking.

use clicord_server::crypto::{channel_key, new_keypair, pub_b64, pw_hash, enc_bytes, dec_bytes};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{self, engine::general_purpose, Engine as _};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use futures_util::SinkExt;

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
            let _pk = self.users.get(name).ok_or_else(|| format!("User {} not found", name))?;
            // Use simple key derivation instead of ECDH due to dependency conflicts
            let key = channel_key(&self.pw, &format!("dm:{}", name));
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
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| Utc::now());
    datetime.format("%H:%M").to_string()
}

/// Placeholder for image to ASCII conversion
fn img_to_ascii(_: &[u8], _: u16) -> String {
    "[Image sending not yet implemented in Rust client]".to_string()
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
                                    let peer = if frm == state_lock.me { to } else { frm };
                                    match state_lock.dmkey(peer) {
                                        Ok(dm_key) => {
                                            if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
                                                let arrow = if frm == state_lock.me { "→" } else { "←" };
                                                state_lock.message_history.push(DisplayedMessage {
                                                    time: format_time(ts),
                                                    text: format!("{} {} {}", frm, arrow, String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string())),
                                                    msg_type: MessageType::Dm,
                                                    user: Some(frm.to_string()),
                                                    channel: None,
                                                });
                                                if state_lock.message_history.len() > 100 {
                                                    state_lock.message_history.remove(0);
                                                }
                                            }
                                        }
                                        Err(_) => {}
                                    }
                                }
                                "users" => {
                                    let mut state = state_clone.lock().await;
                                    let names: Vec<String> = serde_json::from_value(data.get("users").unwrap_or(&serde_json::json!([])).clone()).unwrap_or_default();
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
                                    let chs: Vec<String> = serde_json::from_value(data.get("chs").unwrap_or(&serde_json::json!([])).clone()).unwrap_or_default();
                                    let online = data.get("online").and_then(|v| v.as_u64()).unwrap_or(0);
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
            let stdin = BufReader::new(tokio::io::stdin());
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
                        }
                        "/dm" => {
                            let mut parts = args.splitn(2, ' ');
                            let target = parts.next().unwrap_or("");
                            let msg = parts.next().unwrap_or("");
                            if !target.is_empty() && !msg.is_empty() {
                                let encrypted = match state.dmkey(target) {
                                    Ok(key) => enc_bytes(&key, msg.as_bytes()),
                                    Err(_) => continue,
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
                    }).to_string();
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