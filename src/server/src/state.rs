//! Server state management for Chatify.
//!
//! This module provides the central server state structure that is shared
//! across all connection handler tasks via Arc.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use chatify::metrics::PrometheusMetrics;
use chatify::performance::{Metrics as PerfMetrics, VecCache};
use chatify::voice::VoiceRelay;
use dashmap::{DashMap, DashSet};
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

    /// Unix timestamp (seconds) of the last access. Used by the periodic
    /// sweeper to reclaim channels that were created by arbitrary client
    /// input and are no longer in use.
    pub last_activity: Arc<AtomicU64>,
}

impl Channel {
    /// Creates a new, empty channel with a 256-message broadcast buffer.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))),
            tx,
            last_activity: Arc::new(AtomicU64::new(chatify::now_secs())),
        }
    }

    /// Records that this channel was just used.
    pub fn touch(&self) {
        self.last_activity
            .store(chatify::now_secs(), Ordering::Relaxed);
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

/// Tracks an announced media transfer so chunk frames can be validated
/// against the sender and declared size from the corresponding `file_meta`.
#[derive(Clone, Debug)]
pub struct MediaTransferState {
    pub sender: String,
    pub declared_size: u64,
    pub received_bytes: u64,
    pub last_activity: f64,
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

pub const OUTBOUND_QUEUE_CAPACITY_DEFAULT: usize = 1024;
pub const OUTBOUND_QUEUE_CAPACITY_MIN: usize = 64;
pub const OUTBOUND_QUEUE_CAPACITY_MAX: usize = 16_384;
pub const SLOW_CLIENT_DROP_BURST_DEFAULT: usize = 64;
pub const SLOW_CLIENT_DROP_BURST_MIN: usize = 1;
pub const SLOW_CLIENT_DROP_BURST_MAX: usize = 4096;

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

    /// Last-access timestamps for voice rooms, used by the idle sweeper.
    pub voice_activity: DashMap<String, u64>,

    /// Per-room screen-share relay senders, keyed by room name.
    pub screen: DashMap<String, broadcast::Sender<String>>,

    /// Last-access timestamps for screen-share rooms, used by the idle sweeper.
    pub screen_activity: DashMap<String, u64>,

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

    /// Usernames with an in-flight first-registration. Prevents two
    /// simultaneous first-logins from both passing the `first_login` check
    /// before either credential row has been persisted.
    pub pending_registrations: DashSet<String>,

    /// Connected bridge instances, keyed by username.
    /// Populated during auth when the client sends `"bridge": true`.
    pub bridges: DashMap<String, BridgeInfo>,

    /// In-flight media announcements, keyed by `(channel, file_id)`.
    pub media_transfers: DashMap<(String, String), MediaTransferState>,

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

    /// Whether per-IP auth rate limiting is enabled.
    /// The binary may disable this behind NAT/localhost where it would
    /// interfere with legitimate multi-client bootstrap.
    pub ip_auth_enabled: bool,
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
        ip_auth_enabled: bool,
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
            voice_activity: DashMap::new(),
            screen: DashMap::new(),
            screen_activity: DashMap::new(),
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
            pending_registrations: DashSet::new(),
            bridges: DashMap::new(),
            media_transfers: DashMap::new(),
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
            ip_auth_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    /// Returns the [`Channel`] for `name`, creating it lazily on first access.
    pub fn chan(&self, name: &str) -> Channel {
        let channel = self.channels.entry(name.into()).or_default().clone();
        channel.touch();
        channel
    }

    /// Returns the voice broadcast sender for `room`, creating it lazily on
    /// first access.
    pub fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        let tx = self
            .voice
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone();
        self.voice_activity
            .insert(room.to_string(), chatify::now_secs());
        tx
    }

    /// Returns the screen-share relay sender for `room`, creating it lazily
    /// on first access.
    pub fn screen_tx(&self, room: &str) -> broadcast::Sender<String> {
        let tx = self
            .screen
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone();
        self.screen_activity
            .insert(room.to_string(), chatify::now_secs());
        tx
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
    /// When `ip_auth_enabled` is false, always returns true (used by the server
    /// binary behind NAT/localhost where IP throttle interferes with bootstrap).
    pub fn ip_auth_allowed(&self, addr: &SocketAddr) -> bool {
        if !self.ip_auth_enabled {
            return true;
        }
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
        let mut bytes = <[u8; 32]>::default();
        chatify::os_random_bytes(&mut bytes);
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

    /// Returns the digest under which a raw session token is stored.
    ///
    /// The connection keeps this digest so every frame can be checked against
    /// the authoritative server-side record without trusting a client-supplied
    /// token field.
    pub fn session_digest(token: &str) -> String {
        Self::session_token_digest(token)
    }

    /// Returns true while the session identified by `digest` is still valid.
    ///
    /// This is the authoritative check performed on every received frame. A
    /// session is invalidated immediately by `end_session` /
    /// `invalidate_all_user_sessions` (e.g. on disconnect or password change),
    /// so a client cannot keep operating after its session has been revoked by
    /// simply omitting the optional token field.
    pub fn session_is_active(&self, username: &str, digest: &str) -> bool {
        let now = chatify::now();
        let Some(mut record) = self.session_tokens.get_mut(digest) else {
            return false;
        };

        if record.username != username
            || now - record.issued_at > SESSION_TTL_SECS
            || now - record.last_seen_at > SESSION_IDLE_TTL_SECS
        {
            // Only remove records that are actually expired or malformed. A
            // username mismatch must never let one user delete another
            // user's live session (session invalidation by omission/knowing a
            // token). Explicit revocation goes through `end_session`.
            let expired = now - record.issued_at > SESSION_TTL_SECS
                || now - record.last_seen_at > SESSION_IDLE_TTL_SECS;
            drop(record);
            if expired {
                self.session_tokens.remove(digest);
            }
            return false;
        }

        record.last_seen_at = now;
        true
    }

    /// Validates an optional client-supplied session token for a user.
    ///
    /// Kept for backwards compatibility with clients that include a token on
    /// individual frames. Revocation is enforced unconditionally via
    /// `session_is_active` against the connection's own session digest.
    pub fn validate_session_token(&self, username: &str, token: Option<&str>) -> bool {
        let Some(token) = token else {
            return false;
        };
        let digest = Self::session_token_digest(token);
        self.session_is_active(username, &digest)
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

    // -----------------------------------------------------------------------
    // Nonce cache maintenance
    // -----------------------------------------------------------------------

    /// Removes stale nonce cache entries for users whose last activity is
    /// older than `max_age_secs`. Called periodically by the server binary.
    pub fn evict_stale_nonce_entries(&self, max_age_secs: f64) -> usize {
        let cutoff = chatify::now() - max_age_secs;
        let stale_keys: Vec<String> = self
            .nonce_last_seen
            .iter()
            .filter_map(|entry| {
                if *entry.value() < cutoff {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        let count = stale_keys.len();
        for key in &stale_keys {
            self.nonce_last_seen.remove(key);
            self.recent_nonces.remove(key);
        }
        count
    }

    /// Reclaims idle in-memory channels and rooms created by client input.
    ///
    /// Only entries with no remaining broadcast subscribers and no activity
    /// for `max_age_secs` are removed, so active conversations are untouched.
    pub fn prune_idle_channels(&self, max_age_secs: u64) -> usize {
        let now = chatify::now_secs();
        let mut pruned = 0usize;

        self.channels.retain(|name, channel| {
            let keep = name == "general"
                || now.saturating_sub(channel.last_activity.load(Ordering::Relaxed)) < max_age_secs
                || channel.tx.receiver_count() > 0;
            if !keep {
                pruned += 1;
            }
            keep
        });

        self.voice.retain(|room, tx| {
            let activity = self
                .voice_activity
                .get(room)
                .map(|v| *v.value())
                .unwrap_or(0);
            let keep = now.saturating_sub(activity) < max_age_secs || tx.receiver_count() > 0;
            if !keep {
                self.voice_activity.remove(room);
                pruned += 1;
            }
            keep
        });

        self.screen.retain(|room, tx| {
            let activity = self
                .screen_activity
                .get(room)
                .map(|v| *v.value())
                .unwrap_or(0);
            let keep = now.saturating_sub(activity) < max_age_secs || tx.receiver_count() > 0;
            if !keep {
                self.screen_activity.remove(room);
                pruned += 1;
            }
            keep
        });

        pruned
    }

    /// Evicts expired session tokens and stale auth/rate-limit bookkeeping.
    /// Without this, entries for users/IPs that disappear without a clean
    /// disconnect accumulate for the lifetime of the process.
    pub fn prune_stale_metadata(&self) -> usize {
        let now = chatify::now();
        let mut pruned = 0usize;

        self.session_tokens.retain(|_, record| {
            let keep = now - record.issued_at <= SESSION_TTL_SECS
                && now - record.last_seen_at <= SESSION_IDLE_TTL_SECS;
            if !keep {
                pruned += 1;
            }
            keep
        });
        // Also drop activity timestamps for rooms that have since been
        // removed, keeping those bookkeeping maps bounded.
        let voice_rooms: std::collections::HashSet<String> =
            self.voice.iter().map(|e| e.key().clone()).collect();
        self.voice_activity
            .retain(|room, _| voice_rooms.contains(room));
        let screen_rooms: std::collections::HashSet<String> =
            self.screen.iter().map(|e| e.key().clone()).collect();
        self.screen_activity
            .retain(|room, _| screen_rooms.contains(room));

        self.ip_last_auth.retain(|_, last| {
            let keep = now - *last < 3600.0;
            if !keep {
                pruned += 1;
            }
            keep
        });
        self.user_msg_rate.retain(|_, (_, window_start)| {
            let keep = now - *window_start < 120.0;
            if !keep {
                pruned += 1;
            }
            keep
        });
        self.media_transfers.retain(|_, transfer| {
            let keep = now - transfer.last_activity < 3600.0;
            if !keep {
                pruned += 1;
            }
            keep
        });

        pruned
    }

    /// Registers a `file_meta` announcement. Returns false when the file id
    /// is already in use by a different sender in the same channel.
    pub fn announce_media_transfer(
        &self,
        channel: &str,
        file_id: &str,
        sender: &str,
        declared_size: u64,
    ) -> bool {
        match self
            .media_transfers
            .entry((channel.to_string(), file_id.to_string()))
        {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                if entry.get().sender != sender {
                    return false;
                }
                let transfer = entry.get_mut();
                transfer.declared_size = declared_size;
                transfer.received_bytes = 0;
                transfer.last_activity = chatify::now();
                true
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(MediaTransferState {
                    sender: sender.to_string(),
                    declared_size,
                    received_bytes: 0,
                    last_activity: chatify::now(),
                });
                true
            }
        }
    }

    /// Validates and accounts for an incoming chunk. Returns false if there
    /// is no matching announcement, the sender differs, or the chunk would
    /// push the transfer past its declared size.
    pub fn accept_media_chunk(
        &self,
        channel: &str,
        file_id: &str,
        sender: &str,
        chunk_len: usize,
    ) -> bool {
        let Some(mut transfer) = self
            .media_transfers
            .get_mut(&(channel.to_string(), file_id.to_string()))
        else {
            return false;
        };
        if transfer.sender != sender || chunk_len == 0 {
            return false;
        }
        let added = chunk_len as u64;
        if transfer
            .received_bytes
            .checked_add(added)
            .is_some_and(|total| total <= transfer.declared_size)
        {
            transfer.received_bytes += added;
            transfer.last_activity = chatify::now();
            true
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Suspicious Activity Detection
    // -----------------------------------------------------------------------

    const SPAM_MSG_THRESHOLD: usize = 30;
    const SPAM_WINDOW_SECS: f64 = 60.0;
    const RAID_JOIN_THRESHOLD: usize = 10;
    const RAID_WINDOW_SECS: f64 = 60.0;

    pub fn check_and_alert_spam(&self, username: &str, _channel: &str) {
        self.store.log_suspicious_activity(
            username,
            "spam",
            "low",
            Some("message activity recorded"),
        );

        let recent = self
            .store
            .get_recent_activity_count(username, "spam", Self::SPAM_WINDOW_SECS);
        if recent < Self::SPAM_MSG_THRESHOLD as i64 {
            return;
        }

        let already_alerted =
            self.store
                .get_recent_activity_count(username, "spam_alert", Self::SPAM_WINDOW_SECS);
        if already_alerted > 0 {
            return;
        }

        self.store.log_suspicious_activity(
            username,
            "spam_alert",
            "medium",
            Some(&format!(
                "user sent {} messages in {}s window",
                recent,
                Self::SPAM_WINDOW_SECS
            )),
        );
    }

    pub fn check_and_alert_raid(&self, username: &str, _channel: &str) {
        self.store.log_suspicious_activity(
            username,
            "raid_join",
            "low",
            Some("channel join activity recorded"),
        );

        let recent =
            self.store
                .get_recent_activity_count(username, "raid_join", Self::RAID_WINDOW_SECS);
        if recent < Self::RAID_JOIN_THRESHOLD as i64 {
            return;
        }

        let already_alerted =
            self.store
                .get_recent_activity_count(username, "raid_alert", Self::RAID_WINDOW_SECS);
        if already_alerted > 0 {
            return;
        }

        self.store.log_suspicious_activity(
            username,
            "raid_alert",
            "high",
            Some(&format!(
                "user joined {} channels in {}s window",
                recent,
                Self::RAID_WINDOW_SECS
            )),
        );
        self.broadcast_alert(
            "raid",
            username,
            "high",
            &format!(
                "Possible raid detected: user {} joining multiple channels rapidly",
                username
            ),
        );
    }

    pub fn broadcast_alert(&self, alert_type: &str, target: &str, severity: &str, message: &str) {
        let alert = serde_json::json!({
            "t": "alert",
            "alert_type": alert_type,
            "target": target,
            "severity": severity,
            "message": message,
            "ts": chatify::now()
        })
        .to_string();

        for channel in self.channels.iter() {
            let _ = channel.tx.send(alert.clone());
        }
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
            true,
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
        assert!(state.session_is_active("alice", &digest));

        state.end_session("alice");
        assert!(!state.session_tokens.contains_key(&digest));
        // A revoked session must be rejected even if the client keeps sending
        // frames without a token field.
        assert!(!state.session_is_active("alice", &digest));
    }

    #[test]
    fn invalid_or_expired_session_tokens_are_removed() {
        let state = test_state();

        // Presenting another user's valid token must not invalidate their
        // session — otherwise any user who learns a token can log its owner
        // out by sending a single frame with a mismatched username.
        let mismatched = state.create_session("alice");
        let mismatched_digest = State::session_token_digest(&mismatched);
        assert!(!state.validate_session_token("bob", Some(&mismatched)));
        assert!(state.session_tokens.contains_key(&mismatched_digest));
        assert!(state.session_is_active("alice", &mismatched_digest));

        let expired = state.create_session("alice");
        let expired_digest = State::session_token_digest(&expired);
        if let Some(mut record) = state.session_tokens.get_mut(&expired_digest) {
            record.issued_at -= SESSION_TTL_SECS + 1.0;
        }
        assert!(!state.validate_session_token("alice", Some(&expired)));
        assert!(!state.session_tokens.contains_key(&expired_digest));
    }

    #[test]
    fn idle_channels_are_pruned_but_general_is_kept() {
        let state = test_state();
        let idle = state.chan("idle-room");
        idle.last_activity
            .store(chatify::now_secs().saturating_sub(7200), Ordering::Relaxed);
        // An active subscriber must keep the channel alive.
        let subscribed = state.chan("subscribed-room");
        let _receiver = subscribed.tx.subscribe();
        subscribed
            .last_activity
            .store(chatify::now_secs().saturating_sub(7200), Ordering::Relaxed);

        assert_eq!(state.prune_idle_channels(3600), 1);
        assert!(!state.channels.contains_key("idle-room"));
        assert!(state.channels.contains_key("general"));
        assert!(state.channels.contains_key("subscribed-room"));
    }

    #[test]
    fn media_transfers_enforce_sender_and_declared_size() {
        let state = test_state();
        assert!(!state.accept_media_chunk("general", "f1", "alice", 8));
        assert!(state.announce_media_transfer("general", "f1", "alice", 16));
        assert!(state.accept_media_chunk("general", "f1", "alice", 8));
        assert!(state.accept_media_chunk("general", "f1", "alice", 8));
        // One byte over the declared size must be rejected.
        assert!(!state.accept_media_chunk("general", "f1", "alice", 1));
        // An unknown file id and a foreign sender must be rejected.
        assert!(!state.accept_media_chunk("general", "unknown", "alice", 1));
        assert!(!state.accept_media_chunk("general", "f1", "bob", 1));
        // Another sender cannot hijack the same file id.
        assert!(!state.announce_media_transfer("general", "f1", "bob", 8));
    }

    #[test]
    fn stale_metadata_is_pruned() {
        let state = test_state();
        let token = state.create_session("alice");
        let digest = State::session_token_digest(&token);
        if let Some(mut record) = state.session_tokens.get_mut(&digest) {
            record.issued_at -= SESSION_TTL_SECS + 1.0;
        }
        state
            .ip_last_auth
            .insert(std::net::IpAddr::from([127, 0, 0, 9]), 1.0);

        assert!(state.prune_stale_metadata() >= 2);
        assert!(!state.session_tokens.contains_key(&digest));
        assert!(state.ip_last_auth.is_empty());
    }
}
