//! Server state - incremental refactoring target.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::sync::{broadcast, Notify};

pub struct State {
    pub channels: DashMap<String, Channel>,
    pub voice: DashMap<String, broadcast::Sender<String>>,
    pub user_statuses: DashMap<String, serde_json::Value>,
    pub user_pubkeys: DashMap<String, String>,
    pub recent_nonces: DashMap<String, VecDeque<String>>,
    pub active_connections: AtomicUsize,
    pub drained_notify: Notify,
    pub store: crate::db::EventStore,
    pub ip_connections: DashMap<std::net::IpAddr, usize>,
    pub ip_last_auth: DashMap<std::net::IpAddr, f64>,
    pub session_tokens: DashMap<String, String>,
    pub bridges: DashMap<String, BridgeInfo>,
    pub prometheus: Option<Arc<std::sync::Mutex<clifford::metrics::PrometheusMetrics>>>,
    pub shutdown_in_progress: AtomicBool,
    pub shutdown_notify: Notify,
    pub user_msg_rate: DashMap<String, (u32, f64)>,
    pub max_msgs_per_minute: u32,
    pub user_rate_limit_enabled: bool,
}

#[derive(Clone)]
pub struct Channel {
    pub name: String,
    pub tx: broadcast::Sender<String>,
    pub history: Arc<Mutex<VecDeque<(String, f64)>>>, // (message, timestamp)
}

pub struct BridgeInfo {
    pub username: String,
    pub bridge_type: String,
    pub instance_id: String,
    pub connected_at: f64,
    pub route_count: usize,
}

impl State {
    pub fn new(
        db_path: String,
        db_key: Option<Vec<u8>>,
        prometheus: Option<Arc<std::sync::Mutex<clifford::metrics::PrometheusMetrics>>>,
        max_msgs_per_minute: u32,
        user_rate_limit_enabled: bool,
    ) -> Arc<Self> {
        let store = crate::db::EventStore::new(db_path, db_key);
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            recent_nonces: DashMap::new(),
            active_connections: AtomicUsize::new(0),
            drained_notify: Notify::new(),
            store,
            ip_connections: DashMap::new(),
            ip_last_auth: DashMap::new(),
            session_tokens: DashMap::new(),
            bridges: DashMap::new(),
            prometheus,
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_notify: Notify::new(),
            user_msg_rate: DashMap::new(),
            max_msgs_per_minute,
            user_rate_limit_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    pub fn chan(&self, name: &str) -> Channel {
        if let Some(ch) = self.channels.get(name) {
            return Channel {
                name: ch.name.clone(),
                tx: ch.tx.clone(),
                history: ch.history.clone(),
            };
        }
        let ch = Channel::new();
        self.channels.insert(name.to_string(), ch.clone());
        ch
    }

    pub fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

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

    pub fn initiate_shutdown(&self) -> bool {
        self.shutdown_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_in_progress.load(Ordering::SeqCst)
    }

    pub fn online_count(&self) -> usize {
        self.user_statuses.len()
    }

    pub fn channels_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| serde_json::Value::String(e.key().clone()))
                .collect(),
        )
    }

    pub fn can_send(&self, _username: &str, _channel: &str) -> bool {
        true
    }

    pub fn is_muted(&self, _username: &str, _channel: &str) -> bool {
        false
    }

    pub fn create_session(&self, username: &str) -> String {
        let token = clifford::fresh_nonce_hex();
        self.session_tokens
            .insert(token.clone(), username.to_string());
        token
    }

    pub fn end_session(&self, username: &str) {
        let to_remove: Option<String> = self
            .session_tokens
            .iter()
            .find(|v| v.value() == username)
            .map(|v| v.key().clone());
        if let Some(token) = to_remove {
            self.session_tokens.remove(&token);
        }
    }

    pub fn ip_auth_allowed(&self, addr: &std::net::SocketAddr) -> bool {
        let ip = addr.ip();
        let now = clifford::now();
        if let Some(last) = self.ip_last_auth.get(&ip) {
            if now - *last < 0.5 {
                return false;
            }
        }
        self.ip_last_auth.insert(ip, now);
        true
    }

    pub fn check_user_rate_limit(&self, username: &str) -> (bool, u32, f64) {
        if !self.user_rate_limit_enabled {
            return (true, self.max_msgs_per_minute, 0.0);
        }
        let now = clifford::now();
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
            (false, self.max_msgs_per_minute - *count, reset_in)
        } else {
            (
                true,
                self.max_msgs_per_minute - *count,
                60.0 - (now - *window_start),
            )
        }
    }
}

impl Channel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self {
            name: String::new(),
            tx,
            history: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
        }
    }

    pub fn add_message(&self, msg: String, ts: f64) {
        let mut hist = self.history.lock();
        hist.push_back((msg, ts));
        if hist.len() > 1000 {
            hist.pop_front();
        }
    }

    pub fn hist(&self, limit: usize) -> Vec<serde_json::Value> {
        let hist = self.history.lock();
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
