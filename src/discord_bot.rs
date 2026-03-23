//! Discord ↔ Chatify Bridge Bot
//!
//! Bridges messages between Discord and a Chatify WebSocket server.
//! Messages from Discord channels are encrypted and forwarded to Chatify,
//! and messages from Chatify are displayed in the Discord console.
//!
//! Environment variables required:
//! - `DISCORD_TOKEN`: Discord bot authentication token
//! - `CHATIFY_PASSWORD`: Password for Chatify authentication
//! - `CHATIFY_HOST`: Chatify server hostname (default: 127.0.0.1)
//! - `CHATIFY_PORT`: Chatify server port (default: 8765)
//! - `CHATIFY_CHANNEL`: Target Chatify channel (default: general)
//! - `CHATIFY_BOT_USERNAME`: Username for the bot (default: DiscordBot)
//! - `CHATIFY_LOG`: Set to "1" to enable logging

use clicord_server::crypto::dh_key;
use clicord_server::crypto::{channel_key, dec_bytes, enc_bytes, new_keypair, pub_b64, pw_hash};

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serenity::{
    async_trait,
    model::{channel::Message, gateway::GatewayIntents, gateway::Ready},
    prelude::*,
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

type WsSender = mpsc::UnboundedSender<WsMessage>;
type PayloadMap = HashMap<String, serde_json::Value>;

/// Get current Unix timestamp
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn send_ws_json(tx: &WsSender, payload: serde_json::Value) {
    let _ = tx.send(WsMessage::Text(payload.to_string()));
}

fn load_users(users_value: &serde_json::Value, users_map: &DashMap<String, String>) {
    let users: Vec<serde_json::Value> =
        serde_json::from_value(users_value.clone()).unwrap_or_default();
    for user in users {
        if let Some(name) = user.get("u").and_then(|v| v.as_str()) {
            if let Some(pk) = user.get("pk").and_then(|v| v.as_str()) {
                users_map.insert(name.to_string(), pk.to_string());
            }
        }
    }
}

/// Bot state for managing Chatify connection and credentials
struct BotState {
    /// WebSocket sender for Chatify communication
    ws_tx: Option<WsSender>,
    /// Bot's username on Chatify
    username: String,
    /// Password for Chatify authentication
    password: String,
    /// Current Chatify channel
    channel: String,
    /// Bot's private key bytes for Diffie-Hellman exchanges
    priv_key: Vec<u8>,
    /// Known users and their public keys (name -> pubkey_b64)
    users: DashMap<String, String>,
    /// Cached channel-specific encryption keys
    chan_keys: DashMap<String, Vec<u8>>,
    /// Cached DM-specific encryption keys
    dm_keys: DashMap<String, Vec<u8>>,
}

impl BotState {
    /// Create a new bot state with default values
    fn new() -> Self {
        Self {
            ws_tx: None,
            username: String::new(),
            password: String::new(),
            channel: "general".to_string(),
            priv_key: new_keypair(),
            users: DashMap::new(),
            chan_keys: DashMap::new(),
            dm_keys: DashMap::new(),
        }
    }

    /// Get or create a channel-specific encryption key
    fn get_channel_key(&self, ch: &str) -> Vec<u8> {
        if let Some(key) = self.chan_keys.get(ch) {
            key.clone()
        } else {
            let key = channel_key(&self.password, ch);
            self.chan_keys.insert(ch.to_string(), key.clone());
            key
        }
    }

    /// Get or create a DM-specific encryption key
    fn get_dm_key(&self, username: &str) -> Result<Vec<u8>, String> {
        if let Some(key) = self.dm_keys.get(username) {
            return Ok(key.clone());
        }
        let pk = self
            .users
            .get(username)
            .ok_or_else(|| format!("User '{}' not found", username))?;
        let key = dh_key(&self.priv_key, pk.value().as_str());
        if key.len() != 32 {
            return Err(format!("Invalid derived key for '{}'", username));
        }
        self.dm_keys.insert(username.to_string(), key.clone());
        Ok(key)
    }
}

/// Discord event handler for bridging messages
struct DiscordHandler {
    /// Reference to the bot state
    state: Arc<Mutex<BotState>>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    /// Handle incoming Discord messages and forward to Chatify
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from the bot itself
        if msg.author.id == ctx.cache.current_user_id() {
            return;
        }
        let state = self.state.lock().await;

        let content = msg.content.clone();
        let author = msg.author.name.clone();
        let formatted_msg = format!("{}: {}", author, content);

        // Encrypt and send to Chatify
        let key = state.get_channel_key(&state.channel);
        let encrypted = enc_bytes(&key, formatted_msg.as_bytes());
        if encrypted.is_empty() {
            return;
        }
        let encoded = general_purpose::STANDARD.encode(&encrypted);
        let chatify_msg = serde_json::json!({
            "t": "msg",
            "ch": state.channel,
            "c": encoded,
            "ts": now_secs()
        });
        if let Some(ref tx) = state.ws_tx {
            send_ws_json(tx, chatify_msg);
        }
    }

    /// Called when the bot is ready
    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("📡 Discord bot connected as: {}", ready.user.name);
        let mut state = self.state.lock().await;
        state.username = ready.user.name.to_string();
    }
}

/// Handle Chatify channel messages
async fn handle_chatify_msg(data: &PayloadMap, state: &Arc<Mutex<BotState>>) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");

    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };

    let key = {
        let bot_state = state.lock().await;
        bot_state.get_channel_key(ch)
    };

    if let Ok(content) = dec_bytes(&key, &encrypted) {
        let content_str =
            String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
        println!("[Chatify → Discord] {}: {}", u, content_str);
    }
}

/// Handle Chatify system messages
fn handle_system_msg(data: &PayloadMap) {
    let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
    println!("[Chatify → Discord] System: {}", m);
}

/// Handle Chatify direct messages between users
async fn handle_dm_msg(data: &PayloadMap, state: &Arc<Mutex<BotState>>) {
    let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");

    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };

    let dm_key = {
        let bot_state = state.lock().await;
        let peer = if frm == bot_state.username { to } else { frm };
        bot_state.get_dm_key(peer)
    };

    let Ok(dm_key) = dm_key else {
        return;
    };

    if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
        let content_str =
            String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
        println!(
            "[Chatify → Discord] DM from {} to {}: {}",
            frm, to, content_str
        );
    }
}

async fn handle_users_update(data: &PayloadMap, state: &Arc<Mutex<BotState>>) {
    let users_value = data.get("users").cloned().unwrap_or(serde_json::json!([]));
    let bot_state = state.lock().await;
    load_users(&users_value, &bot_state.users);
}

async fn dispatch_chatify_event(data: &PayloadMap, state: &Arc<Mutex<BotState>>) {
    let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "msg" => handle_chatify_msg(data, state).await,
        "sys" => handle_system_msg(data),
        "dm" => handle_dm_msg(data, state).await,
        "users" | "ok" => handle_users_update(data, state).await,
        _ => {}
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables
    let discord_token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in environment");
    let chatify_host = env::var("CHATIFY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let chatify_port = env::var("CHATIFY_PORT").unwrap_or_else(|_| "8765".to_string());
    let chatify_password =
        env::var("CHATIFY_PASSWORD").expect("Expected CHATIFY_PASSWORD in environment");
    let chatify_channel = env::var("CHATIFY_CHANNEL").unwrap_or_else(|_| "general".to_string());
    let chatify_bot_username =
        env::var("CHATIFY_BOT_USERNAME").unwrap_or_else(|_| "DiscordBot".to_string());

    // Set up logging
    if env::var("CHATIFY_LOG").unwrap_or_default() == "1" {
        env_logger::init();
    }

    // Initialize bot state
    let state = Arc::new(Mutex::new(BotState::new()));
    {
        let mut bot_state = state.lock().await;
        bot_state.password = chatify_password;
        bot_state.channel = chatify_channel;
        bot_state.username = chatify_bot_username; // Set the bot's username for chatify
    }

    // Connect to chatify server (WebSocket)
    let scheme = "ws";
    let uri = format!("{}://{}:{}", scheme, chatify_host, chatify_port);
    println!("Connecting to chatify server at {}", uri);
    let (ws_stream, _) = connect_async(&uri)
        .await
        .expect("Failed to connect to chatify");
    println!("Connected to chatify server");
    let (mut ws_write, mut ws_read) = ws_stream.split();

    let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<WsMessage>();
    tokio::spawn(async move {
        while let Some(msg) = bridge_rx.recv().await {
            let _ = ws_write.send(msg).await;
        }
    });

    // Authenticate with chatify
    {
        let bot_state = state.lock().await;
        let auth_msg = serde_json::json!({
            "t": "auth",
            "u": bot_state.username,
            "pw": pw_hash(&bot_state.password),
            "pk": pub_b64(&bot_state.priv_key),
            "status": {"text": "Online", "emoji": "🟢"}
        });
        send_ws_json(&bridge_tx, auth_msg);
    }

    // Wait for auth response to get server's OK and user list
    if let Some(Ok(WsMessage::Text(resp))) = ws_read.next().await {
        let resp_val: serde_json::Value = serde_json::from_str(&resp).expect("Invalid JSON");
        if resp_val["t"] == "err" {
            eprintln!("Authentication failed: {}", resp_val["m"]);
            return;
        }
        // Update bot state with server response
        let mut bot_state = state.lock().await;
        bot_state.username = resp_val["u"]
            .as_str()
            .unwrap_or(&bot_state.username)
            .to_string();
        load_users(&resp_val["users"], &bot_state.users);
        bot_state.ws_tx = Some(bridge_tx.clone());
    } else {
        eprintln!("Authentication failed: no response from chatify server");
        return;
    }

    // Clone state for the WebSocket reading task
    let state_clone = state.clone();
    let ws_rx_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_read.next().await {
            if let WsMessage::Text(text) = msg {
                if let Ok(data) = serde_json::from_str::<PayloadMap>(&text) {
                    dispatch_chatify_event(&data, &state_clone).await;
                }
            }
        }
    });

    // Set up Serenity client
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&discord_token, intents)
        .event_handler(DiscordHandler {
            state: state.clone(),
        })
        .await
        .expect("Error creating Discord client");

    // Start the Discord client
    let discord_task = tokio::spawn(async move {
        if let Err(why) = client.start().await {
            println!("Discord client error: {:?}", why);
        }
    });

    // Wait for either task to finish (they should run until interrupted)
    let _ = ws_rx_task.await;
    let _ = discord_task.await;
}
