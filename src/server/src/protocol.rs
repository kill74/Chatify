//! Protocol constants and types for Chatify server.
//!
//! This module contains all protocol-related constants, message types,
//! and utility functions that define the wire protocol between clients
//! and server.

use serde_json::Value;

// ============================================================================
// Core Protocol Constants
// ============================================================================

/// Maximum number of messages kept in the per-channel in-memory ring buffer.
pub const HISTORY_CAP: usize = 50;

/// Maximum byte length for any post-auth WebSocket frame.
pub const MAX_BYTES: usize = 16_000;

/// Maximum byte length for the initial auth frame.
pub const MAX_AUTH_BYTES: usize = 4_096;

/// Maximum HTTP header size during WebSocket handshake (CVE-2023-43668 mitigation).
pub const MAX_HANDSHAKE_HEADER_SIZE: usize = 8192;

/// Maximum number of HTTP headers during WebSocket handshake.
pub const MAX_HANDSHAKE_HEADERS: usize = 64;

/// SQLite schema version this binary was built against.
pub const CURRENT_SCHEMA_VERSION: i64 = 7;

/// Default number of events returned by a `history` request.
pub const DEFAULT_HISTORY_LIMIT: usize = 50;

/// Default number of events returned by a `search` request.
pub const DEFAULT_SEARCH_LIMIT: usize = 30;

/// Default look-back window in seconds for a `rewind` request (1 hour).
pub const DEFAULT_REWIND_SECONDS: u64 = 3600;

/// Default number of events returned by a `rewind` request.
pub const DEFAULT_REWIND_LIMIT: usize = 100;

/// DM channel name prefix.
pub const DM_CHANNEL_PREFIX: &str = "__dm__";

/// WebSocket sub-protocol version advertised in the `"ok"` auth response.
pub const PROTOCOL_VERSION: u64 = 1;

/// Maximum allowed username length in characters (ASCII only).
pub const MAX_USERNAME_LEN: usize = 32;

/// Maximum allowed length for the password hash field.
pub const MAX_PASSWORD_FIELD_LEN: usize = 256;

/// Maximum allowed length for the public key base64 field.
pub const MAX_PUBLIC_KEY_FIELD_LEN: usize = 256;

/// Allowed clock skew in seconds between client and server timestamps.
pub const MAX_CLOCK_SKEW_SECS: f64 = 300.0;

/// Maximum length of a nonce string.
pub const MAX_NONCE_LEN: usize = 64;

/// Maximum number of recently seen nonces remembered per user.
pub const NONCE_CACHE_CAP: usize = 256;

/// How often the nonce cleanup task runs (seconds).
pub const NONCE_CLEANUP_INTERVAL_SECS: u64 = 60;

/// Age threshold for nonce eviction (seconds).
pub const NONCE_MAX_AGE_SECS: f64 = MAX_CLOCK_SKEW_SECS * 2.0;

/// Maximum number of concurrent connections from a single IP.
pub const MAX_CONNECTIONS_PER_IP: usize = 5;

/// Minimum interval between auth attempts from the same IP (seconds).
pub const AUTH_RATE_LIMIT_SECS: f64 = 0.5;

/// Maximum file transfer size in bytes (100 MB).
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum filename length.
pub const MAX_FILE_NAME_LEN: usize = 256;

/// Maximum transfer identifier length.
pub const MAX_FILE_ID_LEN: usize = 128;

/// Maximum MIME type length.
pub const MAX_MEDIA_MIME_LEN: usize = 128;

/// Maximum allowed length for status text.
pub const MAX_STATUS_TEXT_LEN: usize = 128;

/// Maximum allowed length for status emoji.
pub const MAX_STATUS_EMOJI_LEN: usize = 16;

/// Maximum allowed length for message IDs.
pub const MAX_MSG_ID_LEN: usize = 64;

/// Maximum allowed length for reaction emoji.
pub const MAX_REACTION_EMOJI_LEN: usize = 32;

// ============================================================================
// Message Type Constants
// ============================================================================

/// Client-to-server message types.
pub mod msg_type {
    /// Authentication request (first frame required).
    pub const AUTH: &str = "auth";
    /// Self-registration request.
    pub const REGISTER: &str = "register";
    /// Channel message.
    pub const MSG: &str = "msg";
    /// Direct message.
    pub const DM: &str = "dm";
    /// Join a channel.
    pub const JOIN: &str = "join";
    /// Leave a channel.
    pub const LEAVE: &str = "leave";
    /// Status update.
    pub const STATUS: &str = "status";
    /// Search request.
    pub const SEARCH: &str = "search";
    /// History request.
    pub const HISTORY: &str = "history";
    /// Users list request.
    pub const USERS: &str = "users";
    /// Server info request.
    pub const INFO: &str = "info";
    /// Keepalive ping.
    pub const PING: &str = "ping";
    /// Typing indicator.
    pub const TYPING: &str = "typing";
    /// Reaction to a message.
    pub const REACTION: &str = "reaction";
    /// Edit a message.
    pub const EDIT: &str = "edit";
    /// Delete a message.
    pub const DELETE: &str = "delete";
    /// Replay events since timestamp.
    pub const REPLAY: &str = "replay";
    /// Rewind to time window.
    pub const REWIND: &str = "rewind";
    /// File metadata announcement.
    pub const FILE_META: &str = "file_meta";
    /// File chunk transfer.
    pub const FILE_CHUNK: &str = "file_chunk";
    /// Join voice room.
    pub const VJOIN: &str = "vjoin";
    /// Leave voice room.
    pub const VLEAVE: &str = "vleave";
    /// Voice data.
    pub const VDATA: &str = "vdata";
    /// Ban a user.
    pub const BAN: &str = "ban";
    /// Unban a user.
    pub const UNBAN: &str = "unban";
    /// Mute a user.
    pub const MUTE: &str = "mute";
    /// Unmute a user.
    pub const UNMUTE: &str = "unmute";
    /// Kick a user.
    pub const KICK: &str = "kick";
    /// Slash command.
    pub const SLASH: &str = "slash";
    /// 2FA setup request.
    pub const FA_SETUP: &str = "2fa_setup";
    /// 2FA enable confirmation.
    pub const FA_ENABLE: &str = "2fa_enable";
    /// 2FA disable request.
    pub const FA_DISABLE: &str = "2fa_disable";
    /// 2FA verification.
    pub const FA_VERIFY: &str = "2fa_verify";
    /// Password change.
    pub const PASSWORD_CHANGE: &str = "password_change";
}

/// Server-to-client message types.
pub mod srv_type {
    /// Authentication success.
    pub const OK: &str = "ok";
    /// Error response.
    pub const ERR: &str = "err";
    /// Channel message.
    pub const MSG: &str = "msg";
    /// Join confirmation.
    pub const JOINED: &str = "joined";
    /// Leave confirmation.
    pub const LEFT: &str = "left";
    /// Status update broadcast.
    pub const STATUS_UPDATE: &str = "status_update";
    /// Typing indicator.
    pub const TYPING: &str = "typing";
    /// Reaction broadcast.
    pub const REACTION: &str = "reaction";
    /// Server info response.
    pub const INFO: &str = "info";
    /// Users list response.
    pub const USERS: &str = "users";
    /// History response.
    pub const HISTORY: &str = "history";
    /// Search results.
    pub const SEARCH_RESULTS: &str = "search_results";
    /// Pong response.
    pub const PONG: &str = "pong";
    /// System message.
    pub const SYSTEM: &str = "system";
    /// File metadata.
    pub const FILE_META: &str = "file_meta";
    /// File chunk.
    pub const FILE_CHUNK: &str = "file_chunk";
    /// Voice joined.
    pub const VJOINED: &str = "vjoined";
    /// Voice left.
    pub const VLEFT: &str = "vleft";
    /// Voice data.
    pub const VDATA: &str = "vdata";
    /// Banned notification.
    pub const BANNED: &str = "banned";
    /// Unbanned notification.
    pub const UNBANNED: &str = "unbanned";
    /// Muted notification.
    pub const MUTED: &str = "muted";
    /// Unmuted notification.
    pub const UNMUTED: &str = "unmuted";
    /// Kicked notification.
    pub const KICKED: &str = "kicked";
    /// 2FA setup response.
    pub const FA_SETUP: &str = "2fa_setup";
    /// 2FA enabled response.
    pub const FA_ENABLED: &str = "2fa_enabled";
    /// 2FA disabled response.
    pub const FA_DISABLED: &str = "2fa_disabled";
    /// Password changed.
    pub const PASSWORD_CHANGED: &str = "password_changed";
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Creates a DM channel name for a given user.
pub fn dm_channel_name(user: &str) -> String {
    format!("{}{}", DM_CHANNEL_PREFIX, user)
}

/// Clamps a raw `limit` parameter from a client request to `[1, max]`.
pub fn clamp_limit(raw: Option<u64>, default: usize, max: usize) -> usize {
    match raw {
        None => default,
        Some(0) => default,
        Some(n) if n > max as u64 => max,
        Some(n) => n as usize,
    }
}

/// Returns the current Unix timestamp as seconds (f64).
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Validates that a username meets protocol requirements.
pub fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validates that a message ID meets protocol requirements.
pub fn is_valid_msg_id(msg_id: &str) -> bool {
    if msg_id.is_empty() || msg_id.len() > MAX_MSG_ID_LEN {
        return false;
    }
    msg_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validates that an emoji is a valid reaction token.
pub fn is_valid_reaction_emoji(emoji: &str) -> bool {
    !emoji.trim().is_empty() && emoji.len() <= MAX_REACTION_EMOJI_LEN
}

/// Returns true if the event type requires replay protection (nonce + timestamp).
pub fn requires_replay_protection(event_type: &str) -> bool {
    matches!(
        event_type,
        msg_type::MSG
            | msg_type::DM
            | msg_type::VDATA
            | msg_type::EDIT
            | msg_type::FILE_META
            | msg_type::FILE_CHUNK
            | msg_type::STATUS
            | msg_type::REACTION
    )
}

// ============================================================================
// Response Builders
// ============================================================================

/// Creates a success response with optional fields.
pub fn ok_response() -> Value {
    serde_json::json!({ "t": srv_type::OK })
}

/// Creates an error response.
pub fn err_response(message: &str) -> Value {
    serde_json::json!({
        "t": srv_type::ERR,
        "m": message
    })
}

/// Creates an auth success response with full session data.
pub fn auth_ok_response(
    username: &str,
    channels: Vec<Value>,
    users: Vec<Value>,
    history: Vec<Value>,
    proto_version: u64,
    max_payload: usize,
    token: Option<&str>,
) -> String {
    let mut response = serde_json::json!({
        "t": srv_type::OK,
        "u": username,
        "channels": channels,
        "users": users,
        "hist": history,
        "proto": {
            "v": proto_version,
            "max_payload_bytes": max_payload
        }
    });

    if let Some(t) = token {
        response["token"] = serde_json::json!(t);
    }

    response.to_string()
}

/// Creates a system message (e.g., user joined/left notifications).
pub fn system_message(content: &str) -> Value {
    serde_json::json!({
        "t": srv_type::SYSTEM,
        "c": content,
        "ts": now()
    })
}

/// Sanitizes a channel name for safe use.
pub fn sanitize_channel_name(raw: &str) -> Option<String> {
    let s: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
