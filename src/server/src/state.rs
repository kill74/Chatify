//! Server state management for Chatify.
//!
//! This module provides the central server state structure that is shared
//! across all connection handler tasks via Arc.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::{broadcast, Notify};

use crate::protocol::DM_CHANNEL_PREFIX;

// ============================================================================
// Channel Types
// ============================================================================

/// A named chat channel with broadcast messaging and bounded history.
#[derive(Clone)]
pub struct Channel {
    /// Channel name.
    pub name: String,
    /// Broadcast sender for real-time fan-out to all subscribers.
    pub tx: broadcast::Sender<String>,
    /// Bounded in-memory history ring buffer.
    pub history: Arc<tokio::sync::Mutex<VecDeque<(String, f64)>>>,
}

impl Channel {
    /// Creates a new channel with default settings.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self {
            name: String::new(),
            tx,
            history: Arc::new(tokio::sync::Mutex::new(VecDeque::with_capacity(50))),
        }
    }

    /// Adds a message to the channel history.
    pub fn add_message(&self, msg: String, ts: f64) {
        let mut hist = self.history.blocking_lock();
        hist.push_back((msg, ts));
        if hist.len() > 50 {
            hist.pop_front();
        }
    }

    /// Returns the last N messages from history.
    pub async fn hist(&self, limit: usize) -> Vec<Value> {
        let hist = self.history.lock().await;
        hist.iter()
            .rev()
            .take(limit)
            .map(|(msg, ts)| {
                serde_json::json!({
                    "c": msg,
                    "ts": ts
                })
            })
            .collect()
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

// ============================================================================
// Server State
// ============================================================================

/// Central server state shared by all connection handler tasks.
///
/// Every field uses a lock-free concurrent map ([`DashMap`]) or atomic
/// primitive so that individual operations do not require global locking.
pub struct State {
    /// Named public channels, keyed by sanitised channel name.
    pub channels: DashMap<String, Channel>,
    /// Per-room voice broadcast senders.
    pub voice: DashMap<String, broadcast::Sender<String>>,
    /// Current status value for each online user.
    pub user_statuses: DashMap<String, Value>,
    /// Public key (base64) for each online user.
    pub user_pubkeys: DashMap<String, String>,
    /// Per-user ring buffer of recently seen nonce values.
    pub recent_nonces: DashMap<String, VecDeque<String>>,
    /// Last-seen timestamp for each user's nonce cache entry.
    pub nonce_last_seen: DashMap<String, f64>,
    /// Number of WebSocket connections currently open.
    pub active_connections: AtomicUsize,
    /// Notified when active_connections reaches zero.
    pub drained_notify: Notify,
    /// SQLite-backed event persistence and 2-FA storage.
    pub store: crate::db::EventStore,
    /// Per-IP connection count for rate limiting.
    pub ip_connections: DashMap<std::net::IpAddr, usize>,
    /// Per-IP last auth attempt timestamp.
    pub ip_last_auth: DashMap<std::net::IpAddr, f64>,
    /// Session tokens keyed by token → username.
    pub session_tokens: DashMap<String, String>,
    /// Connected bridge instances, keyed by username.
    pub bridges: DashMap<String, BridgeInfo>,
    /// Internal metrics for runtime stats.
    pub metrics: chatify::performance::Metrics,
    /// Prometheus metrics for export.
    pub prometheus: Option<Arc<std::sync::Mutex<chatify::metrics::PrometheusMetrics>>>,
    /// Flag to signal graceful shutdown in progress.
    pub shutdown_in_progress: AtomicBool,
    /// Shutdown trigger for external signaling.
    pub shutdown_notify: Notify,
    /// Per-user message rate limiting: username → (count, window_start).
    pub user_msg_rate: DashMap<String, (u32, f64)>,
    /// Maximum messages per user per minute.
    pub max_msgs_per_minute: u32,
    /// Whether per-user rate limiting is enabled.
    pub user_rate_limit_enabled: bool,
    /// Whether self-registration is enabled.
    pub self_registration_enabled: bool,
    /// Plugin runtime manager.
    pub plugin_runtime: crate::plugin_runtime::PluginRuntime,
}

impl State {
    /// Creates the initial server state with the "general" channel pre-populated.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db_path: String,
        db_key: Option<Vec<u8>>,
        db_durability: crate::args::DbDurabilityMode,
        prometheus: Option<Arc<std::sync::Mutex<chatify::metrics::PrometheusMetrics>>>,
        plugin_runtime: crate::plugin_runtime::PluginRuntime,
        max_msgs_per_minute: u32,
        user_rate_limit_enabled: bool,
        self_registration_enabled: bool,
    ) -> Arc<Self> {
        let store = crate::db::EventStore::new(db_path, db_key, db_durability, prometheus.clone());

        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
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
            bridges: DashMap::new(),
            metrics: chatify::performance::Metrics::new(),
            prometheus,
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_notify: Notify::new(),
            user_msg_rate: DashMap::new(),
            max_msgs_per_minute,
            user_rate_limit_enabled,
            self_registration_enabled,
            plugin_runtime,
        });

        // Pre-create the "general" channel
        s.channels.insert("general".into(), Channel::new());
        s
    }

    /// Returns or creates a channel by name.
    pub fn chan(&self, name: &str) -> Channel {
        self.channels.entry(name.into()).or_default().clone()
    }

    /// Returns the number of active connections.
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

    /// Attempts to register a new IP connection. Returns false if limit exceeded.
    pub fn ip_connect(&self, addr: &std::net::SocketAddr) -> bool {
        let ip = addr.ip();
        let mut count = self.ip_connections.entry(ip).or_insert(0);
        if *count >= 5 {
            false
        } else {
            *count += 1;
            true
        }
    }

    /// Decrements the connection count for an IP.
    pub fn ip_disconnect(&self, addr: &std::net::SocketAddr) {
        let ip = addr.ip();
        if let Some(mut count) = self.ip_connections.get_mut(&ip) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                drop(count);
                self.ip_connections.remove(&ip);
            }
        }
    }

    /// Returns true if this is the first shutdown request.
    pub fn initiate_shutdown(&self) -> bool {
        self.shutdown_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Returns whether shutdown is in progress.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_in_progress.load(Ordering::SeqCst)
    }

    /// Returns the number of online users.
    pub fn online_count(&self) -> usize {
        self.user_statuses.len()
    }

    /// Returns channel names as JSON array (excluding DM channels).
    pub fn channels_json(&self) -> Value {
        Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with(DM_CHANNEL_PREFIX))
                .map(|e| Value::String(e.key().clone()))
                .collect(),
        )
    }

    /// Returns user info as JSON array.
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

    /// Creates a new session token for a user.
    pub fn create_session(&self, username: &str) -> String {
        use rand::{rngs::OsRng, RngCore};
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        self.session_tokens
            .insert(token.clone(), username.to_string());
        token
    }

    /// Ends a user's session.
    pub fn end_session(&self, username: &str) {
        // Find and remove the token for this user
        let token_to_remove: Option<String> = self
            .session_tokens
            .iter()
            .find(|v| *v.value() == username)
            .map(|v| v.key().clone());

        if let Some(token) = token_to_remove {
            self.session_tokens.remove(token.as_str());
        }
    }

    /// Checks if an auth attempt from this IP is allowed.
    pub fn ip_auth_allowed(&self, addr: &std::net::SocketAddr) -> bool {
        let ip = addr.ip();
        let now = crate::protocol::now();
        if let Some(last) = self.ip_last_auth.get(&ip) {
            if now - *last < 0.5 {
                return false;
            }
        }
        self.ip_last_auth.insert(ip, now);
        true
    }

    /// Checks rate limit for a user. Returns (allowed, remaining, reset_in).
    pub fn check_user_rate_limit(&self, username: &str) -> (bool, u32, f64) {
        if !self.user_rate_limit_enabled {
            return (true, self.max_msgs_per_minute, 0.0);
        }

        let now = crate::protocol::now();
        let mut entry = self
            .user_msg_rate
            .entry(username.to_string())
            .or_insert((0, now));

        let (count, window_start) = &mut *entry;
        if now - *window_start >= 60.0 {
            *count = 0;
            *window_start = now;
        }

        *count += 1;
        if *count > self.max_msgs_per_minute {
            let reset_in = 60.0 - (now - *window_start);
            (false, 0, reset_in)
        } else {
            (
                true,
                self.max_msgs_per_minute.saturating_sub(*count),
                60.0 - (now - *window_start),
            )
        }
    }

    /// Checks if a user can send to a channel.
    pub fn can_send(&self, username: &str, channel: &str) -> bool {
        if let Ok(muted) = self.store.is_user_muted(username, channel) {
            !muted
        } else {
            true
        }
    }
}

// ============================================================================
// Connection Guard
// ============================================================================

/// RAII guard that tracks active connections.
pub struct ConnectionGuard {
    state: Arc<State>,
    addr: std::net::SocketAddr,
}

impl ConnectionGuard {
    /// Creates a new guard, incrementing active_connections.
    pub fn new(state: Arc<State>, addr: std::net::SocketAddr) -> Self {
        state.active_connections.fetch_add(1, Ordering::SeqCst);
        Self { state, addr }
    }

    /// Decrements active_connections and notifies if zero.
    pub fn cleanup(&self) {
        let prev = self.state.active_connections.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            self.state.drained_notify.notify_waiters();
        }
        self.state.ip_disconnect(&self.addr);
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}
