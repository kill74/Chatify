//! Server state management for Chatify.
//!
//! This module provides the central server state structure that is shared
//! across all connection handler tasks via Arc.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use chatify::metrics::PrometheusMetrics;
use chatify::performance::{Metrics as PerfMetrics, VecCache};
use chatify::voice::VoiceRelay;
use dashmap::DashMap;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, Notify, RwLock};

use crate::db::EventStore;
use crate::plugin_runtime::PluginRuntime;
use crate::protocol::{DM_CHANNEL_PREFIX, HISTORY_CAP, MAX_CONNECTIONS_PER_IP};

// ============================================================================
// Channel Types
// ============================================================================

/// A named chat channel consisting of a bounded in-memory history ring buffer
/// and a [tokio broadcast] channel for real-time fan-out to all subscribers.
///
/// `Channel` is cheap to clone; all clones share the same `Arc`-wrapped
/// history and the same `broadcast::Sender` handle. New subscribers obtain a
/// fresh `Receiver` via `tx.subscribe()`.
#[derive(Clone)]
pub struct Channel {
    /// In-memory ring buffer of the last [`HISTORY_CAP`] messages.
    /// Wrapped in `Arc<RwLock<…>>` so multiple tasks can read concurrently
    /// while writes are exclusive.
    pub history: Arc<RwLock<VecDeque<Value>>>,

    /// Broadcast sender. The channel capacity (256) is deliberately larger
    /// than [`HISTORY_CAP`] to absorb short bursts without dropping frames.
    pub tx: broadcast::Sender<String>,
}

impl Channel {
    /// Creates a new, empty channel with a 256-message broadcast buffer.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))),
            tx,
        }
    }

    /// Appends `entry` to the in-memory history, evicting the oldest entry if
    /// the ring buffer is at capacity.
    pub async fn push(&self, entry: Value) {
        let mut h = self.history.write().await;
        if h.len() >= HISTORY_CAP {
            h.pop_front();
        }
        h.push_back(entry);
    }

    /// Returns a snapshot of the current history as a `Vec`, oldest first.
    pub async fn hist(&self) -> Vec<Value> {
        self.history.read().await.iter().cloned().collect()
    }
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata for a connected bridge (e.g. Discord ↔ Chatify).
#[derive(Clone, Debug)]
pub struct BridgeInfo {
    /// Username of the bridge.
    pub username: String,
    /// Bridge type identifier.
    pub bridge_type: String,
    /// Instance ID for loop prevention.
    pub instance_id: String,
    /// Timestamp when connected.
    pub connected_at: f64,
    /// Number of bridge routes.
    pub route_count: usize,
}

/// In-memory session metadata, keyed by a digest of the bearer token.
#[derive(Clone, Debug)]
pub struct SessionRecord {
    /// Authenticated username associated with the session.
    pub username: String,
    /// Unix timestamp when the session was created.
    pub issued_at: f64,
    /// Unix timestamp when the session was most recently accepted.
    pub last_seen_at: f64,
}

/// Absolute lifetime for a session token.
const SESSION_TTL_SECS: f64 = 12.0 * 60.0 * 60.0;
/// Idle lifetime for a session token.
const SESSION_IDLE_TTL_SECS: f64 = 60.0 * 60.0;

// ============================================================================
// Normalize helpers
// ============================================================================

const OUTBOUND_QUEUE_CAPACITY_DEFAULT: usize = 1024;
const OUTBOUND_QUEUE_CAPACITY_MIN: usize = 64;
const OUTBOUND_QUEUE_CAPACITY_MAX: usize = 16_384;
const SLOW_CLIENT_DROP_BURST_DEFAULT: usize = 64;
const SLOW_CLIENT_DROP_BURST_MIN: usize = 1;
const SLOW_CLIENT_DROP_BURST_MAX: usize = 4096;

pub fn normalize_outbound_queue_capacity(requested: usize) -> usize {
    let requested = if requested == 0 {
        OUTBOUND_QUEUE_CAPACITY_DEFAULT
    } else {
        requested
    };
    requested.clamp(OUTBOUND_QUEUE_CAPACITY_MIN, OUTBOUND_QUEUE_CAPACITY_MAX)
}

pub fn normalize_slow_client_drop_burst(requested: usize) -> usize {
    let requested = if requested == 0 {
        SLOW_CLIENT_DROP_BURST_DEFAULT
    } else {
        requested
    };
    requested.clamp(SLOW_CLIENT_DROP_BURST_MIN, SLOW_CLIENT_DROP_BURST_MAX)
}

// ============================================================================
// Server State
// ============================================================================

/// Central server state shared by all connection handler tasks via `Arc`.
///
/// Every field uses a lock-free concurrent map ([`DashMap`]) or atomic
/// primitive so that individual operations (insert, remove, lookup) do not
/// require global locking. Per-channel operations that require exclusive
/// history access use `tokio::sync::RwLock` scoped to the specific channel.
pub struct State {
    /// Named public channels, keyed by sanitised channel name.
    /// DM channels live here too under the `__dm__<username>` naming
    /// convention; they are filtered out when listing channels to clients.
    pub channels: DashMap<String, Channel>,

    /// Per-room voice broadcast senders, keyed by room name.
    pub voice: DashMap<String, broadcast::Sender<String>>,

    /// Per-room screen-share relay senders, keyed by room name.
    pub screen: DashMap<String, broadcast::Sender<String>>,

    /// Current status value for each online user
    /// (e.g. `{"text":"Online","emoji":"✓"}`).
    /// Presence in this map is the authoritative signal that a user is online.
    pub user_statuses: DashMap<String, Value>,

    /// Public key (base64) for each online user, used by clients to encrypt
    /// DM payloads without a separate key-exchange round-trip.
    pub user_pubkeys: DashMap<String, String>,

    /// Per-user ring buffer of recently seen nonce values.
    /// Bounded to [`NONCE_CACHE_CAP`] entries; the oldest entry is evicted
    /// once the cap is reached.
    pub recent_nonces: DashMap<String, VecDeque<String>>,

    /// Last-seen timestamp for each user's nonce cache entry.
    /// Updated on every nonce validation. Used by the periodic cleanup
    /// task to evict stale entries from `recent_nonces` when a user's
    /// connection drops without proper cleanup (crash, network partition).
    pub nonce_last_seen: DashMap<String, f64>,

    /// Number of WebSocket connections currently open. Managed via
    /// [`ConnectionGuard`] RAII to guarantee accurate accounting even on
    /// panics.
    pub active_connections: AtomicUsize,

    /// Notified whenever `active_connections` reaches zero, allowing the
    /// graceful-shutdown loop to wake immediately rather than polling.
    pub drained_notify: Notify,

    /// SQLite-backed event persistence and 2-FA storage.
    pub store: EventStore,

    /// Per-IP connection count for rate limiting.
    /// Incremented on TCP accept, decremented on disconnect.
    pub ip_connections: DashMap<IpAddr, usize>,

    /// Per-IP last auth timestamp for auth rate limiting.
    /// Enforces a minimum 500ms interval between auth attempts from the same IP.
    pub ip_last_auth: DashMap<IpAddr, f64>,

    /// Session tokens keyed by token string → username.
    /// Generated at auth time and validated when a client supplies a token.
    pub session_tokens: DashMap<String, SessionRecord>,

    /// Transient client credential hashes accepted while a password change is
    /// being re-hashed and persisted server-side.
    pub pending_credentials: DashMap<String, String>,

    /// Connected bridge instances, keyed by username.
    /// Populated during auth when the client sends `"bridge": true`.
    pub bridges: DashMap<String, BridgeInfo>,

    /// Internal metrics for runtime stats and debugging.
    pub metrics: PerfMetrics,

    /// Prometheus metrics for export.
    pub prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,

    pub message_cache: VecCache<Value>,

    /// Voice channel relay for managing voice rooms, members, and state
    pub voice_relay: VoiceRelay,

    /// Flag to signal graceful shutdown in progress.
    /// When true, server stops accepting new connections.
    pub shutdown_in_progress: AtomicBool,

    /// Shutdown trigger for external signaling (SIGHUP, shutdown endpoint).
    pub shutdown_notify: Notify,

    /// Per-user message rate limiting: username -> (count, window_start).
    /// Uses DashMap for concurrent access without locking.
    pub user_msg_rate: DashMap<String, (u32, f64)>,

    /// Maximum messages per user per minute.
    pub max_msgs_per_minute: u32,

    /// Whether per-user rate limiting is enabled.
    pub user_rate_limit_enabled: bool,

    /// Whether self-registration is enabled via CLI flag.
    pub self_registration_enabled: bool,

    /// Per-connection outbound queue capacity.
    pub outbound_queue_capacity: usize,

    /// Number of consecutive dropped non-blocking outbound messages that
    /// triggers a slow-client disconnect.
    pub slow_client_drop_burst: usize,

    /// Plugin runtime manager (API v1).
    pub plugin_runtime: PluginRuntime,
}

impl State {
    /// Creates the initial server state, pre-populating the `"general"` channel.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db_path: String,
        db_key: Option<Vec<u8>>,
        db_durability: crate::args::DbDurabilityMode,
        db_pool_size: u32,
        prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
        plugin_runtime: PluginRuntime,
        max_msgs_per_minute: u32,
        user_rate_limit_enabled: bool,
        self_registration_enabled: bool,
        outbound_queue_capacity: usize,
        slow_client_drop_burst: usize,
    ) -> Arc<Self> {
        let outbound_queue_capacity = normalize_outbound_queue_capacity(outbound_queue_capacity);
        let slow_client_drop_burst = normalize_slow_client_drop_burst(slow_client_drop_burst);
        let store = EventStore::new(
            db_path,
            db_key,
            db_durability,
            db_pool_size,
            prometheus.clone(),
        );
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            screen: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            recent_nonces: DashMap::new(),
            nonce_last_seen: DashMap::new(),
            active_connections: AtomicUsize::new(0),
            drained_notify: Notify::new(),
            store,
            ip_connections: DashMap::new(),
            ip_last_auth: DashMap::new(),
            session_tokens: DashMap::new(),
            pending_credentials: DashMap::new(),
            bridges: DashMap::new(),
            metrics: PerfMetrics::new(),
            prometheus,
            message_cache: VecCache::new(1000),
            voice_relay: VoiceRelay::new(),
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_notify: Notify::new(),
            user_msg_rate: DashMap::new(),
            max_msgs_per_minute,
            user_rate_limit_enabled,
            self_registration_enabled,
            outbound_queue_capacity,
            slow_client_drop_burst,
            plugin_runtime,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    /// Returns the [`Channel`] for `name`, creating it lazily on first access.
    pub fn chan(&self, name: &str) -> Channel {
        self.channels.entry(name.into()).or_default().clone()
    }

    /// Returns the voice broadcast sender for `room`, creating it lazily on
    /// first access.
    pub fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.voice
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone()
    }

    /// Returns the screen-share relay sender for `room`, creating it lazily
    /// on first access.
    pub fn screen_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.screen
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone()
    }

    /// Returns the number of currently online users.
    pub fn online_count(&self) -> usize {
        self.user_statuses.len()
    }

    /// Serialises the list of public (non-DM) channel names as a JSON array.
    pub fn channels_json(&self) -> Value {
        Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with(DM_CHANNEL_PREFIX))
                .map(|e| Value::String(e.key().clone()))
                .collect(),
        )
    }

    /// Serialises the list of online users with their public keys as a JSON
    /// array of `{"u": "...", "pk": "..."}` objects.
    pub fn users_with_keys_json(&self) -> Value {
        Value::Array(
            self.user_pubkeys
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "u": e.key(),
                        "pk": e.value()
                    })
                })
                .collect(),
        )
    }

    fn connection_opened(&self) {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
        self.metrics.inc_accepted();
        if let Some(ref m) = self.prometheus {
            if let Ok(mutex_guard) = m.lock() {
                mutex_guard.record_connection_accepted();
            }
        }
    }

    fn connection_closed(&self) {
        let prev = self.active_connections.fetch_sub(1, Ordering::SeqCst);
        self.metrics.inc_closed();
        if let Some(ref m) = self.prometheus {
            if let Ok(mutex_guard) = m.lock() {
                mutex_guard.record_connection_closed();
            }
        }
        if prev <= 1 {
            self.drained_notify.notify_waiters();
        }
    }

    /// Returns the number of active connections.
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

    /// Increments the per-IP connection counter. Returns `false` if the
    /// IP has exceeded [`MAX_CONNECTIONS_PER_IP`].
    pub fn ip_connect(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        let mut entry = self.ip_connections.entry(ip).or_insert(0);
        if *entry >= MAX_CONNECTIONS_PER_IP {
            return false;
        }
        *entry += 1;
        true
    }

    /// Decrements the per-IP connection counter, removing the entry if it
    /// reaches zero.
    pub fn ip_disconnect(&self, addr: &SocketAddr) {
        let ip = addr.ip();
        if let Some(mut entry) = self.ip_connections.get_mut(&ip) {
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                drop(entry);
                self.ip_connections.remove(&ip);
            }
        }
    }

    /// Checks whether an auth attempt from `addr` is allowed.
    /// Enforces a minimum 500ms interval between auth attempts from the same IP.
    pub fn ip_auth_allowed(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        let now = chatify::now();
        let min_interval = 0.5;

        let mut last_auth = self.ip_last_auth.entry(ip).or_insert(0.0);
        if now - *last_auth < min_interval {
            return false;
        }
        *last_auth = now;
        true
    }

    /// Check and record per-user message rate limit.
    /// Returns (allowed, remaining, reset_in_secs).
    pub fn check_user_rate_limit(&self, username: &str) -> (bool, u32, u64) {
        if !self.user_rate_limit_enabled {
            return (true, self.max_msgs_per_minute, 60);
        }

        if self.max_msgs_per_minute == 0 {
            return (true, u32::MAX, 60);
        }

        let now = chatify::now();
        let window_secs = 60.0;

        let mut entry = self
            .user_msg_rate
            .entry(username.to_string())
            .or_insert((0, now));

        if now - entry.1 >= window_secs {
            entry.0 = 1;
            entry.1 = now;
            return (true, self.max_msgs_per_minute - 1, 60);
        }

        if entry.0 >= self.max_msgs_per_minute {
            let reset_in = (window_secs - (now - entry.1)) as u64;
            return (false, 0, reset_in);
        }

        entry.0 += 1;
        let remaining = self.max_msgs_per_minute - entry.0;
        let reset_in = (window_secs - (now - entry.1)) as u64;
        (true, remaining, reset_in)
    }

    /// Signal the server to begin graceful shutdown.
    /// Returns true if shutdown was initiated, false if already shutting down.
    pub fn initiate_shutdown(&self) -> bool {
        if self
            .shutdown_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.shutdown_notify.notify_waiters();
            true
        } else {
            false
        }
    }

    /// Check if server is shutting down.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_in_progress.load(Ordering::SeqCst)
    }

    pub fn notify_shutdown(&self) {
        self.shutdown_notify.notify_waiters();
    }

    /// Returns the non-reversible storage key for a raw session token.
    pub fn session_token_digest(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"chatify:session:v1:");
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Creates a new session token for a user.
    pub fn create_session(&self, username: &str) -> String {
        use rand::{rngs::OsRng, RngCore};
        let mut bytes = <[u8; 32]>::default();
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        let now = chatify::now();
        self.session_tokens.insert(
            Self::session_token_digest(&token),
            SessionRecord {
                username: username.to_string(),
                issued_at: now,
                last_seen_at: now,
            },
        );
        token
    }

    /// Ends a user's session.
    pub fn end_session(&self, username: &str) {
        self.session_tokens
            .retain(|_, record| record.username != username);
    }

    /// Validates a session token for a user and refreshes its idle timestamp.
    pub fn validate_session_token(&self, username: &str, token: Option<&str>) -> bool {
        let Some(token) = token else {
            return false;
        };
        let digest = Self::session_token_digest(token);
        let now = chatify::now();
        let Some(mut record) = self.session_tokens.get_mut(&digest) else {
            return false;
        };

        if record.username != username
            || now - record.issued_at > SESSION_TTL_SECS
            || now - record.last_seen_at > SESSION_IDLE_TTL_SECS
        {
            drop(record);
            self.session_tokens.remove(&digest);
            return false;
        }

        record.last_seen_at = now;
        true
    }

    /// Invalidates all sessions for a user, for example after password change.
    pub fn invalidate_all_user_sessions(&self, username: &str) {
        self.end_session(username);
    }

    /// Check if user can send messages in a channel (checks bans, mutes, and permissions)
    pub fn can_send(&self, username: &str, channel: &str) -> bool {
        if let Some(ban) = self.store.is_banned(username, channel) {
            if ban.is_active() {
                return false;
            }
        }
        if let Some(mute) = self.store.is_muted(username, channel) {
            if mute.is_active() {
                return false;
            }
        }
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.permissions.contains(crate::db::RolePermissions::SEND),
            None => true,
        }
    }

    /// Check if user can kick others in a channel
    pub fn can_kick(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.can_kick(),
            None => false,
        }
    }

    /// Check if user can ban others in a channel
    pub fn can_ban(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.can_ban(),
            None => false,
        }
    }

    /// Check if user can mute others in a channel
    pub fn can_mute(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.can_mute(),
            None => false,
        }
    }

    /// Check if user can perform administrative management actions.
    pub fn can_manage(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.can_manage(),
            None => false,
        }
    }

    /// Get user's role in a channel
    pub fn get_user_role(&self, username: &str, channel: &str) -> Option<String> {
        self.store.get_user_role(username, channel).map(|r| r.name)
    }

    /// Check if user is banned in a channel
    pub fn is_banned(&self, username: &str, channel: &str) -> bool {
        self.store
            .is_banned(username, channel)
            .map(|b| b.is_active())
            .unwrap_or(false)
    }

    /// Check if user is muted in a channel
    pub fn is_muted(&self, username: &str, channel: &str) -> bool {
        self.store
            .is_muted(username, channel)
            .map(|m| m.is_active())
            .unwrap_or(false)
    }
}

// ============================================================================
// Connection Guard
// ============================================================================

/// RAII guard that increments [`State::active_connections`] on construction
/// and decrements it on drop, guaranteeing accurate accounting even when a
/// connection handler panics or returns early.
pub struct ConnectionGuard {
    state: Arc<State>,
    addr: SocketAddr,
}

impl ConnectionGuard {
    /// Creates a new guard, incrementing active_connections.
    pub fn new(state: Arc<State>, addr: SocketAddr) -> Self {
        state.connection_opened();
        Self { state, addr }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.state.ip_disconnect(&self.addr);
        self.state.connection_closed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::DbDurabilityMode;
    use crate::plugin_runtime::PluginRuntime;

    fn test_state() -> Arc<State> {
        State::new(
            ":memory:".to_string(),
            None,
            DbDurabilityMode::Balanced,
            8,
            None,
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe")),
            60,
            true,
            true,
            1024,
            64,
        )
    }

    #[test]
    fn session_tokens_are_stored_by_digest_and_validate() {
        let state = test_state();
        let token = state.create_session("alice");
        let digest = State::session_token_digest(&token);

        assert!(!state.session_tokens.contains_key(&token));
        assert!(state.session_tokens.contains_key(&digest));
        assert!(state.validate_session_token("alice", Some(&token)));
        assert!(!state.validate_session_token("alice", None));

        state.end_session("alice");
        assert!(!state.session_tokens.contains_key(&digest));
    }

    #[test]
    fn invalid_or_expired_session_tokens_are_removed() {
        let state = test_state();

        let mismatched = state.create_session("alice");
        let mismatched_digest = State::session_token_digest(&mismatched);
        assert!(!state.validate_session_token("bob", Some(&mismatched)));
        assert!(!state.session_tokens.contains_key(&mismatched_digest));

        let expired = state.create_session("alice");
        let expired_digest = State::session_token_digest(&expired);
        if let Some(mut record) = state.session_tokens.get_mut(&expired_digest) {
            record.issued_at -= SESSION_TTL_SECS + 1.0;
        }
        assert!(!state.validate_session_token("alice", Some(&expired)));
        assert!(!state.session_tokens.contains_key(&expired_digest));
    }
}
