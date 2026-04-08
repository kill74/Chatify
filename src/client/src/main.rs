use std::io::{self, Write};
use std::sync::Arc;

use clap::Parser;
use clifford::crypto::{new_keypair, pub_b64, pw_hash_client};
use futures_util::{SinkExt, StreamExt};
use log::info;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use clifford::config::Config;
use clifford::error::{ChatifyError, ChatifyResult};

use clifford_client::{
    args::Args,
    handlers,
    state::{ClientState, SharedState},
};

const DEFAULT_HISTORY_LIMIT: usize = 50;
const MAX_HISTORY_LIMIT: usize = 500;
const DEFAULT_RECENT_LIMIT: usize = 10;
const MAX_RECENT_LIMIT: usize = 50;
const DEFAULT_REACTION_SYNC_LIMIT: usize = 500;

fn is_valid_reaction_emoji(emoji: &str) -> bool {
    let trimmed = emoji.trim();
    !trimmed.is_empty() && trimmed.len() <= 32
}

fn prompt_input(label: &str, default: Option<&str>) -> ChatifyResult<String> {
    if let Some(default_value) = default {
        print!("{} [{}]: ", label, default_value);
    } else {
        print!("{}: ", label);
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim();
    if value.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(value.to_string())
    }
}

fn sanitize_username(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}

fn print_help() {
    println!("Commands:");
    println!("  /help                          Show this help");
    println!("  /join <channel>                Join or switch to a channel");
    println!("  /history [channel] [limit]     Load channel history");
    println!("  /users                         Request online users");
    println!("  /recent [n]                    Show recent message IDs");
    println!("  /react <msg_id|#index> <emoji> React to a message");
    println!("  /sync                          Request reaction sync for active channel");
    println!("  /quit                          Exit client");
    println!("  <text>                         Send a message to active channel");
}

fn print_prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}

async fn print_recent_messages(state: &SharedState, limit: usize) {
    let state_lock = state.lock().await;
    let mut shown = 0usize;
    for msg in state_lock.message_history.iter().rev() {
        if msg.id.is_empty() {
            continue;
        }

        let reaction_summary = state_lock.reaction_summary(&msg.id);
        let short_id: String = msg.id.chars().take(8).collect();
        if reaction_summary.is_empty() {
            println!("#{} {}: {}", short_id, msg.sender, msg.content);
        } else {
            println!(
                "#{} {}: {} {}",
                short_id, msg.sender, msg.content, reaction_summary
            );
        }

        shown += 1;
        if shown >= limit {
            break;
        }
    }

    if shown == 0 {
        println!("No message IDs available yet. Send or load history first.");
    }
}

async fn handle_user_input(state: &SharedState, input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return true;
    }

    if !trimmed.starts_with('/') {
        let state_lock = state.lock().await;
        let channel = state_lock.ch.clone();
        if let Err(err) = state_lock.send_message(&channel, trimmed) {
            eprintln!("failed to send message: {}", err);
        }
        return true;
    }

    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next().unwrap_or("");

    match cmd {
        "/help" => {
            print_help();
        }
        "/quit" | "/exit" | "/q" => {
            return false;
        }
        "/join" | "/switch" => {
            let Some(channel_raw) = parts.next() else {
                println!("Usage: /join <channel>");
                return true;
            };
            let channel =
                clifford::normalize_channel(channel_raw).unwrap_or_else(|| "general".to_string());

            let mut state_lock = state.lock().await;
            state_lock.ch = channel.clone();
            if let Err(err) = state_lock.send_join(&channel) {
                eprintln!("failed to join channel: {}", err);
            }
        }
        "/history" => {
            let channel = if let Some(ch) = parts.next() {
                clifford::normalize_channel(ch).unwrap_or_else(|| "general".to_string())
            } else {
                state.lock().await.ch.clone()
            };
            let limit = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(DEFAULT_HISTORY_LIMIT)
                .clamp(1, MAX_HISTORY_LIMIT);

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_history(&channel, limit) {
                eprintln!("failed to request history: {}", err);
            }
        }
        "/users" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                eprintln!("failed to request users: {}", err);
            }
        }
        "/recent" => {
            let limit = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(DEFAULT_RECENT_LIMIT)
                .clamp(1, MAX_RECENT_LIMIT);
            print_recent_messages(state, limit).await;
        }
        "/sync" => {
            let state_lock = state.lock().await;
            let channel = state_lock.ch.clone();
            if let Err(err) = state_lock.send_reaction_sync(&channel, DEFAULT_REACTION_SYNC_LIMIT) {
                eprintln!("failed to sync reactions: {}", err);
            }
        }
        "/react" => {
            let Some(msg_ref) = parts.next() else {
                println!("Usage: /react <msg_id|#index> <emoji>");
                return true;
            };
            let Some(emoji) = parts.next() else {
                println!("Usage: /react <msg_id|#index> <emoji>");
                return true;
            };
            let emoji = emoji.trim();
            if !is_valid_reaction_emoji(emoji) {
                println!("Invalid emoji token. Use a non-empty value up to 32 bytes.");
                return true;
            }

            let state_lock = state.lock().await;
            let channel = state_lock.ch.clone();
            let resolved_msg_id = if let Some(index_str) = msg_ref.strip_prefix('#') {
                let Some(index) = index_str.parse::<usize>().ok() else {
                    println!(
                        "Invalid message index '{}'. Use e.g. #1 for most recent.",
                        index_str
                    );
                    return true;
                };
                let Some(msg_id) = state_lock.resolve_recent_message_id_in_channel(&channel, index)
                else {
                    println!(
                        "Could not resolve message index #{} in channel #{}.",
                        index, channel
                    );
                    return true;
                };
                msg_id
            } else {
                msg_ref.to_string()
            };

            if let Err(err) = state_lock.send_reaction(&channel, &resolved_msg_id, emoji) {
                eprintln!("failed to send reaction: {}", err);
            } else {
                println!(
                    "reaction sent: {} -> #{}",
                    emoji,
                    resolved_msg_id.chars().take(8).collect::<String>()
                );
            }
        }
        _ => {
            println!("Unknown command. Type /help.");
        }
    }

    true
}

async fn run_input_loop(state: SharedState) {
    print_help();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print_prompt();
        let Ok(next) = lines.next_line().await else {
            break;
        };
        let Some(line) = next else {
            break;
        };

        if !handle_user_input(&state, &line).await {
            break;
        }
    }
}

#[tokio::main]
async fn main() -> ChatifyResult<()> {
    let args = Args::parse();
    let config = Config::load();
    let client_config = args.merge_with_config(&config);

    if client_config.log_enabled {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    let uri = client_config.uri();
    info!("Connecting to {}", uri);

    let default_username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string());
    let input_username = prompt_input("Username", Some(&default_username))?;
    let username = sanitize_username(&input_username);

    let mut password = String::new();
    while password.is_empty() {
        password = prompt_input("Password", None)?;
        if password.is_empty() {
            println!("Password cannot be empty.");
        }
    }
    let pw_hash = pw_hash_client(&password).map_err(ChatifyError::Validation)?;

    let (ws_stream, _) = connect_async(&uri).await?;
    let (mut write, mut read) = ws_stream.split();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let state: SharedState = Arc::new(Mutex::new(ClientState::new(
        tx,
        client_config.clone(),
        config,
    )));

    let priv_key = new_keypair();
    let pub_key = pub_b64(&priv_key).map_err(ChatifyError::Crypto)?;

    {
        let mut state_lock = state.lock().await;
        state_lock.me = username.clone();
        state_lock.pw = pw_hash.clone();
        state_lock.priv_key = priv_key;
    }

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let recv_state = state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(map) = data.as_object() {
                            handlers::dispatch_event(&recv_state, map).await;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });

    {
        let state_lock = state.lock().await;
        state_lock
            .send_json(serde_json::json!({
                "t": "auth",
                "u": username,
                "pw": pw_hash,
                "pk": pub_key,
                "status": {"text": "Online", "emoji": ""}
            }))
            .map_err(ChatifyError::Message)?;

        state_lock
            .send_join("general")
            .map_err(ChatifyError::Message)?;
    }

    let mut input_task = tokio::spawn(run_input_loop(state.clone()));

    tokio::select! {
        _ = &mut input_task => {}
        _ = &mut recv_task => {}
    }

    send_task.abort();
    recv_task.abort();
    input_task.abort();

    println!("Disconnected");
    Ok(())
}
