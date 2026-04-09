use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
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
const DEFAULT_SEARCH_LIMIT: usize = 50;
const MAX_SEARCH_LIMIT: usize = 200;
const DEFAULT_REPLAY_LIMIT: usize = 1000;
const MAX_REPLAY_LIMIT: usize = 5000;
const DEFAULT_RECENT_LIMIT: usize = 10;
const MAX_RECENT_LIMIT: usize = 50;
const DEFAULT_REACTION_SYNC_LIMIT: usize = 500;
const DEFAULT_TRUST_AUDIT_LIMIT: usize = 20;
const MAX_TRUST_AUDIT_LIMIT: usize = 200;

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

fn resolve_known_username(users: &HashMap<String, String>, raw: &str) -> Result<String, String> {
    let query = raw.trim().trim_start_matches('@');
    if query.is_empty() {
        return Err("user cannot be empty".to_string());
    }

    if users.contains_key(query) {
        return Ok(query.to_string());
    }

    let needle = query.to_ascii_lowercase();
    let mut matches: Vec<String> = users
        .keys()
        .filter(|candidate| candidate.to_ascii_lowercase() == needle)
        .cloned()
        .collect();

    match matches.len() {
        0 => Err(format!(
            "unknown user '{}'; run /users to refresh directory",
            query
        )),
        1 => Ok(matches.remove(0)),
        _ => {
            matches.sort();
            Err(format!(
                "ambiguous user '{}'; matches: {}",
                query,
                matches.join(", ")
            ))
        }
    }
}

fn trust_status_label(state: &ClientState, user: &str, observed_fingerprint: &str) -> &'static str {
    match state.trust_store.peers.get(user) {
        Some(peer) if peer.fingerprint == observed_fingerprint && peer.verified => "trusted",
        Some(peer) if peer.fingerprint == observed_fingerprint => "known",
        Some(_) => "key-changed",
        None => "untrusted",
    }
}

fn is_explicit_scope_token(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.starts_with('#') || trimmed.to_ascii_lowercase().starts_with("dm:")
}

fn normalize_scope_token(raw: &str, fallback_channel: &str, allow_plain_channel: bool) -> String {
    let trimmed = raw.trim();
    if let Some(peer_raw) = trimmed.strip_prefix("dm:") {
        let peer = peer_raw.trim().to_ascii_lowercase();
        return format!("dm:{}", peer);
    }

    if trimmed.starts_with('#') || allow_plain_channel {
        let channel_raw = trimmed.trim_start_matches('#');
        return clifford::normalize_channel(channel_raw)
            .unwrap_or_else(|| fallback_channel.to_string());
    }

    fallback_channel.to_string()
}

fn parse_limit_token(raw: &str) -> Option<usize> {
    raw.strip_prefix("limit=")
        .or_else(|| raw.strip_prefix("l="))
        .and_then(|v| v.parse::<usize>().ok())
}

fn parse_replay_timestamp(raw: &str) -> Option<f64> {
    let ts = raw.parse::<f64>().ok()?;
    if ts.is_finite() && ts >= 0.0 {
        Some(ts)
    } else {
        None
    }
}

fn format_scope_for_help(scope: &str) -> String {
    if scope.starts_with("dm:") {
        scope.to_string()
    } else {
        format!("#{}", scope)
    }
}

fn print_help() {
    println!("Commands:");
    println!("  /help                          Show this help");
    println!("  /join <channel>                Join or switch to a channel");
    println!("  /leave [channel]               Leave a channel (default: current)");
    println!("  /history [ch|dm:user] [limit]  Load channel or DM history");
    println!("  /search [#ch|dm:user] <query> [limit=N]   Search timeline events");
    println!("  /replay <from_ts> [#ch|dm:user] [limit=N] Replay events from timestamp");
    println!("  /users                         Refresh online users and key directory");
    println!("  /metrics                       Show runtime and database metrics");
    println!("  /db-profile                    Show detailed database latency profile");
    println!("  /dm <user> <message>           Send direct message (trust-verified)");
    println!("  /fingerprint [user]            Show current key fingerprint(s)");
    println!("  /trust <user> <fingerprint>    Trust a peer fingerprint");
    println!("  /trust-audit [n]               Show recent trust audit entries");
    println!("  /trust-export [path]           Export deterministic trust audit JSON");
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
        "/leave" | "/part" => {
            let maybe_channel_raw = parts.next();
            if parts.next().is_some() {
                println!("Usage: /leave [channel]");
                return true;
            }

            let channel = if let Some(channel_raw) = maybe_channel_raw {
                clifford::normalize_channel(channel_raw).unwrap_or_else(|| "general".to_string())
            } else {
                let current = state.lock().await.ch.clone();
                if current.starts_with("dm:") {
                    println!("Usage: /leave <channel>");
                    return true;
                }
                current
            };

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_leave(&channel) {
                eprintln!("failed to leave channel: {}", err);
            }
        }
        "/history" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            let mut scope = current_scope.clone();
            let mut limit = DEFAULT_HISTORY_LIMIT;

            if let Some(first) = tokens.first().copied() {
                if let Ok(parsed_limit) = first.parse::<usize>() {
                    limit = parsed_limit.clamp(1, MAX_HISTORY_LIMIT);
                    let _ = tokens.remove(0);
                } else {
                    scope = normalize_scope_token(first, &current_scope, true);
                    let _ = tokens.remove(0);
                }
            }

            if let Some(next) = tokens.first().copied() {
                if let Ok(parsed_limit) = next.parse::<usize>() {
                    limit = parsed_limit.clamp(1, MAX_HISTORY_LIMIT);
                    let _ = tokens.remove(0);
                } else {
                    println!("Usage: /history [ch|dm:user] [limit]");
                    return true;
                }
            }

            if !tokens.is_empty() {
                println!("Usage: /history [ch|dm:user] [limit]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_history(&scope, limit) {
                eprintln!("failed to request history: {}", err);
            } else {
                println!(
                    "history request sent for {} (limit={})",
                    format_scope_for_help(&scope),
                    limit
                );
            }
        }
        "/search" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            if tokens.is_empty() {
                println!("Usage: /search [#ch|dm:user] <query> [limit=N]");
                return true;
            }

            let mut scope = current_scope.clone();
            if let Some(first) = tokens.first().copied() {
                if is_explicit_scope_token(first) {
                    scope = normalize_scope_token(first, &current_scope, false);
                    let _ = tokens.remove(0);
                }
            }

            let mut limit = DEFAULT_SEARCH_LIMIT;
            if let Some(last) = tokens.last().copied() {
                if let Some(parsed) = parse_limit_token(last) {
                    limit = parsed.clamp(1, MAX_SEARCH_LIMIT);
                    let _ = tokens.pop();
                }
            }

            let query = tokens.join(" ").trim().to_string();
            if query.is_empty() {
                println!("Usage: /search [#ch|dm:user] <query> [limit=N]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_search(&scope, &query, limit) {
                eprintln!("failed to request search: {}", err);
            } else {
                println!(
                    "search request sent for {} (limit={}): {}",
                    format_scope_for_help(&scope),
                    limit,
                    query
                );
            }
        }
        "/replay" => {
            let current_scope = state.lock().await.ch.clone();
            let mut tokens: Vec<&str> = parts.collect();
            if tokens.is_empty() {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                return true;
            }

            let from_ts_raw = tokens.remove(0);
            let Some(from_ts) = parse_replay_timestamp(from_ts_raw) else {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                println!("from_ts must be a non-negative unix timestamp in seconds");
                return true;
            };

            let mut scope = current_scope.clone();
            if let Some(first) = tokens.first().copied() {
                if is_explicit_scope_token(first) {
                    scope = normalize_scope_token(first, &current_scope, false);
                    let _ = tokens.remove(0);
                }
            }

            let mut limit = DEFAULT_REPLAY_LIMIT;
            if let Some(last) = tokens.last().copied() {
                if let Some(parsed) = parse_limit_token(last) {
                    limit = parsed.clamp(1, MAX_REPLAY_LIMIT);
                    let _ = tokens.pop();
                }
            }

            if !tokens.is_empty() {
                println!("Usage: /replay <from_ts> [#ch|dm:user] [limit=N]");
                return true;
            }

            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_replay(&scope, from_ts, limit) {
                eprintln!("failed to request replay: {}", err);
            } else {
                println!(
                    "replay request sent for {} from ts={} (limit={})",
                    format_scope_for_help(&scope),
                    from_ts,
                    limit
                );
            }
        }
        "/users" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                eprintln!("failed to request users: {}", err);
            }
        }
        "/metrics" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_metrics() {
                eprintln!("failed to request metrics: {}", err);
            }
        }
        "/db-profile" | "/dbprofile" => {
            let state_lock = state.lock().await;
            if let Err(err) = state_lock.send_db_profile() {
                eprintln!("failed to request db profile: {}", err);
            }
        }
        "/dm" => {
            let Some(raw_user) = parts.next() else {
                println!("Usage: /dm <user> <message>");
                return true;
            };

            let body = parts.collect::<Vec<&str>>().join(" ").trim().to_string();
            if body.is_empty() {
                println!("Usage: /dm <user> <message>");
                return true;
            }

            let mut state_lock = state.lock().await;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!("No key directory loaded yet; requested /users from server. Retry /dm after refresh.");
                return true;
            }

            let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                Ok(user) => user,
                Err(err) => {
                    println!("{}", err);
                    return true;
                }
            };

            let audit_len_before = state_lock.trust_store.audit_log.len();
            match state_lock.ensure_peer_trusted_for_dm(&canonical) {
                Ok(fingerprint) => {
                    if let Err(err) = state_lock.send_dm(&canonical, &body) {
                        eprintln!("failed to send dm: {}", err);
                    } else {
                        println!(
                            "dm sent to {} (trusted fingerprint {})",
                            canonical,
                            ClientState::format_fingerprint_for_display(&fingerprint)
                        );
                    }
                }
                Err(err) => {
                    println!("dm blocked by trust policy: {}", err);
                    if let Some(pubkey_b64) = state_lock.users.get(&canonical) {
                        if let Some(observed) = ClientState::fingerprint_for_pubkey(pubkey_b64) {
                            println!(
                                "current fingerprint for {}: {}",
                                canonical,
                                ClientState::format_fingerprint_for_display(&observed)
                            );
                        }
                    }
                }
            }

            if state_lock.trust_store.audit_log.len() != audit_len_before {
                if let Err(err) = state_lock.save_trust_store() {
                    eprintln!("failed to persist trust store: {}", err);
                }
            }
        }
        "/fingerprint" => {
            let user_arg = parts.next();
            if parts.next().is_some() {
                println!("Usage: /fingerprint [user]");
                return true;
            }

            let mut state_lock = state.lock().await;
            let mut trust_store_dirty = false;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!(
                    "No key directory loaded yet; requested /users from server. Run /fingerprint again."
                );
                return true;
            }

            if let Some(raw_user) = user_arg {
                let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                    Ok(user) => user,
                    Err(err) => {
                        println!("{}", err);
                        return true;
                    }
                };

                let Some(pubkey_b64) = state_lock.users.get(&canonical) else {
                    println!(
                        "unknown user '{}'; run /users to refresh directory",
                        canonical
                    );
                    return true;
                };

                let Some(fingerprint) = ClientState::fingerprint_for_pubkey(pubkey_b64) else {
                    println!("user '{}' has an invalid public key", canonical);
                    return true;
                };

                println!(
                    "{} fingerprint: {}",
                    canonical,
                    ClientState::format_fingerprint_for_display(&fingerprint)
                );

                let status = trust_status_label(&state_lock, &canonical, &fingerprint);
                if let Some(peer) = state_lock.trust_store.peers.get(&canonical) {
                    println!("trust status: {} (trusted_at={})", status, peer.trusted_at);
                } else {
                    println!("trust status: {}", status);
                }

                state_lock.append_trust_audit(
                    "fingerprint_view",
                    &canonical,
                    &format!("fingerprint={}", fingerprint),
                );
                trust_store_dirty = true;
            } else {
                let mut users: Vec<String> = state_lock.users.keys().cloned().collect();
                users.sort_by_key(|a| a.to_ascii_lowercase());

                println!("Known user fingerprints:");
                for user in users {
                    let Some(pubkey_b64) = state_lock.users.get(&user) else {
                        continue;
                    };

                    let Some(fingerprint) = ClientState::fingerprint_for_pubkey(pubkey_b64) else {
                        println!("  {} [invalid-key]", user);
                        continue;
                    };

                    println!(
                        "  {} [{}] {}",
                        user,
                        trust_status_label(&state_lock, &user, &fingerprint),
                        ClientState::format_fingerprint_for_display(&fingerprint)
                    );
                }
            }

            if trust_store_dirty {
                if let Err(err) = state_lock.save_trust_store() {
                    eprintln!("failed to persist trust store: {}", err);
                }
            }
        }
        "/trust" => {
            let Some(raw_user) = parts.next() else {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            };
            let Some(fingerprint) = parts.next() else {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            };
            if parts.next().is_some() {
                println!("Usage: /trust <user> <fingerprint>");
                return true;
            }

            let mut state_lock = state.lock().await;
            if state_lock.users.is_empty() {
                if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
                    eprintln!("failed to request users: {}", err);
                }
                println!(
                    "No key directory loaded yet; requested /users from server. Retry /trust after refresh."
                );
                return true;
            }

            let canonical = match resolve_known_username(&state_lock.users, raw_user) {
                Ok(user) => user,
                Err(err) => {
                    println!("{}", err);
                    return true;
                }
            };

            match state_lock.trust_peer(&canonical, fingerprint) {
                Ok(observed) => {
                    println!(
                        "trusted {} with fingerprint {}",
                        canonical,
                        ClientState::format_fingerprint_for_display(&observed)
                    );
                }
                Err(err) => {
                    println!("trust failed: {}", err);
                    if let Some(pubkey_b64) = state_lock.users.get(&canonical) {
                        if let Some(observed) = ClientState::fingerprint_for_pubkey(pubkey_b64) {
                            println!(
                                "current fingerprint for {}: {}",
                                canonical,
                                ClientState::format_fingerprint_for_display(&observed)
                            );
                        }
                    }
                }
            }

            if let Err(err) = state_lock.save_trust_store() {
                eprintln!("failed to persist trust store: {}", err);
            }
        }
        "/trust-audit" => {
            let limit = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(DEFAULT_TRUST_AUDIT_LIMIT)
                .clamp(1, MAX_TRUST_AUDIT_LIMIT);
            if parts.next().is_some() {
                println!("Usage: /trust-audit [limit]");
                return true;
            }

            let state_lock = state.lock().await;
            if state_lock.trust_store.audit_log.is_empty() {
                println!("No trust audit entries yet.");
                return true;
            }

            println!(
                "Trust audit (latest {} entries):",
                limit.min(state_lock.trust_store.audit_log.len())
            );
            for entry in state_lock.trust_store.audit_log.iter().rev().take(limit) {
                println!(
                    "  ts={} action={} peer={} details={}",
                    entry.timestamp, entry.action, entry.peer, entry.details
                );
            }
        }
        "/trust-export" => {
            let maybe_path = parts.next();
            if parts.next().is_some() {
                println!("Usage: /trust-export [path]");
                return true;
            }

            let state_lock = state.lock().await;
            let target_path = maybe_path
                .map(PathBuf::from)
                .unwrap_or_else(|| state_lock.trust_audit_export_path());

            match state_lock.export_trust_audit_to_path(&target_path) {
                Ok(entries) => {
                    println!(
                        "exported {} trust audit entries to {}",
                        entries,
                        target_path.display()
                    );
                }
                Err(err) => {
                    eprintln!("failed to export trust audit: {}", err);
                }
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

        match state_lock.load_trust_store() {
            Ok(true) => {
                println!(
                    "Loaded trust store with {} trusted peers.",
                    state_lock.trust_store.peers.len()
                );
            }
            Ok(false) => {}
            Err(err) => {
                eprintln!("failed to load trust store: {}", err);
            }
        }
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
