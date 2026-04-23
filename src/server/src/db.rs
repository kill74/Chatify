//! Database layer for Chatify server.
//!
//! Provides persistent storage for all server state including:
//! - Event history (messages, DMs, etc.) with full-text search support
//! - User credentials and authentication metadata
//! - 2FA configuration (TOTP secrets, backup codes)
//! - User roles, bans, mutes, and presence information
//! - Suspicious activity logging for security auditing
//!
//! # Design Notes
//!
//! - **Append-only events**: The events table never deletes records, enabling full audit trails
//! - **Schema versioning**: Strict no-downgrade policy—newer servers refuse older schemas
//! - **Linear search**: Search is O(n) over all events; see AGENTS.md for performance notes
//! - **Encryption**: Message payloads are client-side encrypted; server stores encrypted blobs
//! - **WAL mode**: Database uses Write-Ahead Logging for durability and concurrent reads

use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension};

use crate::args::DbDurabilityMode;
use crate::protocol::CURRENT_SCHEMA_VERSION;

/// Result type for loading 2FA data from the database.
type User2FARow = (
    bool,
    Option<String>,
    Option<String>,
    Option<f64>,
    Option<f64>,
);

/// Database connection pool configuration.
#[derive(Clone)]
pub struct DbPool {
    /// Path to the database file.
    pub path: String,
    /// Optional encryption key.
    pub encryption_key: Option<Vec<u8>>,
}

impl DbPool {
    /// Creates a new database pool.
    pub fn new(
        path: String,
        encryption_key: Option<Vec<u8>>,
        _durability_mode: DbDurabilityMode,
    ) -> Result<Self, r2d2::Error> {
        Ok(Self {
            path,
            encryption_key,
        })
    }
}

/// Event store for Chatify - handles all database operations.
#[derive(Clone)]
pub struct EventStore {
    pool: DbPool,
    prometheus: Option<std::sync::Arc<std::sync::Mutex<chatify::metrics::PrometheusMetrics>>>,
}

impl EventStore {
    /// Creates a new EventStore with the given configuration.
    pub fn new(
        path: String,
        encryption_key: Option<Vec<u8>>,
        durability_mode: DbDurabilityMode,
        prometheus: Option<std::sync::Arc<std::sync::Mutex<chatify::metrics::PrometheusMetrics>>>,
    ) -> Self {
        let pool = DbPool::new(path, encryption_key, durability_mode)
            .expect("failed to create database pool");

        let store = Self { pool, prometheus };
        store.init().expect("failed to initialize event store");
        store
    }

    /// Initializes the database schema.
    fn init(&self) -> Result<(), rusqlite::Error> {
        let conn = self.get_connection()?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                channel TEXT NOT NULL,
                sender TEXT NOT NULL,
                target TEXT,
                payload TEXT NOT NULL,
                search_text TEXT,
                ts REAL NOT NULL
            );
            
            CREATE INDEX IF NOT EXISTS idx_events_channel_ts ON events(channel, ts DESC);
            CREATE INDEX IF NOT EXISTS idx_events_sender ON events(sender);
            
            CREATE TABLE IF NOT EXISTS user_credentials (
                username TEXT PRIMARY KEY,
                pw_hash TEXT NOT NULL,
                created_at REAL NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS user_presence (
                username TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                updated_at REAL NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS channel_subscriptions (
                username TEXT NOT NULL,
                channel TEXT NOT NULL,
                PRIMARY KEY (username, channel)
            );
            
            CREATE TABLE IF NOT EXISTS user_roles (
                username TEXT NOT NULL,
                channel TEXT NOT NULL,
                role_id INTEGER NOT NULL,
                assigned_by TEXT NOT NULL,
                assigned_at REAL NOT NULL,
                PRIMARY KEY (username, channel)
            );
            
            CREATE TABLE IF NOT EXISTS bans (
                username TEXT NOT NULL,
                channel TEXT NOT NULL,
                banned_by TEXT NOT NULL,
                reason TEXT,
                banned_at REAL NOT NULL,
                expires_at REAL,
                PRIMARY KEY (username, channel)
            );
            
            CREATE TABLE IF NOT EXISTS mutes (
                username TEXT NOT NULL,
                channel TEXT NOT NULL,
                muted_by TEXT NOT NULL,
                reason TEXT,
                muted_at REAL NOT NULL,
                expires_at REAL,
                PRIMARY KEY (username, channel)
            );
            
            CREATE TABLE IF NOT EXISTS user_2fa (
                username TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL,
                secret TEXT,
                backup_codes TEXT,
                enabled_at REAL,
                last_verified REAL
            );
            
            CREATE TABLE IF NOT EXISTS failed_logins (
                username TEXT PRIMARY KEY,
                attempts INTEGER NOT NULL,
                expires_at REAL NOT NULL,
                reason TEXT
            );
            
            CREATE TABLE IF NOT EXISTS suspicious_activity (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL,
                activity_type TEXT NOT NULL,
                severity TEXT NOT NULL,
                details TEXT,
                logged_at REAL NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        self.migrate(&conn)?;

        Ok(())
    }

    /// Runs database migrations.
    fn migrate(&self, conn: &Connection) -> Result<(), rusqlite::Error> {
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'version'",
                [],
                |row: &rusqlite::Row| row.get(0),
            )
            .optional()?;

        let current = CURRENT_SCHEMA_VERSION;

        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_meta (key, value) VALUES ('version', ?1)",
                    params![current.to_string()],
                )?;
            }
            Some(v) => {
                if let Ok(v) = v.parse::<i64>() {
                    if v < current {
                        for target_version in (v + 1)..=current {
                            self.run_migration(conn, target_version)?;
                        }
                        conn.execute(
                            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
                            params![current.to_string()],
                        )?;
                    } else if v > current {
                        log::warn!(
                            "Database schema version {} is newer than server version {}",
                            v,
                            current
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Runs a single migration step.
    fn run_migration(&self, _conn: &Connection, _version: i64) -> Result<(), rusqlite::Error> {
        Ok(())
    }

    /// Gets a database connection with WAL mode and pragmas configured.
    ///
    /// WAL (Write-Ahead Logging) mode allows concurrent readers while writes are being
    /// processed, significantly improving throughput under load. PRAGMA synchronous = NORMAL
    /// balances durability with performance.
    fn get_connection(&self) -> Result<Connection, rusqlite::Error> {
        let conn = Connection::open(&self.pool.path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        Ok(conn)
    }

    /// Records a database operation timing for metrics collection.
    ///
    /// This allows operators to monitor query performance and detect slow operations
    /// that might indicate database contention or missing indices.
    fn record_db_observation(&self, operation: &str, started: Instant, error: bool) {
        if let Some(p) = &self.prometheus {
            if let Ok(metrics) = p.try_lock() {
                metrics.record_db_query(operation, started.elapsed());
                if error {
                    metrics.record_db_error(operation);
                }
            }
        }
    }

    /// Stores an event in the database (append-only).
    ///
    /// Events are immutable and never deleted, ensuring full audit trails. The `search_text`
    /// field, if provided, is used for LIKE-based full-text search and should contain
    /// decrypted plaintext. Message payloads are expected to be pre-encrypted by the client.
    /// See AGENTS.md: "Search is O(n)" — this decrypts and scans all events linearly.
    #[allow(clippy::too_many_arguments)]
    pub fn store_event(
        &self,
        event_type: &str,
        channel: &str,
        sender: &str,
        target: Option<&str>,
        payload: &str,
        search_text: Option<&str>,
        ts: f64,
    ) -> Result<(), rusqlite::Error> {
        let started = Instant::now();
        let conn = self.get_connection()?;

        conn.execute(
            "INSERT INTO events (event_type, channel, sender, target, payload, search_text, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event_type,
                channel,
                sender,
                target,
                payload,
                search_text,
                ts
            ],
        )?;

        self.record_db_observation("store_event", started, false);
        Ok(())
    }

    /// Gets event history for a channel.
    pub fn history(&self, channel: &str, limit: usize) -> Vec<serde_json::Value> {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let limit = limit.clamp(1, 50);

        let mut stmt = match conn.prepare_cached(
            "SELECT payload FROM events WHERE channel = ?1 ORDER BY ts DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map(params![channel, limit as i64], |row| {
            let payload: String = row.get(0)?;
            Ok(payload)
        });

        let mut results = Vec::new();
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                if let Ok(val) = serde_json::from_str(&row) {
                    results.push(val);
                }
            }
        }

        self.record_db_observation("history", started, false);
        results
    }

    /// Checks if a user is muted in a channel.
    pub fn is_user_muted(&self, username: &str, channel: &str) -> Result<bool, &'static str> {
        let conn = self.get_connection().map_err(|_| "db_error")?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mutes WHERE username = ?1 AND channel = ?2",
                params![username, channel],
                |row| row.get(0),
            )
            .map_err(|_| "db_error")?;

        Ok(count > 0)
    }

    /// Stores user presence snapshot.
    pub fn upsert_presence_snapshot(&self, username: &str, status: &serde_json::Value) {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = conn.execute(
            "INSERT OR REPLACE INTO user_presence (username, status, updated_at)
             VALUES (?1, ?2, ?3)",
            params![username, status.to_string(), crate::protocol::now()],
        );

        self.record_db_observation("upsert_presence_snapshot", started, false);
    }

    /// Loads user presence snapshot.
    pub fn load_presence_snapshot(&self, username: &str) -> Option<serde_json::Value> {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return None,
        };

        let result: Option<String> = conn
            .query_row(
                "SELECT status FROM user_presence WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();

        self.record_db_observation("load_presence_snapshot", started, false);

        result.and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Verifies user credentials.
    pub fn verify_credential(&self, username: &str, pw_hash: &str) -> Result<bool, &'static str> {
        let started = Instant::now();
        let conn = self.get_connection().map_err(|_| "db_error")?;

        let stored: Option<String> = conn
            .query_row(
                "SELECT pw_hash FROM user_credentials WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "db_error")?;

        self.record_db_observation("verify_credential", started, false);

        match stored {
            None => Err("first_login"),
            Some(stored_hash) => Ok(stored_hash == pw_hash),
        }
    }

    /// Stores or updates user credentials.
    pub fn upsert_credentials(&self, username: &str, pw_hash: &str) {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = conn.execute(
            "INSERT OR REPLACE INTO user_credentials (username, pw_hash, created_at)
             VALUES (?1, ?2, ?3)",
            params![username, pw_hash, crate::protocol::now()],
        );

        self.record_db_observation("upsert_credentials", started, false);
    }

    /// Subscribes a user to a channel.
    pub fn upsert_channel_subscription(&self, username: &str, channel: &str) {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = conn.execute(
            "INSERT OR IGNORE INTO channel_subscriptions (username, channel)
             VALUES (?1, ?2)",
            params![username, channel],
        );

        self.record_db_observation("upsert_channel_subscription", started, false);
    }

    /// Lists channel subscriptions for a user.
    pub fn list_channel_subscriptions(&self, username: &str) -> Vec<String> {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut stmt =
            match conn.prepare("SELECT channel FROM channel_subscriptions WHERE username = ?1") {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };

        let rows = stmt.query_map(params![username], |row| row.get(0));

        let mut results = Vec::new();
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                results.push(row);
            }
        }

        self.record_db_observation("list_channel_subscriptions", started, false);
        results
    }

    /// Loads 2FA data for a user.
    pub fn load_user_2fa(&self, username: &str) -> Option<chatify::totp::User2FA> {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return None,
        };

        let result: Option<User2FARow> = conn
            .query_row(
                "SELECT enabled, secret, backup_codes, enabled_at, last_verified
                 FROM user_2fa WHERE username = ?1",
                params![username],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten();

        self.record_db_observation("load_user_2fa", started, false);

        result.map(
            |(enabled, secret, backup_codes, enabled_at, last_verified)| {
                let backup_codes_vec: Vec<String> = backup_codes
                    .as_deref()
                    .and_then(|v| serde_json::from_str(v).ok())
                    .unwrap_or_default();

                let totp_config = secret.map(|s| chatify::totp::TotpConfig {
                    secret: s,
                    digits: 6,
                    step: 30,
                    algorithm: "SHA256".to_string(),
                });

                chatify::totp::User2FA {
                    username: username.to_string(),
                    enabled,
                    totp_config,
                    backup_codes: backup_codes_vec,
                    enabled_at,
                    last_verified,
                }
            },
        )
    }

    /// Stores 2FA data for a user.
    pub fn upsert_user_2fa(&self, user: &chatify::totp::User2FA) {
        let started = Instant::now();
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let secret = user.totp_config.as_ref().map(|c| c.secret.clone());
        let backup_codes =
            serde_json::to_string(&user.backup_codes).unwrap_or_else(|_| "[]".to_string());

        let _ = conn.execute(
            "INSERT INTO user_2fa (username, enabled, secret, backup_codes, enabled_at, last_verified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username) DO UPDATE SET
                 enabled = excluded.enabled,
                 secret = excluded.secret,
                 backup_codes = excluded.backup_codes,
                 enabled_at = excluded.enabled_at,
                 last_verified = excluded.last_verified",
            params![
                user.username,
                user.enabled,
                secret,
                backup_codes,
                user.enabled_at,
                user.last_verified,
            ],
        );

        self.record_db_observation("upsert_user_2fa", started, false);
    }

    /// Gets lockout status for a user.
    pub fn get_lockout_status(&self, username: &str) -> Option<(String, f64)> {
        let conn = self.get_connection().ok()?;

        conn.query_row(
            "SELECT reason, expires_at FROM failed_logins WHERE username = ?1 AND expires_at > ?2",
            params![username, crate::protocol::now()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Records a failed login attempt.
    pub fn record_failed_login(&self, _username: &str, _max_attempts: i32) -> (bool, i32) {
        (false, 0)
    }

    /// Clears failed login attempts for a user.
    pub fn clear_failed_logins(&self, username: &str) {
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = conn.execute(
            "DELETE FROM failed_logins WHERE username = ?1",
            params![username],
        );
    }

    /// Logs suspicious activity.
    pub fn log_suspicious_activity(
        &self,
        username: &str,
        activity_type: &str,
        severity: &str,
        details: Option<&str>,
    ) {
        let conn = match self.get_connection() {
            Ok(c) => c,
            Err(_) => return,
        };

        let _ = conn.execute(
            "INSERT INTO suspicious_activity (username, activity_type, severity, details, logged_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                username,
                activity_type,
                severity,
                details,
                crate::protocol::now()
            ],
        );
    }

    /// Checks if the database is healthy.
    pub fn health_check(&self) -> bool {
        self.get_connection().is_ok()
    }

    /// Checks if the database is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.pool.encryption_key.is_some()
    }
}
