//! Discord ↔ Chatify Bridge Bot
//!
//! Bridges messages between Discord and a Chatify WebSocket server.
//! Messages from Discord channels are encrypted and forwarded to Chatify,
//! and messages from Chatify are relayed back to mapped Discord channels.
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
//! - `CHATIFY_RECONNECT_JITTER_PCT`: Adds jitter to reconnect delay (default: 20)
//! - `CHATIFY_RECONNECT_WARN_THRESHOLD`: Warn after N consecutive failures (default: 5)
//! - `CHATIFY_PING_SECS`: Send keepalive ping every N seconds, 0 disables (default: 20)
//! - `CHATIFY_HEALTH_LOG_SECS`: Periodic bridge health snapshot interval (default: 30)
//! - `CHATIFY_BRIDGE_INSTANCE_ID`: Stable source marker for loop prevention and tracing
//! - `CHATIFY_DISCORD_CHANNEL_MAP_FILE`: Optional JSON file for bridge routes
//! - `CHATIFY_DISCORD_CHANNEL_MAP`: Optional map `discordChannelId:chatifyChannel,...`
//! - `CHATIFY_LOG`: Set to "1" to enable logging

use clicord_server::crypto::dh_key;
use clicord_server::crypto::{
    channel_key, dec_bytes, enc_bytes, new_keypair, pub_b64, pw_hash_client,
};

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use rand::RngCore;
use serde::Deserialize;
use serenity::{
    async_trait,
    http::Http,
    model::{
        channel::Message,
        gateway::GatewayIntents,
        gateway::Ready,
        id::{ChannelId, MessageId},
    },
    prelude::*,
};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

type WsSender = mpsc::UnboundedSender<WsMessage>;
type PayloadMap = HashMap<String, serde_json::Value>;
const MAX_DISCORD_CONTENT_LEN: usize = 4000;
const MAX_DISCORD_RELAY_OUT_LEN: usize = 1900;
const MAX_RELAY_ATTACHMENTS: usize = 6;
const MAX_RELAY_ATTACHMENT_URL_LEN: usize = 512;
const MAX_RELAY_ATTACHMENT_NAME_LEN: usize = 128;
const MAX_RELAY_REPLY_EXCERPT_LEN: usize = 160;
const DEFAULT_RECONNECT_JITTER_PCT: u64 = 20;
const DEFAULT_RECONNECT_WARN_THRESHOLD: u64 = 5;
const DEFAULT_PING_SECS: u64 = 20;
const DEFAULT_CHANNEL_MAP_FILE: &str = "bridge-channel-map.json";
const RELAY_ORIGIN_DISCORD: &str = "discord";
const RELAY_MARKER_PREFIX_DISCORD: &str = "discord:";

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RelayReplyMeta {
    discord_message_id: Option<String>,
    discord_channel_id: Option<String>,
    author: Option<String>,
    excerpt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelayAttachmentMeta {
    url: String,
    filename: String,
    size_bytes: u64,
    content_type: Option<String>,
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

fn sanitize_single_line(input: &str, max_chars: usize) -> String {
    let compact = input
        .split_whitespace()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&compact, max_chars)
}

fn format_attachment_size(size_bytes: u64) -> String {
    if size_bytes < 1024 {
        return format!("{}B", size_bytes);
    }
    if size_bytes < 1024 * 1024 {
        return format!("{:.1}KB", size_bytes as f64 / 1024.0);
    }
    format!("{:.1}MB", size_bytes as f64 / (1024.0 * 1024.0))
}

fn extract_discord_attachments(msg: &Message) -> Vec<RelayAttachmentMeta> {
    msg.attachments
        .iter()
        .take(MAX_RELAY_ATTACHMENTS)
        .map(|attachment| RelayAttachmentMeta {
            url: truncate_chars(&attachment.url, MAX_RELAY_ATTACHMENT_URL_LEN),
            filename: sanitize_single_line(&attachment.filename, MAX_RELAY_ATTACHMENT_NAME_LEN),
            size_bytes: attachment.size,
            content_type: attachment
                .content_type
                .as_ref()
                .map(|t| sanitize_single_line(t, 64)),
        })
        .filter(|attachment| !attachment.url.is_empty())
        .collect()
}

fn extract_discord_reply(msg: &Message) -> Option<RelayReplyMeta> {
    let mut reply = RelayReplyMeta::default();

    if let Some(reference) = msg.message_reference.as_ref() {
        reply.discord_message_id = reference.message_id.map(|id| id.to_string());
        reply.discord_channel_id = Some(reference.channel_id.to_string());
    }

    if let Some(referenced) = msg.referenced_message.as_ref() {
        reply.discord_message_id = Some(referenced.id.to_string());
        reply.discord_channel_id = Some(referenced.channel_id.to_string());
        reply.author = Some(sanitize_single_line(&referenced.author.name, 64));
        let excerpt = sanitize_single_line(&referenced.content, MAX_RELAY_REPLY_EXCERPT_LEN);
        if !excerpt.is_empty() {
            reply.excerpt = Some(excerpt);
        }
    }

    if reply.discord_message_id.is_none()
        && reply.discord_channel_id.is_none()
        && reply.author.is_none()
        && reply.excerpt.is_none()
    {
        None
    } else {
        Some(reply)
    }
}

fn attachment_to_json(attachment: &RelayAttachmentMeta) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "url".to_string(),
        serde_json::Value::String(attachment.url.clone()),
    );
    out.insert(
        "filename".to_string(),
        serde_json::Value::String(attachment.filename.clone()),
    );
    out.insert(
        "size".to_string(),
        serde_json::Value::from(attachment.size_bytes),
    );
    if let Some(content_type) = attachment.content_type.as_ref() {
        out.insert(
            "content_type".to_string(),
            serde_json::Value::String(content_type.clone()),
        );
    }
    serde_json::Value::Object(out)
}

fn reply_to_json(reply: &RelayReplyMeta) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(message_id) = reply.discord_message_id.as_ref() {
        out.insert(
            "discord_message_id".to_string(),
            serde_json::Value::String(message_id.clone()),
        );
    }
    if let Some(channel_id) = reply.discord_channel_id.as_ref() {
        out.insert(
            "discord_channel_id".to_string(),
            serde_json::Value::String(channel_id.clone()),
        );
    }
    if let Some(author) = reply.author.as_ref() {
        out.insert(
            "author".to_string(),
            serde_json::Value::String(author.clone()),
        );
    }
    if let Some(excerpt) = reply.excerpt.as_ref() {
        out.insert(
            "excerpt".to_string(),
            serde_json::Value::String(excerpt.clone()),
        );
    }
    serde_json::Value::Object(out)
}

fn parse_attachment_from_json(raw: &serde_json::Value) -> Option<RelayAttachmentMeta> {
    let url = raw
        .get("url")
        .and_then(|v| v.as_str())
        .map(|v| truncate_chars(v, MAX_RELAY_ATTACHMENT_URL_LEN))
        .unwrap_or_default();
    if url.is_empty() {
        return None;
    }

    let filename = raw
        .get("filename")
        .and_then(|v| v.as_str())
        .map(|v| sanitize_single_line(v, MAX_RELAY_ATTACHMENT_NAME_LEN))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "attachment".to_string());

    let size_bytes = raw.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let content_type = raw
        .get("content_type")
        .and_then(|v| v.as_str())
        .map(|v| sanitize_single_line(v, 64))
        .filter(|v| !v.is_empty());

    Some(RelayAttachmentMeta {
        url,
        filename,
        size_bytes,
        content_type,
    })
}

fn relay_attachments_from_payload(data: &PayloadMap) -> Vec<RelayAttachmentMeta> {
    data.get("relay")
        .and_then(|relay| relay.get("attachments"))
        .and_then(|attachments| attachments.as_array())
        .map(|attachments| {
            attachments
                .iter()
                .filter_map(parse_attachment_from_json)
                .take(MAX_RELAY_ATTACHMENTS)
                .collect()
        })
        .unwrap_or_default()
}

fn relay_reply_from_payload(data: &PayloadMap) -> Option<RelayReplyMeta> {
    let reply = data
        .get("relay")
        .and_then(|relay| relay.get("reply"))
        .filter(|reply| reply.is_object())?;

    let out = RelayReplyMeta {
        discord_message_id: reply
            .get("discord_message_id")
            .and_then(|v| v.as_str())
            .map(|v| sanitize_single_line(v, 64))
            .filter(|v| !v.is_empty()),
        discord_channel_id: reply
            .get("discord_channel_id")
            .and_then(|v| v.as_str())
            .map(|v| sanitize_single_line(v, 64))
            .filter(|v| !v.is_empty()),
        author: reply
            .get("author")
            .and_then(|v| v.as_str())
            .map(|v| sanitize_single_line(v, 64))
            .filter(|v| !v.is_empty()),
        excerpt: reply
            .get("excerpt")
            .and_then(|v| v.as_str())
            .map(|v| sanitize_single_line(v, MAX_RELAY_REPLY_EXCERPT_LEN))
            .filter(|v| !v.is_empty()),
    };

    if out.discord_message_id.is_none()
        && out.discord_channel_id.is_none()
        && out.author.is_none()
        && out.excerpt.is_none()
    {
        None
    } else {
        Some(out)
    }
}

fn format_reply_context(reply: &RelayReplyMeta) -> Option<String> {
    let target = reply
        .author
        .as_ref()
        .map(|name| format!("to {}", name))
        .unwrap_or_else(|| "to original message".to_string());

    let id_hint = reply
        .discord_message_id
        .as_ref()
        .map(|id| format!(" id={}", sanitize_single_line(id, 16)))
        .unwrap_or_default();

    let excerpt = reply
        .excerpt
        .as_ref()
        .map(|excerpt| format!(": \"{}\"", sanitize_single_line(excerpt, 80)))
        .unwrap_or_default();

    let line = format!("[reply {}{}]{}", target, id_hint, excerpt);
    if line.trim().is_empty() {
        None
    } else {
        Some(line)
    }
}

fn format_attachment_context(attachment: &RelayAttachmentMeta) -> String {
    format!(
        "[attachment] {} ({}) {}",
        attachment.filename,
        format_attachment_size(attachment.size_bytes),
        attachment.url
    )
}

fn build_chatify_bridge_body(
    author: &str,
    content: &str,
    attachments: &[RelayAttachmentMeta],
    reply: Option<&RelayReplyMeta>,
) -> String {
    let mut lines = Vec::new();

    if let Some(reply_line) = reply.and_then(format_reply_context) {
        lines.push(reply_line);
    }

    let clean_content = content.trim();
    if !clean_content.is_empty() {
        lines.push(format!("{}: {}", author, clean_content));
    } else if !attachments.is_empty() {
        lines.push(format!(
            "{} shared {} attachment(s)",
            author,
            attachments.len()
        ));
    }

    for attachment in attachments {
        lines.push(format_attachment_context(attachment));
    }

    lines.join("\n")
}

#[derive(Debug, Deserialize)]
struct BridgeRouteConfig {
    routes: Vec<BridgeRoute>,
}

#[derive(Debug, Deserialize)]
struct BridgeRoute {
    discord_channel_id: String,
    chatify_channel: String,
}

fn parse_channel_map_file(raw: &str) -> Result<HashMap<String, String>, String> {
    let parsed: BridgeRouteConfig = serde_json::from_str(raw)
        .map_err(|e| format!("invalid JSON in channel map file: {}", e))?;
    let mut map = HashMap::new();
    for route in parsed.routes {
        let discord_channel = route.discord_channel_id.trim();
        let chatify_channel = route.chatify_channel.trim();
        if discord_channel.is_empty() {
            continue;
        }
        let Some(normalized_chatify) = normalize_chatify_channel(chatify_channel) else {
            continue;
        };
        map.insert(discord_channel.to_string(), normalized_chatify);
    }
    Ok(map)
}

fn load_channel_map_file(path: &str) -> Result<HashMap<String, String>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed to read channel map file '{}': {}", path, e))?;
    parse_channel_map_file(&raw)
}

fn merged_channel_map_from_config() -> HashMap<String, String> {
    let mut merged = HashMap::new();

    let map_file = env::var("CHATIFY_DISCORD_CHANNEL_MAP_FILE")
        .ok()
        .or_else(|| {
            if Path::new(DEFAULT_CHANNEL_MAP_FILE).exists() {
                Some(DEFAULT_CHANNEL_MAP_FILE.to_string())
            } else {
                None
            }
        });

    if let Some(path) = map_file {
        match load_channel_map_file(&path) {
            Ok(file_map) => {
                merged.extend(file_map);
            }
            Err(err) => {
                eprintln!(
                    "Failed to load bridge route map from '{}': {}. Continuing without file routes.",
                    path, err
                );
            }
        }
    }

    if let Ok(raw_map) = env::var("CHATIFY_DISCORD_CHANNEL_MAP") {
        // Environment routes override file routes for operational emergency fixes.
        merged.extend(parse_channel_map(&raw_map));
    }

    merged
}

fn relay_marker_for_discord_channel(discord_channel_id: &str) -> String {
    format!("{}{}", RELAY_MARKER_PREFIX_DISCORD, discord_channel_id)
}

fn relay_source_id(data: &PayloadMap) -> Option<&str> {
    data.get("relay")
        .and_then(|v| v.get("source_id"))
        .and_then(|v| v.as_str())
}

fn relay_markers(data: &PayloadMap) -> Vec<String> {
    data.get("relay")
        .and_then(|v| v.get("markers"))
        .and_then(|v| v.as_array())
        .map(|markers| {
            markers
                .iter()
                .filter_map(|m| m.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn should_skip_chatify_to_discord(
    data: &PayloadMap,
    own_source: &str,
    discord_channel_id: &str,
) -> bool {
    if is_self_sourced_event(data, own_source) {
        return true;
    }

    if relay_source_id(data).is_some_and(|src| src == own_source) {
        return true;
    }

    let marker = relay_marker_for_discord_channel(discord_channel_id);
    relay_markers(data)
        .iter()
        .any(|existing| existing == &marker)
}

fn format_discord_relay_text(
    sender: &str,
    content: &str,
    attachments: &[RelayAttachmentMeta],
    reply: Option<&RelayReplyMeta>,
) -> String {
    let mut body_lines = Vec::new();
    let has_context_in_content = content.contains("[attachment]") || content.contains("[reply ");

    if !has_context_in_content {
        if let Some(reply_line) = reply.and_then(format_reply_context) {
            body_lines.push(reply_line);
        }
    }

    if !content.trim().is_empty() {
        body_lines.push(content.trim().to_string());
    }

    if !has_context_in_content {
        for attachment in attachments {
            body_lines.push(format_attachment_context(attachment));
        }
    }

    if body_lines.is_empty() {
        body_lines.push("[empty message]".to_string());
    }

    let mut out = format!("{}: {}", sender, body_lines.join("\n"));
    if out.len() <= MAX_DISCORD_RELAY_OUT_LEN {
        return out;
    }

    out.truncate(MAX_DISCORD_RELAY_OUT_LEN.saturating_sub(3));
    out.push_str("...");
    out
}

async fn send_to_discord_channel(
    http: &Arc<Http>,
    channel_id_num: u64,
    content: &str,
    reply: Option<&RelayReplyMeta>,
) -> Result<(), serenity::Error> {
    let channel_id = ChannelId(channel_id_num);

    let maybe_reply_message_id = reply
        .and_then(|r| {
            r.discord_channel_id
                .as_ref()
                .map(|channel_id_str| channel_id_str == &channel_id_num.to_string())
                .unwrap_or(true)
                .then_some(r.discord_message_id.as_deref())
                .flatten()
        })
        .and_then(|id| id.parse::<u64>().ok());

    if let Some(reply_message_id) = maybe_reply_message_id {
        channel_id
            .send_message(http.as_ref(), |message| {
                message
                    .content(content)
                    .reference_message((channel_id, MessageId(reply_message_id)))
                    .allowed_mentions(|mentions| mentions.empty_parse())
            })
            .await?;
        return Ok(());
    }

    channel_id.say(http.as_ref(), content).await?;
    Ok(())
}

fn format_bridge_status_line(
    snapshot: BridgeMetricsSnapshot,
    ws_connected: bool,
    route_count: usize,
    source_id: &str,
) -> String {
    format!(
        "bridge status | ws_connected={} source_id={} routes={} discord_in={} chatify_in={} discord_out={} chatify_out={} dropped={} reconnects={} attempts={} auth_failures={}",
        ws_connected,
        source_id,
        route_count,
        snapshot.discord_ingress,
        snapshot.chatify_ingress,
        snapshot.discord_forwarded,
        snapshot.chatify_forwarded,
        snapshot.dropped_messages,
        snapshot.reconnects,
        snapshot.connect_attempts,
        snapshot.auth_failures,
    )
}

fn is_self_sourced_event(data: &PayloadMap, own_source: &str) -> bool {
    let source = data.get("src").and_then(|v| v.as_str()).unwrap_or("");
    !source.is_empty() && source == own_source
}

struct BridgeMetrics {
    discord_ingress: AtomicU64,
    chatify_ingress: AtomicU64,
    discord_forwarded: AtomicU64,
    chatify_forwarded: AtomicU64,
    dropped_messages: AtomicU64,
    reconnects: AtomicU64,
    connect_attempts: AtomicU64,
    auth_failures: AtomicU64,
    ws_read_errors: AtomicU64,
    ws_write_errors: AtomicU64,
    pings_sent: AtomicU64,
    pongs_received: AtomicU64,
}

#[derive(Clone, Copy)]
struct BridgeMetricsSnapshot {
    discord_ingress: u64,
    chatify_ingress: u64,
    discord_forwarded: u64,
    chatify_forwarded: u64,
    dropped_messages: u64,
    reconnects: u64,
    connect_attempts: u64,
    auth_failures: u64,
    ws_read_errors: u64,
    ws_write_errors: u64,
    pings_sent: u64,
    pongs_received: u64,
}

impl BridgeMetrics {
    fn new() -> Self {
        Self {
            discord_ingress: AtomicU64::new(0),
            chatify_ingress: AtomicU64::new(0),
            discord_forwarded: AtomicU64::new(0),
            chatify_forwarded: AtomicU64::new(0),
            dropped_messages: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            connect_attempts: AtomicU64::new(0),
            auth_failures: AtomicU64::new(0),
            ws_read_errors: AtomicU64::new(0),
            ws_write_errors: AtomicU64::new(0),
            pings_sent: AtomicU64::new(0),
            pongs_received: AtomicU64::new(0),
        }
    }

    fn inc_discord_ingress(&self) {
        self.discord_ingress.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_chatify_ingress(&self) {
        self.chatify_ingress.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_discord_forwarded(&self) {
        self.discord_forwarded.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_chatify_forwarded(&self) {
        self.chatify_forwarded.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_dropped(&self) {
        self.dropped_messages.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_reconnects(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_connect_attempts(&self) {
        self.connect_attempts.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_auth_failures(&self) {
        self.auth_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_ws_read_errors(&self) {
        self.ws_read_errors.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_ws_write_errors(&self) {
        self.ws_write_errors.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_pings_sent(&self) {
        self.pings_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_pongs_received(&self) {
        self.pongs_received.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> BridgeMetricsSnapshot {
        BridgeMetricsSnapshot {
            discord_ingress: self.discord_ingress.load(Ordering::Relaxed),
            chatify_ingress: self.chatify_ingress.load(Ordering::Relaxed),
            discord_forwarded: self.discord_forwarded.load(Ordering::Relaxed),
            chatify_forwarded: self.chatify_forwarded.load(Ordering::Relaxed),
            dropped_messages: self.dropped_messages.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            connect_attempts: self.connect_attempts.load(Ordering::Relaxed),
            auth_failures: self.auth_failures.load(Ordering::Relaxed),
            ws_read_errors: self.ws_read_errors.load(Ordering::Relaxed),
            ws_write_errors: self.ws_write_errors.load(Ordering::Relaxed),
            pings_sent: self.pings_sent.load(Ordering::Relaxed),
            pongs_received: self.pongs_received.load(Ordering::Relaxed),
        }
    }

    fn log_snapshot_delta(
        &self,
        current: BridgeMetricsSnapshot,
        previous: Option<BridgeMetricsSnapshot>,
        interval_secs: u64,
    ) {
        let zero = BridgeMetricsSnapshot {
            discord_ingress: 0,
            chatify_ingress: 0,
            discord_forwarded: 0,
            chatify_forwarded: 0,
            dropped_messages: 0,
            reconnects: 0,
            connect_attempts: 0,
            auth_failures: 0,
            ws_read_errors: 0,
            ws_write_errors: 0,
            pings_sent: 0,
            pongs_received: 0,
        };
        let prev = previous.unwrap_or(zero);

        let d_discord_in = current.discord_ingress.saturating_sub(prev.discord_ingress);
        let d_chatify_in = current.chatify_ingress.saturating_sub(prev.chatify_ingress);
        let d_discord_out = current
            .discord_forwarded
            .saturating_sub(prev.discord_forwarded);
        let d_chatify_out = current
            .chatify_forwarded
            .saturating_sub(prev.chatify_forwarded);
        let d_drop = current
            .dropped_messages
            .saturating_sub(prev.dropped_messages);
        let d_reconnects = current.reconnects.saturating_sub(prev.reconnects);
        let d_attempts = current
            .connect_attempts
            .saturating_sub(prev.connect_attempts);
        let d_auth_failures = current.auth_failures.saturating_sub(prev.auth_failures);
        let d_ws_read = current.ws_read_errors.saturating_sub(prev.ws_read_errors);
        let d_ws_write = current.ws_write_errors.saturating_sub(prev.ws_write_errors);
        let d_pings = current.pings_sent.saturating_sub(prev.pings_sent);
        let d_pongs = current.pongs_received.saturating_sub(prev.pongs_received);

        info!(
            "event=bridge_health interval_s={} discord_in_total={} chatify_in_total={} discord_out_total={} chatify_out_total={} dropped_total={} reconnects_total={} connect_attempts_total={} auth_failures_total={} ws_read_errors_total={} ws_write_errors_total={} pings_total={} pongs_total={} discord_in_delta={} chatify_in_delta={} discord_out_delta={} chatify_out_delta={} dropped_delta={} reconnects_delta={} attempts_delta={} auth_failures_delta={} ws_read_delta={} ws_write_delta={} pings_delta={} pongs_delta={}",
            interval_secs,
            current.discord_ingress,
            current.chatify_ingress,
            current.discord_forwarded,
            current.chatify_forwarded,
            current.dropped_messages,
            current.reconnects,
            current.connect_attempts,
            current.auth_failures,
            current.ws_read_errors,
            current.ws_write_errors,
            current.pings_sent,
            current.pongs_received,
            d_discord_in,
            d_chatify_in,
            d_discord_out,
            d_chatify_out,
            d_drop,
            d_reconnects,
            d_attempts,
            d_auth_failures,
            d_ws_read,
            d_ws_write,
            d_pings,
            d_pongs,
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
    reconnect_jitter_pct: u64,
    reconnect_warn_threshold: u64,
    ping_secs: u64,
    health_log_secs: u64,
    instance_id: String,
    channel_map: HashMap<String, String>,
}

impl BridgeConfig {
    fn from_env() -> Self {
        let ws_scheme_raw = env::var("CHATIFY_WS_SCHEME").unwrap_or_else(|_| "ws".to_string());
        let chatify_ws_scheme = match ws_scheme_raw.to_ascii_lowercase().as_str() {
            "ws" | "wss" => ws_scheme_raw.to_ascii_lowercase(),
            invalid => {
                eprintln!(
                    "Invalid CHATIFY_WS_SCHEME='{}'. Falling back to 'ws'.",
                    invalid
                );
                "ws".to_string()
            }
        };
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
        let reconnect_jitter_pct = env::var("CHATIFY_RECONNECT_JITTER_PCT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RECONNECT_JITTER_PCT)
            .min(100);
        let reconnect_warn_threshold = env::var("CHATIFY_RECONNECT_WARN_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RECONNECT_WARN_THRESHOLD)
            .max(1);
        let ping_secs = env::var("CHATIFY_PING_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PING_SECS);
        let health_log_secs = env::var("CHATIFY_HEALTH_LOG_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30)
            .max(5);
        let mut instance_id_bytes = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut instance_id_bytes);
        let instance_id = env::var("CHATIFY_BRIDGE_INSTANCE_ID")
            .unwrap_or_else(|_| hex::encode(instance_id_bytes));
        let channel_map = merged_channel_map_from_config();

        let raw_channel = env::var("CHATIFY_CHANNEL").unwrap_or_else(|_| "general".to_string());
        let chatify_channel = normalize_chatify_channel(&raw_channel).unwrap_or_else(|| {
            eprintln!(
                "Invalid CHATIFY_CHANNEL='{}'. Falling back to 'general'.",
                raw_channel
            );
            "general".to_string()
        });

        Self {
            discord_token: env::var("DISCORD_TOKEN")
                .expect("Expected DISCORD_TOKEN in environment"),
            chatify_host: env::var("CHATIFY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            chatify_port: env::var("CHATIFY_PORT").unwrap_or_else(|_| "8765".to_string()),
            chatify_password: env::var("CHATIFY_PASSWORD")
                .expect("Expected CHATIFY_PASSWORD in environment"),
            chatify_channel,
            chatify_bot_username: env::var("CHATIFY_BOT_USERNAME")
                .unwrap_or_else(|_| "DiscordBot".to_string()),
            chatify_ws_scheme,
            auth_timeout_secs,
            reconnect_base_secs,
            reconnect_max_secs,
            reconnect_jitter_pct,
            reconnect_warn_threshold,
            ping_secs,
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

fn jittered_backoff_secs(base_secs: u64, jitter_pct: u64) -> u64 {
    if base_secs == 0 || jitter_pct == 0 {
        return base_secs;
    }

    let max_extra = base_secs.saturating_mul(jitter_pct).saturating_div(100);
    if max_extra == 0 {
        return base_secs;
    }

    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let noise = u64::from_le_bytes(bytes) % (max_extra + 1);
    base_secs.saturating_add(noise)
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
    /// Discord HTTP client used for Chatify -> Discord relay.
    discord_http: Option<Arc<Http>>,
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
            discord_http: None,
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

    /// Resolve Discord target channels for a Chatify channel.
    fn discord_targets_for_chatify_channel(&self, ch: &str) -> Vec<String> {
        self.channel_map
            .iter()
            .filter_map(|(discord_channel_id, chatify_channel)| {
                if chatify_channel == ch {
                    Some(discord_channel_id.clone())
                } else {
                    None
                }
            })
            .collect()
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

        if content.eq_ignore_ascii_case("/bridge status") {
            let (ws_connected, route_count, source_id) = {
                let state = self.state.lock().await;
                (
                    state.ws_tx.is_some(),
                    state.channel_map.len(),
                    state.bridge_src_tag.clone(),
                )
            };
            let status = format_bridge_status_line(
                self.metrics.snapshot(),
                ws_connected,
                route_count,
                &source_id,
            );

            match msg.channel_id.say(&ctx.http, &status).await {
                Ok(_) => {
                    info!(
                        "event=bridge_status_command requester={} channel_id={} ws_connected={} route_count={} source_id={}",
                        msg.author.name,
                        msg.channel_id,
                        ws_connected,
                        route_count,
                        source_id
                    );
                }
                Err(err) => {
                    warn!(
                        "event=bridge_status_command_failed requester={} channel_id={} error={}",
                        msg.author.name, msg.channel_id, err
                    );
                    self.metrics.inc_dropped();
                }
            }
            return;
        }

        let attachments = extract_discord_attachments(&msg);
        let reply_meta = extract_discord_reply(&msg);

        if content.len() > MAX_DISCORD_CONTENT_LEN {
            self.metrics.inc_dropped();
            debug!(
                "event=discord_drop reason=too_large content_len={} channel_id={}",
                content.len(),
                msg.channel_id
            );
            return;
        }
        if content.is_empty() && attachments.is_empty() {
            self.metrics.inc_dropped();
            debug!(
                "event=discord_drop reason=empty_no_attachments channel_id={}",
                msg.channel_id
            );
            return;
        }
        self.metrics.inc_discord_ingress();

        let author = msg.author.name.clone();
        let bridge_body =
            build_chatify_bridge_body(&author, &content, &attachments, reply_meta.as_ref());

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

        let encrypted = match enc_bytes(&key, bridge_body.as_bytes()) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "event=discord_encrypt_failed channel={} author={} error={}",
                    channel, author, e
                );
                self.metrics.inc_dropped();
                return;
            }
        };
        let encoded = general_purpose::STANDARD.encode(&encrypted);
        let relay_marker = relay_marker_for_discord_channel(&discord_channel_id);
        let mut relay = serde_json::Map::new();
        relay.insert("source_id".to_string(), serde_json::json!(src_tag.clone()));
        relay.insert(
            "origin".to_string(),
            serde_json::json!(RELAY_ORIGIN_DISCORD),
        );
        relay.insert(
            "markers".to_string(),
            serde_json::json!([relay_marker.clone()]),
        );
        if !attachments.is_empty() {
            relay.insert(
                "attachments".to_string(),
                serde_json::Value::Array(
                    attachments
                        .iter()
                        .map(attachment_to_json)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        if let Some(reply) = reply_meta.as_ref() {
            relay.insert("reply".to_string(), reply_to_json(reply));
        }

        let chatify_msg = serde_json::json!({
            "t": "msg",
            "ch": channel,
            "c": encoded,
            "p": bridge_body,
            "ts": now_secs(),
            "n": fresh_nonce_hex(),
            "src": src_tag.clone(),
            "relay": serde_json::Value::Object(relay)
        });
        if let Some(tx) = ws_tx {
            send_ws_json(&tx, chatify_msg);
            self.metrics.inc_discord_forwarded();
            info!(
                "event=discord_relayed_to_chatify discord_channel_id={} chatify_channel={} author={} marker={} attachments_count={} reply_present={}",
                discord_channel_id,
                channel,
                author,
                relay_marker,
                attachments.len(),
                reply_meta.is_some()
            );
        } else {
            warn!(
                "event=discord_drop reason=chatify_disconnected channel_id={}",
                discord_channel_id
            );
            self.metrics.inc_dropped();
        }
    }

    /// Called when the bot is ready
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("event=discord_ready bot_user={}", ready.user.name);
        let mut state = self.state.lock().await;
        state.username = ready.user.name.to_string();
        state.discord_http = Some(ctx.http.clone());
    }
}

/// Handle Chatify channel messages
async fn handle_chatify_msg(
    data: &PayloadMap,
    state: &Arc<Mutex<BotState>>,
    metrics: &Arc<BridgeMetrics>,
) {
    metrics.inc_chatify_ingress();
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");

    let (own_source, key, discord_targets, discord_http) = {
        let bot_state = state.lock().await;
        (
            bot_state.bridge_src_tag.clone(),
            bot_state.get_channel_key(ch),
            bot_state.discord_targets_for_chatify_channel(ch),
            bot_state.discord_http.clone(),
        )
    };

    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => {
            metrics.inc_dropped();
            debug!("event=chatify_drop reason=invalid_base64 type=msg");
            return;
        }
    };

    if let Ok(content) = dec_bytes(&key, &encrypted) {
        let content_str =
            String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
        let relay_attachments = relay_attachments_from_payload(data);
        let relay_reply = relay_reply_from_payload(data);

        if discord_targets.is_empty() {
            debug!(
                "event=chatify_drop reason=no_discord_route channel={} sender={}",
                ch, u
            );
            return;
        }

        let Some(discord_http) = discord_http else {
            warn!(
                "event=chatify_drop reason=discord_not_ready channel={} sender={}",
                ch, u
            );
            metrics.inc_dropped();
            return;
        };

        let relay_text =
            format_discord_relay_text(u, &content_str, &relay_attachments, relay_reply.as_ref());
        for discord_channel_id in discord_targets {
            if should_skip_chatify_to_discord(data, &own_source, &discord_channel_id) {
                debug!(
                    "event=chatify_loop_prevented chatify_channel={} discord_channel_id={} source_id={} relay_source={} relay_markers={}",
                    ch,
                    discord_channel_id,
                    own_source,
                    relay_source_id(data).unwrap_or(""),
                    relay_markers(data).join("|")
                );
                continue;
            }

            let Ok(channel_id_num) = discord_channel_id.parse::<u64>() else {
                warn!(
                    "event=chatify_drop reason=invalid_discord_channel_id channel={} discord_channel_id={} sender={}",
                    ch,
                    discord_channel_id,
                    u
                );
                metrics.inc_dropped();
                continue;
            };

            match send_to_discord_channel(
                &discord_http,
                channel_id_num,
                &relay_text,
                relay_reply.as_ref(),
            )
            .await
            {
                Ok(_) => {
                    metrics.inc_chatify_forwarded();
                    info!(
                        "event=chatify_relayed_to_discord chatify_channel={} discord_channel_id={} sender={} relay_source={} relay_markers={} attachments_count={} reply_present={}",
                        ch,
                        discord_channel_id,
                        u,
                        relay_source_id(data).unwrap_or(""),
                        relay_markers(data).join("|"),
                        relay_attachments.len(),
                        relay_reply.is_some()
                    );
                }
                Err(err) => {
                    warn!(
                        "event=chatify_drop reason=discord_send_failed channel={} discord_channel_id={} sender={} error={}",
                        ch,
                        discord_channel_id,
                        u,
                        err
                    );
                    metrics.inc_dropped();
                }
            }
        }
    } else {
        metrics.inc_dropped();
        debug!(
            "event=chatify_drop reason=decrypt_failed type=msg channel={}",
            ch
        );
    }
}

/// Handle Chatify system messages
fn handle_system_msg(data: &PayloadMap, metrics: &Arc<BridgeMetrics>) {
    metrics.inc_chatify_ingress();
    let m = data.get("m").and_then(|v| v.as_str()).unwrap_or("");
    metrics.inc_chatify_forwarded();
    info!("event=chatify_system message={}", m);
}

/// Handle Chatify direct messages between users
async fn handle_dm_msg(
    data: &PayloadMap,
    state: &Arc<Mutex<BotState>>,
    metrics: &Arc<BridgeMetrics>,
) {
    metrics.inc_chatify_ingress();
    let frm = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");

    let encrypted = match general_purpose::STANDARD.decode(c) {
        Ok(bytes) => bytes,
        Err(_) => {
            metrics.inc_dropped();
            debug!("event=chatify_drop reason=invalid_base64 type=dm");
            return;
        }
    };

    let dm_key = {
        let bot_state = state.lock().await;
        let peer = if frm == bot_state.username { to } else { frm };
        bot_state.get_dm_key(peer)
    };

    let Ok(dm_key) = dm_key else {
        metrics.inc_dropped();
        debug!(
            "event=chatify_drop reason=dm_key_unavailable from={} to={}",
            frm, to
        );
        return;
    };

    if let Ok(content) = dec_bytes(&dm_key, &encrypted) {
        let content_str =
            String::from_utf8(content).unwrap_or_else(|_| "[Invalid UTF-8]".to_string());
        metrics.inc_chatify_forwarded();
        info!(
            "event=chatify_dm from={} to={} content_len={}",
            frm,
            to,
            content_str.len()
        );
    } else {
        metrics.inc_dropped();
        debug!(
            "event=chatify_drop reason=decrypt_failed type=dm from={} to={}",
            frm, to
        );
    }
}

async fn handle_users_update(data: &PayloadMap, state: &Arc<Mutex<BotState>>) {
    let users_value = data.get("users").cloned().unwrap_or(serde_json::json!([]));
    let bot_state = state.lock().await;
    load_users(&users_value, &bot_state.users);
    debug!("event=chatify_users_update");
}

fn handle_pong(metrics: &Arc<BridgeMetrics>) {
    metrics.inc_pongs_received();
    debug!("event=chatify_pong");
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
        "pong" => handle_pong(metrics),
        _ => {
            debug!("event=chatify_unhandled_frame type={}", t);
        }
    }
}

async fn run_chatify_session(
    state: Arc<Mutex<BotState>>,
    cfg: &BridgeConfig,
    metrics: Arc<BridgeMetrics>,
) -> Result<(), String> {
    let uri = cfg.uri();
    metrics.inc_connect_attempts();
    info!(
        "event=chatify_connect_attempt uri={} auth_timeout_s={}",
        uri, cfg.auth_timeout_secs
    );
    let (ws_stream, _) = connect_async(&uri)
        .await
        .map_err(|e| format!("Failed to connect to Chatify: {e}"))?;
    info!("event=chatify_connected uri={}", uri);

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<WsMessage>();
    let writer_metrics = metrics.clone();
    tokio::spawn(async move {
        while let Some(msg) = bridge_rx.recv().await {
            if let Err(err) = ws_write.send(msg).await {
                writer_metrics.inc_ws_write_errors();
                warn!("event=chatify_writer_error error={}", err);
                break;
            }
        }
    });

    // Authenticate with Chatify.
    {
        let bot_state = state.lock().await;
        let pw_hash = pw_hash_client(&bot_state.password).map_err(|e| {
            metrics.inc_auth_failures();
            format!("Failed to hash bridge password for auth: {}", e)
        })?;
        let pk = pub_b64(&bot_state.priv_key).map_err(|e| {
            metrics.inc_auth_failures();
            format!("Failed to derive bridge public key: {}", e)
        })?;
        let auth_msg = serde_json::json!({
            "t": "auth",
            "u": bot_state.username,
            "pw": pw_hash,
            "pk": pk,
            "status": {"text": "Online", "emoji": "🟢"}
        });
        send_ws_json(&bridge_tx, auth_msg);
    }

    // Wait for auth response with timeout.
    let auth_reply = timeout(Duration::from_secs(cfg.auth_timeout_secs), ws_read.next())
        .await
        .map_err(|_| {
            metrics.inc_auth_failures();
            "Auth response timeout from Chatify".to_string()
        })?;

    let auth_text = match auth_reply {
        Some(Ok(WsMessage::Text(resp))) => resp,
        Some(Ok(other)) => {
            metrics.inc_auth_failures();
            return Err(format!("Unexpected auth frame from Chatify: {:?}", other));
        }
        Some(Err(e)) => {
            metrics.inc_auth_failures();
            return Err(format!("WebSocket error during auth: {e}"));
        }
        None => {
            metrics.inc_auth_failures();
            return Err("Chatify closed connection before auth completed".to_string());
        }
    };

    let resp_val: serde_json::Value = serde_json::from_str(&auth_text).map_err(|e| {
        metrics.inc_auth_failures();
        format!("Invalid auth JSON: {e}")
    })?;
    let typ = resp_val.get("t").and_then(|v| v.as_str()).unwrap_or("");
    if typ == "err" {
        metrics.inc_auth_failures();
        return Err(format!(
            "Authentication failed: {}",
            resp_val["m"].as_str().unwrap_or("unknown error")
        ));
    }
    if typ != "ok" {
        metrics.inc_auth_failures();
        return Err(format!("Unexpected auth response type: {}", typ));
    }
    if !resp_val.get("users").map(|v| v.is_array()).unwrap_or(false) {
        metrics.inc_auth_failures();
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
    info!(
        "event=chatify_authenticated username={}",
        resp_val["u"].as_str().unwrap_or("unknown")
    );

    let mut ping_interval = if cfg.ping_secs > 0 {
        let mut interval = tokio::time::interval(Duration::from_secs(cfg.ping_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Some(interval)
    } else {
        None
    };

    loop {
        let frame = if let Some(interval) = ping_interval.as_mut() {
            tokio::select! {
                _ = interval.tick() => {
                    send_ws_json(&bridge_tx, serde_json::json!({"t": "ping", "ts": now_secs()}));
                    metrics.inc_pings_sent();
                    debug!("event=chatify_ping");
                    continue;
                }
                next = ws_read.next() => next,
            }
        } else {
            ws_read.next().await
        };

        let Some(frame) = frame else {
            return Err("Chatify websocket stream ended".to_string());
        };

        match frame {
            Ok(WsMessage::Text(text)) => {
                if let Ok(data) = serde_json::from_str::<PayloadMap>(&text) {
                    dispatch_chatify_event(&data, &state, &metrics).await;
                } else {
                    warn!("event=chatify_drop reason=non_json_payload");
                    metrics.inc_dropped();
                }
            }
            Ok(WsMessage::Close(close_frame)) => {
                info!("event=chatify_closed frame={:?}", close_frame);
                return Err("Chatify connection closed".to_string());
            }
            Ok(WsMessage::Ping(payload)) => {
                let _ = bridge_tx.send(WsMessage::Pong(payload));
            }
            Ok(_) => {}
            Err(e) => {
                metrics.inc_ws_read_errors();
                return Err(format!("Chatify websocket read error: {e}"));
            }
        }
    }
}

async fn bridge_supervisor(
    state: Arc<Mutex<BotState>>,
    cfg: BridgeConfig,
    metrics: Arc<BridgeMetrics>,
) {
    let mut backoff = cfg.reconnect_base_secs;
    let mut consecutive_failures = 0u64;
    loop {
        match run_chatify_session(state.clone(), &cfg, metrics.clone()).await {
            Ok(()) => {
                backoff = cfg.reconnect_base_secs;
                consecutive_failures = 0;
            }
            Err(err) => {
                metrics.inc_reconnects();
                consecutive_failures = consecutive_failures.saturating_add(1);
                {
                    let mut bot_state = state.lock().await;
                    bot_state.ws_tx = None;
                }
                let sleep_secs = jittered_backoff_secs(backoff, cfg.reconnect_jitter_pct);
                if consecutive_failures >= cfg.reconnect_warn_threshold {
                    warn!(
                        "event=bridge_reconnect_slow consecutive_failures={} reason={} backoff_base_s={} backoff_sleep_s={}",
                        consecutive_failures,
                        err,
                        backoff,
                        sleep_secs
                    );
                } else {
                    info!(
                        "event=bridge_reconnect reason={} consecutive_failures={} backoff_base_s={} backoff_sleep_s={}",
                        err,
                        consecutive_failures,
                        backoff,
                        sleep_secs
                    );
                }
                sleep(Duration::from_secs(sleep_secs)).await;
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
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    info!(
        "event=bridge_start host={} port={} channel={} ws_scheme={} reconnect_base_s={} reconnect_max_s={} reconnect_jitter_pct={} ping_s={} health_s={} channel_map_entries={}",
        cfg.chatify_host,
        cfg.chatify_port,
        cfg.chatify_channel,
        cfg.chatify_ws_scheme,
        cfg.reconnect_base_secs,
        cfg.reconnect_max_secs,
        cfg.reconnect_jitter_pct,
        cfg.ping_secs,
        cfg.health_log_secs,
        cfg.channel_map.len()
    );

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
            let mut previous: Option<BridgeMetricsSnapshot> = None;
            loop {
                sleep(Duration::from_secs(interval_secs)).await;
                let current = metrics_clone.snapshot();
                metrics_clone.log_snapshot_delta(current, previous, interval_secs);
                previous = Some(current);
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
            error!("event=discord_client_error details={:?}", why);
        }
    });

    tokio::select! {
        _ = bridge_task => {
            error!("event=bridge_supervisor_ended_unexpectedly");
        }
        _ = discord_task => {
            error!("event=discord_task_ended_unexpectedly");
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
            reconnect_jitter_pct: 0,
            reconnect_warn_threshold: 3,
            ping_secs: 0,
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
    fn parse_channel_map_file_filters_and_normalizes_entries() {
        let raw = r##"
        {
            "routes": [
                {"discord_channel_id": "111", "chatify_channel": "General"},
                {"discord_channel_id": "222", "chatify_channel": "#Ops"},
                {"discord_channel_id": "", "chatify_channel": "ignore"},
                {"discord_channel_id": "333", "chatify_channel": "***"}
            ]
        }
        "##;

        let parsed = parse_channel_map_file(raw).expect("parse route file");
        assert_eq!(parsed.get("111"), Some(&"general".to_string()));
        assert_eq!(parsed.get("222"), Some(&"ops".to_string()));
        assert!(!parsed.contains_key("333"));
        assert_eq!(parsed.len(), 2);
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

    #[test]
    fn loop_prevention_skips_when_discord_marker_is_present() {
        let own = "discord-bridge:test-instance";
        let mut payload = PayloadMap::new();
        payload.insert("src".to_string(), serde_json::json!("discord-bridge:other"));
        payload.insert(
            "relay".to_string(),
            serde_json::json!({
                "source_id": "discord-bridge:other",
                "origin": "discord",
                "markers": ["discord:111", "chatify:general"]
            }),
        );

        assert!(should_skip_chatify_to_discord(&payload, own, "111"));
        assert!(!should_skip_chatify_to_discord(&payload, own, "222"));
    }

    #[test]
    fn loop_prevention_skips_when_relay_source_matches_instance() {
        let own = "discord-bridge:test-instance";
        let mut payload = PayloadMap::new();
        payload.insert(
            "relay".to_string(),
            serde_json::json!({
                "source_id": own,
                "origin": "discord",
                "markers": ["discord:333"]
            }),
        );

        assert!(should_skip_chatify_to_discord(&payload, own, "444"));
    }

    #[test]
    fn bridge_status_line_contains_core_metrics() {
        let snapshot = BridgeMetricsSnapshot {
            discord_ingress: 1,
            chatify_ingress: 2,
            discord_forwarded: 3,
            chatify_forwarded: 4,
            dropped_messages: 5,
            reconnects: 6,
            connect_attempts: 7,
            auth_failures: 8,
            ws_read_errors: 9,
            ws_write_errors: 10,
            pings_sent: 11,
            pongs_received: 12,
        };

        let line = format_bridge_status_line(snapshot, true, 2, "discord-bridge:test");
        assert!(line.contains("ws_connected=true"));
        assert!(line.contains("routes=2"));
        assert!(line.contains("discord_in=1"));
        assert!(line.contains("chatify_out=4"));
    }

    #[test]
    fn build_chatify_bridge_body_includes_reply_and_attachments() {
        let attachments = vec![RelayAttachmentMeta {
            url: "https://cdn.example.com/a.png".to_string(),
            filename: "a.png".to_string(),
            size_bytes: 2048,
            content_type: Some("image/png".to_string()),
        }];
        let reply = RelayReplyMeta {
            discord_message_id: Some("9001".to_string()),
            discord_channel_id: Some("123".to_string()),
            author: Some("bob".to_string()),
            excerpt: Some("earlier context".to_string()),
        };

        let body = build_chatify_bridge_body("alice", "new message", &attachments, Some(&reply));
        assert!(body.contains("[reply to bob"));
        assert!(body.contains("alice: new message"));
        assert!(body.contains("[attachment] a.png"));
    }

    #[test]
    fn relay_payload_extracts_attachment_and_reply_metadata() {
        let mut payload = PayloadMap::new();
        payload.insert(
            "relay".to_string(),
            serde_json::json!({
                "attachments": [
                    {
                        "url": "https://cdn.example.com/doc.pdf",
                        "filename": "doc.pdf",
                        "size": 8192,
                        "content_type": "application/pdf"
                    }
                ],
                "reply": {
                    "discord_message_id": "777",
                    "discord_channel_id": "111",
                    "author": "carol",
                    "excerpt": "quoted"
                }
            }),
        );

        let attachments = relay_attachments_from_payload(&payload);
        let reply = relay_reply_from_payload(&payload).expect("reply metadata");

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "doc.pdf");
        assert_eq!(attachments[0].size_bytes, 8192);
        assert_eq!(reply.discord_message_id.as_deref(), Some("777"));
        assert_eq!(reply.author.as_deref(), Some("carol"));
    }

    #[test]
    fn discord_relay_text_appends_context_when_not_in_body() {
        let attachments = vec![RelayAttachmentMeta {
            url: "https://cdn.example.com/file.txt".to_string(),
            filename: "file.txt".to_string(),
            size_bytes: 120,
            content_type: Some("text/plain".to_string()),
        }];
        let reply = RelayReplyMeta {
            discord_message_id: Some("42".to_string()),
            discord_channel_id: Some("1".to_string()),
            author: Some("dave".to_string()),
            excerpt: Some("hello".to_string()),
        };

        let text = format_discord_relay_text("alice", "payload", &attachments, Some(&reply));
        assert!(text.contains("alice:"));
        assert!(text.contains("[reply to dave"));
        assert!(text.contains("[attachment] file.txt"));
    }

    #[test]
    fn jittered_backoff_without_jitter_is_stable() {
        assert_eq!(jittered_backoff_secs(10, 0), 10);
        assert_eq!(jittered_backoff_secs(0, 20), 0);
    }

    #[test]
    fn jittered_backoff_with_jitter_stays_within_bounds() {
        let base = 20;
        let jitter_pct = 25;
        let upper_bound = base + (base * jitter_pct / 100);

        for _ in 0..128 {
            let value = jittered_backoff_secs(base, jitter_pct);
            assert!(value >= base, "backoff should never be less than base");
            assert!(
                value <= upper_bound,
                "backoff should be bounded by jitter window"
            );
        }
    }
}
