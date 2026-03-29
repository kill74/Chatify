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
//! - `CHATIFY_WS_SCHEME`: WebSocket scheme (`ws` or `wss`, default: ws)
//! - `CHATIFY_AUTH_TIMEOUT_SECS`: Auth response timeout in seconds (default: 15)
//! - `CHATIFY_RECONNECT_BASE_SECS`: Reconnect base backoff in seconds (default: 1)
//! - `CHATIFY_RECONNECT_MAX_SECS`: Reconnect max backoff in seconds (default: 30)
//! - `CHATIFY_LOG`: Set to "1" to enable logging

use clicord_server::crypto::dh_key;
use clicord_server::crypto::{
    channel_key, dec_bytes, enc_bytes, new_keypair, pub_b64, pw_hash_client,
};

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serenity::{
    async_trait,
    model::{channel::Message, gateway::GatewayIntents, gateway::Ready},
    prelude::*,
};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

type WsSender = mpsc::UnboundedSender<WsMessage>;
type PayloadMap = HashMap<String, serde_json::Value>;
const MAX_DISCORD_CONTENT_LEN: usize = 4000;

fn normalize_chatify_channel(raw: &str) -> Option<String> {
    let ch: String = raw
        .to_lowercase()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if ch.is_empty() {
        None
    } else {
        Some(ch)
    }
}

fn parse_channel_map(raw: &str) -> HashMap<String, String> {
    raw.split(',')
        .filter_map(|entry| {
            let mut parts = entry.splitn(2, ':');
            let discord_channel = parts.next()?.trim();
            let chatify_channel = parts.next()?.trim();
            if discord_channel.is_empty() {
                return None;
            }
            let mapped = normalize_chatify_channel(chatify_channel)?;
            Some((discord_channel.to_string(), mapped))
        })
        .collect()
}

fn is_self_sourced_event(data: &PayloadMap, own_source: &str) -> bool {
    let source = data.get("src").and_then(|v| v.as_str()).unwrap_or("");
    !source.is_empty() && source == own_source
}

struct BridgeMetrics {
    discord_ingress: AtomicU64,
    chatify_ingress: AtomicU64,
    dropped_messages: AtomicU64,
    reconnects: AtomicU64,
}

impl BridgeMetrics {
    fn new() -> Self {
        Self {
            discord_ingress: AtomicU64::new(0),
            chatify_ingress: AtomicU64::new(0),
            dropped_messages: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
        }
    }

    fn log_snapshot(&self) {
        println!(
            "[bridge health] discord_in={} chatify_in={} dropped={} reconnects={}",
            self.discord_ingress.load(Ordering::Relaxed),
            self.chatify_ingress.load(Ordering::Relaxed),
            self.dropped_messages.load(Ordering::Relaxed),
            self.reconnects.load(Ordering::Relaxed),
        );
    }
}

#[derive(Clone)]
struct BridgeConfig {
    discord_token: String,
    chatify_host: String,
    chatify_port: String,
    chatify_password: String,
    chatify_channel: String,
    chatify_bot_username: String,
    chatify_ws_scheme: String,
    auth_timeout_secs: u64,
    reconnect_base_secs: u64,
    reconnect_max_secs: u64,
    health_log_secs: u64,
    instance_id: String,
    channel_map: HashMap<String, String>,
}

impl BridgeConfig {
    fn from_env() -> Self {
        let auth_timeout_secs = env::var("CHATIFY_AUTH_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(15);
        let reconnect_base_secs = env::var("CHATIFY_RECONNECT_BASE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1)
            .max(1);
        let reconnect_max_secs = env::var("CHATIFY_RECONNECT_MAX_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30)
            .max(reconnect_base_secs);
        let health_log_secs = env::var("CHATIFY_HEALTH_LOG_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30)
            .max(5);
        let mut instance_id_bytes = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut instance_id_bytes);
        let instance_id = env::var("CHATIFY_BRIDGE_INSTANCE_ID")
            .unwrap_or_else(|_| hex::encode(instance_id_bytes));
        let channel_map = env::var("CHATIFY_DISCORD_CHANNEL_MAP")
            .ok()
            .map(|s| parse_channel_map(&s))
            .unwrap_or_default();

        Self {
            discord_token: env::var("DISCORD_TOKEN")
                .expect("Expected DISCORD_TOKEN in environment"),
            chatify_host: env::var("CHATIFY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            chatify_port: env::var("CHATIFY_PORT").unwrap_or_else(|_| "8765".to_string()),
            chatify_password: env::var("CHATIFY_PASSWORD")
                .expect("Expected CHATIFY_PASSWORD in environment"),
            chatify_channel: env::var("CHATIFY_CHANNEL").unwrap_or_else(|_| "general".to_string()),
            chatify_bot_username: env::var("CHATIFY_BOT_USERNAME")
                .unwrap_or_else(|_| "DiscordBot".to_string()),
            chatify_ws_scheme: env::var("CHATIFY_WS_SCHEME").unwrap_or_else(|_| "ws".to_string()),
            auth_timeout_secs,
            reconnect_base_secs,
            reconnect_max_secs,
            health_log_secs,
            instance_id,
            channel_map,
        }
    }

    fn uri(&self) -> String {
        format!(
            "{}://{}:{}",
            self.chatify_ws_scheme, self.chatify_host, self.chatify_port
        )
    }
}

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

fn fresh_nonce_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
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
    /// Optional map from Discord channel id -> Chatify channel name.
    channel_map: HashMap<String, String>,
    /// Source marker for loop prevention and tracing.
    bridge_src_tag: String,
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
            channel_map: HashMap::new(),
            bridge_src_tag: "discord-bridge".to_string(),
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
        let key = dh_key(&self.priv_key, pk.value().as_str())?;
        self.dm_keys.insert(username.to_string(), key.clone());
        Ok(key)
    }
}

/// Discord event handler for bridging messages
struct DiscordHandler {
    /// Reference to the bot state
    state: Arc<Mutex<BotState>>,
    metrics: Arc<BridgeMetrics>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    /// Handle incoming Discord messages and forward to Chatify
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore bridge's own messages and messages from any bot account.
        if msg.author.id == ctx.cache.current_user_id() || msg.author.bot {
            return;
        }

        let content = msg.content.trim().to_string();
        if content.is_empty() || content.len() > MAX_DISCORD_CONTENT_LEN {
            self.metrics
                .dropped_messages
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.metrics.discord_ingress.fetch_add(1, Ordering::Relaxed);

        let author = msg.author.name.clone();
        let formatted_msg = format!("{}: {}", author, content);

        // Encrypt and send to Chatify.
        let discord_channel_id = msg.channel_id.to_string();
        let (channel, key, ws_tx, src_tag) = {
            let state = self.state.lock().await;
            let channel = state
                .channel_map
                .get(&discord_channel_id)
                .cloned()
                .unwrap_or_else(|| state.channel.clone());
            let key = state.get_channel_key(&channel);
            let ws_tx = state.ws_tx.clone();
            let src_tag = state.bridge_src_tag.clone();
            (channel, key, ws_tx, src_tag)
        };

        let encrypted = match enc_bytes(&key, formatted_msg.as_bytes()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let encoded = general_purpose::STANDARD.encode(&encrypted);
        let chatify_msg = serde_json::json!({
            "t": "msg",
            "ch": channel,
            "c": encoded,
            "ts": now_secs(),
            "n": fresh_nonce_hex(),
            "src": src_tag,
        });
        if let Some(tx) = ws_tx {
            send_ws_json(&tx, chatify_msg);
        } else {
            eprintln!("Bridge not connected to Chatify; dropping Discord message");
            self.metrics
                .dropped_messages
                .fetch_add(1, Ordering::Relaxed);
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
async fn handle_chatify_msg(
    data: &PayloadMap,
    state: &Arc<Mutex<BotState>>,
    metrics: &Arc<BridgeMetrics>,
) {
    metrics.chatify_ingress.fetch_add(1, Ordering::Relaxed);
    let own_source = {
        let bot_state = state.lock().await;
        bot_state.bridge_src_tag.clone()
    };
    if is_self_sourced_event(data, &own_source) {
        return;
    }

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
fn handle_system_msg(data: &PayloadMap, metrics: &Arc<BridgeMetrics>) {
    metrics.chatify_ingress.fetch_add(1, Ordering::Relaxed);
    let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
    println!("[Chatify → Discord] System: {}", m);
}

/// Handle Chatify direct messages between users
async fn handle_dm_msg(
    data: &PayloadMap,
    state: &Arc<Mutex<BotState>>,
    metrics: &Arc<BridgeMetrics>,
) {
    metrics.chatify_ingress.fetch_add(1, Ordering::Relaxed);
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

async fn dispatch_chatify_event(
    data: &PayloadMap,
    state: &Arc<Mutex<BotState>>,
    metrics: &Arc<BridgeMetrics>,
) {
    let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "msg" => handle_chatify_msg(data, state, metrics).await,
        "sys" => handle_system_msg(data, metrics),
        "dm" => handle_dm_msg(data, state, metrics).await,
        "users" | "ok" => handle_users_update(data, state).await,
        _ => {}
    }
}

async fn run_chatify_session(
    state: Arc<Mutex<BotState>>,
    cfg: &BridgeConfig,
    metrics: Arc<BridgeMetrics>,
) -> Result<(), String> {
    let uri = cfg.uri();
    println!("Connecting to Chatify server at {}", uri);
    let (ws_stream, _) = connect_async(&uri)
        .await
        .map_err(|e| format!("Failed to connect to Chatify: {e}"))?;
    println!("Connected to Chatify server");

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<WsMessage>();
    tokio::spawn(async move {
        while let Some(msg) = bridge_rx.recv().await {
            if let Err(err) = ws_write.send(msg).await {
                eprintln!("Chatify writer task failed: {}", err);
                break;
            }
        }
    });

    // Authenticate with Chatify.
    {
        let bot_state = state.lock().await;
        let auth_msg = serde_json::json!({
            "t": "auth",
            "u": bot_state.username,
            "pw": pw_hash_client(&bot_state.password),
            "pk": pub_b64(&bot_state.priv_key).expect("generated keypair must produce valid public key"),
            "status": {"text": "Online", "emoji": "🟢"}
        });
        send_ws_json(&bridge_tx, auth_msg);
    }

    // Wait for auth response with timeout.
    let auth_reply = timeout(Duration::from_secs(cfg.auth_timeout_secs), ws_read.next())
        .await
        .map_err(|_| "Auth response timeout from Chatify".to_string())?;

    let auth_text = match auth_reply {
        Some(Ok(WsMessage::Text(resp))) => resp,
        Some(Ok(other)) => {
            return Err(format!("Unexpected auth frame from Chatify: {:?}", other));
        }
        Some(Err(e)) => return Err(format!("WebSocket error during auth: {e}")),
        None => return Err("Chatify closed connection before auth completed".to_string()),
    };

    let resp_val: serde_json::Value =
        serde_json::from_str(&auth_text).map_err(|e| format!("Invalid auth JSON: {e}"))?;
    let typ = resp_val.get("t").and_then(|v| v.as_str()).unwrap_or("");
    if typ == "err" {
        return Err(format!(
            "Authentication failed: {}",
            resp_val["m"].as_str().unwrap_or("unknown error")
        ));
    }
    if typ != "ok" {
        return Err(format!("Unexpected auth response type: {}", typ));
    }
    if !resp_val.get("users").map(|v| v.is_array()).unwrap_or(false) {
        return Err("Malformed auth users payload".to_string());
    }

    {
        let mut bot_state = state.lock().await;
        bot_state.username = resp_val["u"]
            .as_str()
            .unwrap_or(&bot_state.username)
            .to_string();
        load_users(&resp_val["users"], &bot_state.users);
        bot_state.ws_tx = Some(bridge_tx.clone());
    }
    println!("Chatify bridge authenticated as connected user");

    while let Some(frame) = ws_read.next().await {
        match frame {
            Ok(WsMessage::Text(text)) => {
                if let Ok(data) = serde_json::from_str::<PayloadMap>(&text) {
                    dispatch_chatify_event(&data, &state, &metrics).await;
                } else {
                    eprintln!("Received non-JSON Chatify event; ignoring");
                    metrics.dropped_messages.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(WsMessage::Close(_)) => {
                return Err("Chatify connection closed".to_string());
            }
            Ok(_) => {}
            Err(e) => {
                return Err(format!("Chatify websocket read error: {e}"));
            }
        }
    }

    Err("Chatify websocket stream ended".to_string())
}

async fn bridge_supervisor(
    state: Arc<Mutex<BotState>>,
    cfg: BridgeConfig,
    metrics: Arc<BridgeMetrics>,
) {
    let mut backoff = cfg.reconnect_base_secs;
    loop {
        match run_chatify_session(state.clone(), &cfg, metrics.clone()).await {
            Ok(()) => {
                backoff = cfg.reconnect_base_secs;
            }
            Err(err) => {
                metrics.reconnects.fetch_add(1, Ordering::Relaxed);
                {
                    let mut bot_state = state.lock().await;
                    bot_state.ws_tx = None;
                }
                eprintln!(
                    "Bridge session ended: {}. Reconnecting in {}s...",
                    err, backoff
                );
                sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff.saturating_mul(2)).min(cfg.reconnect_max_secs);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables.
    let cfg = BridgeConfig::from_env();

    // Set up logging.
    if env::var("CHATIFY_LOG").unwrap_or_default() == "1" {
        env_logger::init();
    }

    // Initialize bot state.
    let state = Arc::new(Mutex::new(BotState::new()));
    {
        let mut bot_state = state.lock().await;
        bot_state.password = cfg.chatify_password.clone();
        bot_state.channel = cfg.chatify_channel.clone();
        bot_state.username = cfg.chatify_bot_username.clone();
        bot_state.channel_map = cfg.channel_map.clone();
        bot_state.bridge_src_tag = format!("discord-bridge:{}", cfg.instance_id);
    }

    let metrics = Arc::new(BridgeMetrics::new());
    if cfg.health_log_secs > 0 {
        let metrics_clone = metrics.clone();
        let interval_secs = cfg.health_log_secs;
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(interval_secs)).await;
                metrics_clone.log_snapshot();
            }
        });
    }

    // Start Chatify bridge supervisor with reconnect policy.
    let bridge_task = tokio::spawn(bridge_supervisor(
        state.clone(),
        cfg.clone(),
        metrics.clone(),
    ));

    // Set up Serenity client.
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&cfg.discord_token, intents)
        .event_handler(DiscordHandler {
            state: state.clone(),
            metrics: metrics.clone(),
        })
        .await
        .expect("Error creating Discord client");

    // Start the Discord client.
    let discord_task = tokio::spawn(async move {
        if let Err(why) = client.start().await {
            println!("Discord client error: {:?}", why);
        }
    });

    tokio::select! {
        _ = bridge_task => {
            eprintln!("Bridge supervisor task ended unexpectedly");
        }
        _ = discord_task => {
            eprintln!("Discord task ended unexpectedly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn spawn_mock_chatify_server(
        attempts: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock chatify server");
        let addr = listener.local_addr().expect("mock server local addr");
        let uri = format!("ws://{}:{}", addr.ip(), addr.port());

        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let attempts = attempts.clone();
                tokio::spawn(async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let mut ws = match accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(_) => return,
                    };

                    // Expect auth request from bridge and acknowledge it.
                    match ws.next().await {
                        Some(Ok(WsMessage::Text(_auth))) => {
                            let ok = serde_json::json!({
                                "t": "ok",
                                "u": "DiscordBot",
                                "users": []
                            });
                            let _ = ws.send(WsMessage::Text(ok.to_string())).await;
                        }
                        _ => return,
                    }

                    // Force session termination so supervisor must reconnect.
                    let _ = ws.close(None).await;
                });
            }
        });

        (uri, task)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bridge_supervisor_reconnects_after_disconnect() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (uri, mock_server_task) = spawn_mock_chatify_server(attempts.clone()).await;

        let ws_addr = uri.trim_start_matches("ws://");
        let mut parts = ws_addr.split(':');
        let host = parts.next().expect("host part in ws uri").to_string();
        let port = parts.next().expect("port part in ws uri").to_string();

        let state = Arc::new(Mutex::new(BotState::new()));
        {
            let mut bot_state = state.lock().await;
            bot_state.password = "test-password".to_string();
            bot_state.channel = "general".to_string();
            bot_state.username = "DiscordBot".to_string();
        }

        let cfg = BridgeConfig {
            discord_token: String::new(),
            chatify_host: host,
            chatify_port: port,
            chatify_password: "test-password".to_string(),
            chatify_channel: "general".to_string(),
            chatify_bot_username: "DiscordBot".to_string(),
            chatify_ws_scheme: "ws".to_string(),
            auth_timeout_secs: 2,
            reconnect_base_secs: 1,
            reconnect_max_secs: 1,
            health_log_secs: 30,
            instance_id: "test-instance".to_string(),
            channel_map: HashMap::new(),
        };

        let supervisor_task = tokio::spawn(bridge_supervisor(
            state,
            cfg,
            Arc::new(BridgeMetrics::new()),
        ));

        let wait_result = timeout(Duration::from_secs(5), async {
            loop {
                if attempts.load(Ordering::SeqCst) >= 2 {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        supervisor_task.abort();
        mock_server_task.abort();

        assert!(
            wait_result.is_ok(),
            "bridge did not reconnect within timeout"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "expected at least two bridge connection attempts"
        );
    }

    #[test]
    fn parse_channel_map_filters_and_normalizes_entries() {
        let parsed =
            parse_channel_map("123:General, ,bad,456:#Team-42,789:***,abc:Alpha_beta,123:override");

        assert_eq!(parsed.get("123"), Some(&"override".to_string()));
        assert_eq!(parsed.get("456"), Some(&"team-42".to_string()));
        assert_eq!(parsed.get("abc"), Some(&"alpha_beta".to_string()));
        assert!(!parsed.contains_key("789"));
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn self_source_filter_matches_only_non_empty_identical_source() {
        let own = "discord-bridge:test-instance";
        let mut from_self = PayloadMap::new();
        from_self.insert("src".to_string(), serde_json::json!(own));
        assert!(is_self_sourced_event(&from_self, own));

        let mut from_other = PayloadMap::new();
        from_other.insert("src".to_string(), serde_json::json!("discord-bridge:other"));
        assert!(!is_self_sourced_event(&from_other, own));

        let mut empty_src = PayloadMap::new();
        empty_src.insert("src".to_string(), serde_json::json!(""));
        assert!(!is_self_sourced_event(&empty_src, own));

        let missing_src = PayloadMap::new();
        assert!(!is_self_sourced_event(&missing_src, own));
    }

    #[test]
    fn self_source_filter_ignores_non_string_src_values() {
        let own = "discord-bridge:test-instance";

        let mut numeric_src = PayloadMap::new();
        numeric_src.insert("src".to_string(), serde_json::json!(1234));
        assert!(!is_self_sourced_event(&numeric_src, own));

        let mut bool_src = PayloadMap::new();
        bool_src.insert("src".to_string(), serde_json::json!(true));
        assert!(!is_self_sourced_event(&bool_src, own));

        let mut object_src = PayloadMap::new();
        object_src.insert("src".to_string(), serde_json::json!({"id": own}));
        assert!(!is_self_sourced_event(&object_src, own));
    }
}
