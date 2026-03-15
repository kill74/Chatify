use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{self, engine::general_purpose, Engine as _};
use chacha20poly1305::{ChaCha20Poly1305};
use chacha20poly1305::aead::{Aead, NewAead};
use dashmap::DashMap;
use hex::{self, FromHex};
use pbkdf2::pbkdf2;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::Sha256;
use serenity::{
    async_trait,
    model::{channel::Message, gateway::Ready, gateway::GatewayIntents},
    prelude::*,
};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use tokio::io::{self as tokio_io, AsyncBufReadExt, AsyncWriteExt};
use x25519_dalek::{PublicKey, StaticSecret};

use chrono::{DateTime, Utc};

// Utility functions (same as in client)
fn channel_key(password: &str, channel: &str) -> Vec<u8> {
    let mut key = [0u8; 32];
    pbkdf2::<Sha256>(password.as_bytes(), format!("chatify:{}", channel).as_bytes(), 120000, &mut key);
    key.to_vec()
}

fn dh_key(priv_key: &StaticSecret, pubkey_b64: &str) -> Vec<u8> {
    let pubkey_bytes = general_purpose::STANDARD.decode(pubkey_b64).expect("Invalid base64");
    let pubkey = PublicKey::from(pubkey_bytes.as_slice());
    let secret = priv_key.diffie_hellman(&pubkey);
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

fn pub_b64(priv_key: &StaticSecret) -> String {
    let pubkey = PublicKey::from(priv_key);
    general_purpose::STANDARD.encode(pubkey.as_bytes())
}

// Helper to format timestamp (not used heavily in bot, but kept)
fn format_time(ts: u64) -> String {
    let datetime = DateTime::<Utc>::from_timestamp(ts as i64, 0).unwrap_or_else(|| Utc::now());
    datetime.format("%H:%M").to_string()
}

// Bot struct that holds state
struct Bot {
    chatify_ws_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    chatify_me: String,
    chatify_pw: String,
    chatify_ch: String,
    chatify_priv_key: StaticSecret,
    chatify_users: DashMap<String, String>, // name -> pubkey_b64
    chatify_chan_keys: DashMap<String, Vec<u8>>,
    chatify_dm_keys: DashMap<String, Vec<u8>>,
}

impl Bot {
    fn new() -> Self {
        Self {
            chatify_ws_tx: None,
            chatify_me: String::new(),
            chatify_pw: String::new(),
            chatify_ch: "general".to_string(),
            chatify_priv_key: StaticSecret::new(OsRng),
            chatify_users: DashMap::new(),
            chatify_chan_keys: DashMap::new(),
            chatify_dm_keys: DashMap::new(),
        }
    }

    // Helper to get or create channel key
    fn ckey(&self, ch: &str) -> Vec<u8> {
        if let Some(key) = self.chatify_chan_keys.get(ch) {
            key.clone()
        } else {
            let key = channel_key(&self.chatify_pw, ch);
            self.chatify_chan_keys.insert(ch.to_string(), key.clone());
            key
        }
    }

    // Helper to get or create DM key
    fn dmkey(&self, name: &str) -> Vec<u8> {
        if let Some(key) = self.chatify_dm_keys.get(name) {
            key.clone()
        } else {
            let pk = self.chatify_users.get(name).expect("User not found");
            let pk_bytes = Vec::from_hex(pk).expect("Invalid pubkey");
            let key = dh_key(&self.chatify_priv_key, &pk_bytes);
            self.chatify_dm_keys.insert(name.to_string(), key.clone());
            key
        }
    }
}

// Serenity event handler
struct Handler {
    bot: Arc<Mutex<Bot>>,
}

#[async_trait]
impl EventHandler for Handler {
    // Called when a message is received in Discord
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from the bot itself
        if msg.author.id == ctx.cache.current_user_id() {
            return;
        }
        let bot_lock = self.bot.lock().await;
        let mut bot = bot_lock.lock().await;

        // Only forward messages from a specific Discord channel (optional: you can make this configurable)
        // For simplicity, forward all messages to chatify channel set in bot (e.g., general)
        let content = msg.content.clone();
        let author = msg.author.name.clone();

        // Encrypt and send to chatify
        if let Ok(encrypted) = enc_bytes(&bot.ckey(&bot.chatify_ch), format!("{}: {}", author, content).as_bytes()) {
            let encoded = general_purpose::STANDARD.encode(&encrypted);
            let chatify_msg = serde_json::json!({
                "t": "msg",
                "ch": bot.chatify_ch,
                "c": encoded,
                "ts": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            if let Some(ref tx) = bot.chatify_ws_tx {
                let _ = tx.send(WsMessage::Text(chatify_msg.to_string()));
            }
        }
    }

    // Called when the bot is ready
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
        // Store the bot's username for filtering
        let bot_lock = self.bot.lock().await;
        let mut bot = bot_lock.lock().await;
        bot.chatify_me = ready.user.name.to_string();
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables
    let discord_token = env::var("DISCORD_TOKEN")
        .expect("Expected DISCORD_TOKEN in environment");
    let chatify_host = env::var("CHATIFY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let chatify_port = env::var("CHATIFY_PORT").unwrap_or_else(|_| "8765".to_string());
    let chatify_password = env::var("CHATIFY_PASSWORD")
        .expect("Expected CHATIFY_PASSWORD in environment");
    let chatify_channel = env::var("CHATIFY_CHANNEL").unwrap_or_else(|_| "general".to_string());
    let chatify_bot_username = env::var("CHATIFY_BOT_USERNAME").unwrap_or_else(|_| "DiscordBot".to_string());

    // Set up logging
    if env::var("CHATIFY_LOG").unwrap_or_default() == "1" {
        env_logger::init();
    }

    // Initialize bot state
    let bot = Arc::new(Mutex::new(Bot::new()));
    {
        let mut bot_lock = bot.lock().await;
        let mut bot = bot_lock.lock().await;
        bot.chatify_pw = chatify_password;
        bot.chatify_ch = chatify_channel;
        bot.chatify_me = chatify_bot_username; // Set the bot's username for chatify
    }

    // Connect to chatify server (WebSocket)
    let scheme = "ws";
    let uri = format!("{}://{}:{}", scheme, chatify_host, chatify_port);
    println!("Connecting to chatify server at {}", uri);
    let (ws_stream, _) = connect_async(&uri).await.expect("Failed to connect to chatify");
    println!("Connected to chatify server");
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Authenticate with chatify
    {
        let bot_lock = bot.lock().await;
        let mut bot = bot_lock.lock().await;
        let auth_msg = serde_json::json!({
            "t": "auth",
            "u": bot.chatify_me,
            "pw": pw_hash(&bot.chatify_pw),
            "pk": pub_b64(&new_keypair()),
            "status": {"text": "Online", "emoji": "🟢"}
        });
        ws_tx.send(WsMessage::Text(auth_msg.to_string())).await.expect("Failed to send auth");
    }

    // Wait for auth response to get server's OK and user list
    if let Some(Ok(WsMessage::Text(resp))) = ws_rx.next().await {
        let resp_val: serde_json::Value = serde_json::from_str(&resp).expect("Invalid JSON");
        if resp_val["t"] == "err" {
            eprintln!("Authentication failed: {}", resp_val["m"]);
            return;
        }
        // Update bot state with server response
        let mut bot_lock = bot.lock().await;
        let mut bot = bot_lock.lock().await;
        bot.chatify_me = resp_val["u"].as_str().unwrap_or(&bot.chatify_me).to_string();
        let users: Vec<serde_json::Value> = serde_json::from_value(resp_val["users"].clone()).expect("Invalid users");
        for u in users {
            if let Some(name) = u["u"].as_str() {
                if let Some(pk) = u["pk"].as_str() {
                    bot.chatify_users.insert(name.to_string(), pk.to_string());
                }
            }
        }
        // Set the WS tx in bot for sending messages
        bot.chatify_ws_tx = Some(mpsc::UnboundedSender::clone(&ws_tx));
    }

    // Clone bot for the WebSocket reading task
    let bot_clone = bot.clone();
    let ws_rx_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                WsMessage::Text(text) => {
                    if let Ok(data) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) {
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
                                if let Ok(content) = dec_bytes(&{
                                    let bot_lock = bot_clone.lock().await;
                                    bot_lock.ckey(ch)
                                }, &encrypted) {
                                    let content_str = String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
                                    // Send to Discord (we need to get a channel ID to send to; for simplicity, we'll send to the first text channel the bot can see)
                                    // We'll use a simple approach: send to the same Discord channel that triggered the bridge? 
                                    // Since we don't have context here, we'll just print to console; in a real bot you'd store a target channel ID.
                                    println!("[Chatify → Discord] {}: {}", u, content_str);
                                    // To actually send to Discord, you'd need to store a Serenity Context and use it to send a message.
                                    // This is left as an exercise; the bot framework can be extended.
                                }
                            }
                            "sys" => {
                                let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
                                println!("[Chatify → Discord] System: {}", m);
                            }
                            "dm" => {
                                let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                                let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
                                let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
                                let encrypted = match general_purpose::STANDARD.decode(c) {
                                    Ok(bytes) => bytes,
                                    Err(_) => continue,
                                };
                                if let Ok(content) = dec_bytes(&{
                                    let bot_lock = bot_clone.lock().await;
                                    bot_lock.dmkey(if frm == bot_clone.lock().await.chatify_me { &to } else { &frm })
                                }, &encrypted) {
                                    let content_str = String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
                                    println!("[Chatify → Discord] DM from {} to {}: {}", frm, to, content_str);
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

    // Set up Serenity client
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&discord_token, intents)
        .event_handler(Handler {
            bot: bot.clone(),
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