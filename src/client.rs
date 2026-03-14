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
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::tungstenite::Error as WsError;
use x25519_dalek::{PublicKey, StaticSecret};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, DisableLineWrap, EnableLineWrap, SetSize},
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

fn enc(key: &[u8], plaintext: &str) -> String {
    let nonce = chaCha20_nonce();
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("Key length must be 32 bytes");
    let ciphertext = cipher.encrypt(nonce.as_ref(), plaintext.as_bytes()).expect("Encryption failure");
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    general_purpose::STANDARD.encode(combined)
}

fn dec(key: &[u8], ciphertext_b64: &str) -> String {
    let combined = general_purpose::STANDARD.decode(ciphertext_b64).expect("Invalid base64");
    if combined.len() < 12 {
        panic!("Ciphertext too short");
    }
    let (nonce, ciphertext) = combined.split_at(12);
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("Key length must be 32 bytes");
    let plaintext = cipher.decrypt(nonce, ciphertext).expect("Decryption failure");
    String::from_utf8(plaintext).expect("Invalid UTF-8")
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
    theme: Theme,
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

#[derive(Debug, Clone)]
struct Theme {
    prompt: Color,
    user: Color,
    system: Color,
    info: Color,
    error: Color,
    timestamp: Color,
    highlight: Color,
}

impl Theme {
    fn default() -> Self {
        Self {
            prompt: Color::Blue,
            user: Color::Cyan,
            system: Color::Yellow,
            info: Color::Green,
            error: Color::Red,
            timestamp: Color::DarkGrey,
            highlight: Color::White,
        }
    }
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
            theme: Theme::default(),
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::error::Error>> {
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

    let password = rpassword::prompt_password_stdout("password: ").unwrap();

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
            theme: Theme::default(),
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

        // Set up terminal for TUI
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Clone state for UI task
        let state_clone = state.clone();
        let ui_task = tokio::spawn(async move {
            loop {
                // Draw UI
                let state = state_clone.lock().unwrap();
                terminal.draw(|f| {
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .margin(1)
                        .constraints([
                            Constraint::Length(3), // Header
                            Constraint::Min(0),    // Message list
                            Constraint::Length(3), // Input
                        ])
                        .split(f.size());

                    // Header
                    let header = Paragraph::new(format!(
                        "{} [{}] ({} online)",
                        state.me,
                        state.ch,
                        state.users.len()
                    ))
                    .style(Style::default().fg(state.theme.prompt))
                    .block(Block::default().borders(Borders::ALL).title("Chatify"));
                    f.render_widget(header, chunks[0]);

                    // Message list
                    let messages: Vec<ListItem> = state
                        .message_history
                        .iter()
                        .map(|msg| {
                            let time = &msg.time;
                            let user = msg.user.as_deref().unwrap_or("?");
                            let text = &msg.text;
                            let style = match msg.msg_type {
                                MessageType::Msg => Style::default().fg(state.theme.user),
                                MessageType::Sys => Style::default().fg(state.theme.system),
                                MessageType::Err => Style::default().fg(state.theme.error),
                                _ => Style::default(),
                            };
                            let content = Line::from(vec![
                                Span::styled(format!("[{}] ", time), Style::default().fg(state.theme.timestamp)),
                                Span::styled(user.to_string(), style),
                                Span::raw(": "),
                                Span::styled(text.to_string(), Style::default()),
                            ]);
                            ListItem::new(content)
                        })
                        .collect();
                    let messages_list = List::new(messages)
                        .block(Block::default().borders(Borders::ALL).title("Messages"));
                    f.render_widget(messages_list, chunks[1]);

                    // Input
                    let input = Paragraph::new("> ")
                        .style(Style::default().fg(state.theme.prompt))
                        .block(Block::default().borders(Borders::ALL).title("Input"));
                    f.render_widget(input, chunks[2]);
                })
                .unwrap();

                // Handle events
                if event::poll(std::time::Duration::from_millis(16)).unwrap() {
                    if let Event::Key(key) = event::read().unwrap() {
                        match key.code {
                            KeyCode::Enter => {
                                // We'll handle input submission later
                                // For now, just break on Enter for testing
                                break;
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Ctrl+C to quit
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        // Task to read from WebSocket and update state
        let state_clone2 = state.clone();
        let rx_task = tokio::spawn(async move {
            while let Some(Ok(msg)) = ws_rx.next().await {
                match msg {
                    Message::Text(text) => {
                        if let Ok(data) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) {
                            // Process message and update state
                            // We'll implement a simplified version
                            let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
                            let ts = data.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
                            match t {
                                "msg" => {
                                    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
                                    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                                    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
                                    if let Ok(content) = dec(&state_clone2.lock().unwrap().ckey(ch), c) {
                                        let mut state = state_clone2.lock().unwrap();
                                        state.message_history.push(DisplayedMessage {
                                            time: format_time(ts),
                                            text: content,
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
                                "sys" => {
                                    let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
                                    let mut state = state_clone2.lock().unwrap();
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
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        // Task to read from stdin and send messages
        let state_clone3 = state.clone();
        let stdin_task = tokio::spawn(async move {
            let mut input = String::new();
            loop {
                input.clear();
                if io::stdin().read_line(&mut input).unwrap() == 0 {
                    break;
                }
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }
                if input.starts_with('/') {
                    // Handle commands
                    // For simplicity, we'll just send as a regular message for now
                    // In a full implementation, we'd parse commands
                }
                // Encrypt and send message
                let state = state_clone3.lock().unwrap();
                if let Ok(encrypted) = enc(&state.ckey(&state.ch), input) {
                    let msg = serde_json::json!({
                        "t": "msg",
                        "ch": state.ch,
                        "c": encrypted,
                        "ts": SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    });
                    let _ = state.ws_tx.send(Message::Text(msg.to_string()));
                }
                // Clear input for next iteration
            }
        });

        // Wait for tasks to complete (they will break on Ctrl+C or Enter in UI)
        let _ = ui_task.await;
        let _ = rx_task.await;
        let _ = stdin_task.await;

        // Restore terminal
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
    }

    Ok(())
}