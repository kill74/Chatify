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

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use chatify::crypto;
use chatify::error::{ChatifyError, ChatifyResult};
use chatify::metrics::PrometheusMetrics;
use chatify::performance::PoolStats;
use log::warn;
use r2d2;
use rusqlite::{params, Connection, Error as SqlError, OptionalExtension};
use serde_json::Value;

use crate::args::DbDurabilityMode;
use crate::protocol::*;

// ---------------------------------------------------------------------------
// Roles & Permissions System
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Clone, Debug, Default)]
    pub struct RolePermissions: u32 {
        const NONE      = 0;
        const VIEW       = 1 << 0;
        const SEND       = 1 << 1;
        const KICK       = 1 << 2;
        const BAN        = 1 << 3;
        const MUTE       = 1 << 4;
        const MANAGE     = 1 << 5;
        const PIN        = 1 << 6;
    }
}

impl RolePermissions {
    pub fn from_db_row(
        can_kick: bool,
        can_ban: bool,
        can_mute: bool,
        can_manage: bool,
        can_pin: bool,
    ) -> Self {
        let mut perms = Self::NONE;
        perms |= Self::VIEW | Self::SEND;
        if can_kick {
            perms |= Self::KICK;
        }
        if can_ban {
            perms |= Self::BAN;
        }
        if can_mute {
            perms |= Self::MUTE;
        }
        if can_manage {
            perms |= Self::MANAGE;
        }
        if can_pin {
            perms |= Self::PIN;
        }
        perms
    }
}

#[derive(Clone, Debug)]
pub struct Role {
    pub id: i64,
    pub name: String,
    pub level: i32,
    pub permissions: RolePermissions,
}

impl Role {
    fn from_row(row: RoleRow) -> Self {
        let (id, name, level, can_kick, can_ban, can_mute, can_manage, can_pin) = row;
        let mut permissions =
            RolePermissions::from_db_row(can_kick, can_ban, can_mute, can_manage, can_pin);
        if name == "readonly" {
            permissions.remove(RolePermissions::SEND);
        }

        Self {
            id,
            name,
            level,
            permissions,
        }
    }

    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.level >= 100
    }

    #[must_use]
    pub fn can_kick(&self) -> bool {
        self.permissions.contains(RolePermissions::KICK)
    }

    #[must_use]
    pub fn can_ban(&self) -> bool {
        self.permissions.contains(RolePermissions::BAN)
    }

    #[must_use]
    pub fn can_mute(&self) -> bool {
        self.permissions.contains(RolePermissions::MUTE)
    }

    #[must_use]
    pub fn can_manage(&self) -> bool {
        self.permissions.contains(RolePermissions::MANAGE)
    }
}

#[derive(Clone, Debug)]
pub struct Ban {
    pub username: String,
    pub channel: String,
    pub banned_by: String,
    pub reason: Option<String>,
    pub banned_at: f64,
    pub expires_at: Option<f64>,
}

impl Ban {
    #[must_use]
    pub fn is_active(&self) -> bool {
        if let Some(expires) = self.expires_at {
            chatify::now() < expires
        } else {
            true
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mute {
    pub username: String,
    pub channel: String,
    pub muted_by: String,
    pub reason: Option<String>,
    pub muted_at: f64,
    pub expires_at: Option<f64>,
}

impl Mute {
    #[must_use]
    pub fn is_active(&self) -> bool {
        if let Some(expires) = self.expires_at {
            chatify::now() < expires
        } else {
            true
        }
    }
}

// ---------------------------------------------------------------------------
// EventStore — SQLite persistence layer with connection pooling
// ---------------------------------------------------------------------------

const DB_POOL_SIZE_DEFAULT: u32 = 8;
const DB_POOL_SIZE_MIN: u32 = 1;
const DB_POOL_SIZE_MAX: u32 = 128;
const DB_POOL_MIN_IDLE: u32 = 2;
const DB_POOL_IDLE_TIMEOUT_SECS: u64 = 60;
const DB_BUSY_TIMEOUT_SECS: u64 = 5;
const ENCRYPTED_SEARCH_SCAN_CAP: usize = 100_000;
const MEDIA_CHUNK_ENC_PREFIX: &[u8] = b"cfm1";

fn normalize_db_pool_size(requested: u32) -> u32 {
    let requested = if requested == 0 {
        DB_POOL_SIZE_DEFAULT
    } else {
        requested
    };
    requested.clamp(DB_POOL_SIZE_MIN, DB_POOL_SIZE_MAX)
}

#[derive(Clone)]
pub struct PooledConnection {
    path: String,
    durability_mode: DbDurabilityMode,
}

impl PooledConnection {
    fn new(path: String, durability_mode: DbDurabilityMode) -> Self {
        Self {
            path,
            durability_mode,
        }
    }
}

impl r2d2::ManageConnection for PooledConnection {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(DB_BUSY_TIMEOUT_SECS))?;
        if self.path != ":memory:" {
            conn.execute_batch(self.durability_mode.db_pragmas())?;
        } else {
            conn.execute_batch("PRAGMA foreign_keys = ON")?;
        }
        conn.set_prepared_statement_cache_capacity(100);
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.execute_batch("SELECT 1")
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_autocommit()
    }
}

#[derive(Clone)]
pub struct DbPool {
    pub path: String,
    pub encryption_key: Option<Vec<u8>>,
    durability_mode: DbDurabilityMode,
    max_size: u32,
    pub pool: r2d2::Pool<PooledConnection>,
}

impl DbPool {
    fn new(
        path: String,
        encryption_key: Option<Vec<u8>>,
        durability_mode: DbDurabilityMode,
        pool_size: u32,
    ) -> Result<Self, r2d2::Error> {
        let manager = PooledConnection::new(path.clone(), durability_mode);
        let pool_size = normalize_db_pool_size(pool_size);
        let min_idle = pool_size.min(DB_POOL_MIN_IDLE);
        let pool = r2d2::Pool::builder()
            .max_size(pool_size)
            .min_idle(Some(min_idle))
            .idle_timeout(Some(std::time::Duration::from_secs(
                DB_POOL_IDLE_TIMEOUT_SECS,
            )))
            .connection_timeout(std::time::Duration::from_secs(10))
            .test_on_check_out(false)
            .build(manager)?;
        Ok(Self {
            pool,
            path,
            encryption_key,
            durability_mode,
            max_size: pool_size,
        })
    }
}

// ---------------------------------------------------------------------------
// Event Store — core persistence logic
// ---------------------------------------------------------------------------

type RoleRow = (i64, String, i32, bool, bool, bool, bool, bool);
type BanMuteRow = (String, String, String, Option<String>, f64, Option<f64>);

/// SQLite-backed event store with connection pooling and optional encryption.
#[derive(Clone)]
pub struct EventStore {
    pool: DbPool,
    prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
}

impl EventStore {
    pub fn new(
        path: String,
        encryption_key: Option<Vec<u8>>,
        durability_mode: DbDurabilityMode,
        db_pool_size: u32,
        prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
    ) -> Self {
        let pool = DbPool::new(
            path.clone(),
            encryption_key.clone(),
            durability_mode,
            db_pool_size,
        )
        .expect("failed to create database pool");
        let store = Self { pool, prometheus };
        store
            .init()
            .expect("failed to initialise event store; check database path, permissions, and encryption key");
        store
            .verify_encryption_access()
            .expect("failed to verify database encryption key compatibility");
        store.run_startup_checkpoint();
        store
    }

    fn record_db_observation(&self, operation: &str, started: Instant, error: bool) {
        let Some(prometheus) = self.prometheus.as_ref() else {
            return;
        };
        let Ok(metrics) = prometheus.try_lock() else {
            return;
        };

        metrics.record_db_query(operation, started.elapsed());
        if error {
            metrics.record_db_error(operation);
        }

        let state = self.pool.pool.state();
        metrics.update_db_pool_stats(
            (state.connections - state.idle_connections) as usize,
            state.idle_connections as usize,
        );
    }

    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.pool.encryption_key.is_some()
    }

    fn verify_encryption_access(&self) -> ChatifyResult<()> {
        if self.pool.encryption_key.is_none() {
            return Ok(());
        }

        let Some(conn) = self.get_connection() else {
            return Err(ChatifyError::Message(
                "database pool unavailable during encryption verification".to_string(),
            ));
        };

        self.verify_encrypted_sample(
            &conn,
            r#"SELECT payload FROM events WHERE payload LIKE '{"ct":"%"}' LIMIT 1"#,
            "events.payload",
        )?;
        self.verify_encrypted_sample(
            &conn,
            r#"SELECT pw_hash FROM user_credentials WHERE pw_hash LIKE '{"ct":"%"}' LIMIT 1"#,
            "user_credentials.pw_hash",
        )?;
        self.verify_encrypted_sample(
            &conn,
            r#"SELECT secret FROM user_2fa WHERE secret IS NOT NULL AND secret LIKE '{"ct":"%"}' LIMIT 1"#,
            "user_2fa.secret",
        )?;
        self.verify_encrypted_sample(
            &conn,
            r#"SELECT backup_codes FROM user_2fa WHERE backup_codes IS NOT NULL AND backup_codes LIKE '{"ct":"%"}' LIMIT 1"#,
            "user_2fa.backup_codes",
        )?;
        self.verify_encrypted_blob_sample(
            &conn,
            "SELECT chunk_blob FROM media_chunks LIMIT 1",
            "media_chunks.chunk_blob",
        )?;

        Ok(())
    }

    fn verify_encrypted_sample(
        &self,
        conn: &Connection,
        sql: &str,
        field_label: &str,
    ) -> ChatifyResult<()> {
        let sample: Option<String> = match conn.query_row(sql, [], |row| row.get(0)).optional() {
            Ok(sample) => sample,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("no such table") || msg.contains("no such column") {
                    warn!(
                        "encryption verification skipped for {} because schema object is missing",
                        field_label
                    );
                    return Ok(());
                }
                return Err(ChatifyError::Message(format!(
                    "failed to read {} sample for encryption verification: {}",
                    field_label, e
                )));
            }
        };

        if let Some(stored) = sample {
            if self.decrypt_field(&stored).is_none() {
                return Err(ChatifyError::Validation(format!(
                    "database encryption key mismatch: cannot decrypt {}",
                    field_label
                )));
            }
        }

        Ok(())
    }

    fn verify_encrypted_blob_sample(
        &self,
        conn: &Connection,
        sql: &str,
        field_label: &str,
    ) -> ChatifyResult<()> {
        let Some(ref key) = self.pool.encryption_key else {
            return Ok(());
        };

        let sample: Option<Vec<u8>> = match conn.query_row(sql, [], |row| row.get(0)).optional() {
            Ok(sample) => sample,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("no such table") || msg.contains("no such column") {
                    warn!(
                        "encryption verification skipped for {} because schema object is missing",
                        field_label
                    );
                    return Ok(());
                }
                return Err(ChatifyError::Message(format!(
                    "failed to read {} sample for encryption verification: {}",
                    field_label, e
                )));
            }
        };

        if let Some(blob) = sample {
            if !blob.starts_with(MEDIA_CHUNK_ENC_PREFIX) {
                warn!(
                    "legacy plaintext blob encountered while encryption is enabled for {}",
                    field_label
                );
                return Ok(());
            }

            if crypto::dec_bytes(key, &blob[MEDIA_CHUNK_ENC_PREFIX.len()..]).is_err() {
                return Err(ChatifyError::Validation(format!(
                    "database encryption key mismatch: cannot decrypt {}",
                    field_label
                )));
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn health_check(&self) -> bool {
        if let Some(conn) = self.get_connection() {
            conn.query_row("SELECT 1", [], |_| Ok(())).is_ok()
        } else {
            false
        }
    }

    fn init(&self) -> rusqlite::Result<()> {
        let conn = self
            .get_connection()
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        let version = Self::schema_version(&conn)?;
        self.migrate(&conn, version)?;
        Ok(())
    }

    fn run_startup_checkpoint(&self) {
        if self.pool.path == ":memory:" {
            return;
        }
        let Some(conn) = self.get_connection() else {
            return;
        };
        if let Err(err) = conn.execute_batch(self.pool.durability_mode.startup_checkpoint_pragma())
        {
            warn!("startup WAL checkpoint failed: {}", err);
        }
        if let Err(err) = conn.execute_batch("PRAGMA optimize;") {
            warn!("startup PRAGMA optimize failed: {}", err);
        }
    }

    fn get_connection(&self) -> Option<r2d2::PooledConnection<PooledConnection>> {
        self.pool.pool.get().ok()
    }

    #[must_use]
    pub fn configured_pool_size(&self) -> u32 {
        self.pool.max_size
    }

    #[must_use]
    pub fn get_pool_stats(&self) -> PoolStats {
        let state = self.pool.pool.state();
        PoolStats {
            active_connections: (state.connections - state.idle_connections) as usize,
            idle_connections: state.idle_connections as usize,
            total_connections: state.connections as usize,
            wait_count: 0,
            acquisition_count: 0,
            release_count: 0,
        }
    }

    fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
        let value: rusqlite::Result<String> = conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        );
        match value {
            Ok(v) => Ok(v.parse::<i64>().unwrap_or(0)),
            Err(SqlError::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn set_schema_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO schema_meta(key, value)
             VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![version.to_string()],
        )?;
        Ok(())
    }

    fn migrate(&self, conn: &Connection, from_version: i64) -> rusqlite::Result<()> {
        let mut version = from_version;

        if version < 1 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS events (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts          REAL    NOT NULL,
                    event_type  TEXT    NOT NULL,
                    channel     TEXT    NOT NULL,
                    sender      TEXT,
                    target      TEXT,
                    payload     TEXT    NOT NULL,
                    search_text TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_channel_ts
                    ON events(channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_search
                    ON events(search_text);
                ",
            )?;
            version = 1;
            Self::set_schema_version(conn, version)?;
        }

        if version < 2 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS user_2fa (
                    username      TEXT    PRIMARY KEY,
                    enabled       BOOLEAN NOT NULL DEFAULT FALSE,
                    secret        TEXT,
                    backup_codes  TEXT,
                    enabled_at    REAL,
                    last_verified REAL
                );
                ",
            )?;
            version = 2;
            Self::set_schema_version(conn, version)?;
        }

        if version < 3 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS user_credentials (
                    username     TEXT PRIMARY KEY,
                    pw_hash      TEXT NOT NULL,
                    created_at   REAL NOT NULL,
                    updated_at   REAL NOT NULL,
                    login_count  INTEGER NOT NULL DEFAULT 0,
                    last_login   REAL
                );
                ",
            )?;
            version = 3;
            Self::set_schema_version(conn, version)?;
        }

        if version < 4 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS events (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts          REAL    NOT NULL,
                    event_type  TEXT    NOT NULL,
                    channel     TEXT    NOT NULL,
                    sender      TEXT,
                    target      TEXT,
                    payload     TEXT    NOT NULL,
                    search_text TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_channel_ts
                    ON events(channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_search
                    ON events(search_text);
                CREATE INDEX IF NOT EXISTS idx_events_dm_route_ts
                    ON events(event_type, sender, target, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_channel_search_ts
                    ON events(channel, ts DESC, search_text);

                CREATE TRIGGER IF NOT EXISTS trg_events_append_only_update
                BEFORE UPDATE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'events is append-only');
                END;

                CREATE TRIGGER IF NOT EXISTS trg_events_append_only_delete
                BEFORE DELETE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'events is append-only');
                END;
                ",
            )?;
            version = 4;
            Self::set_schema_version(conn, version)?;
        }

        if version < 5 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS roles (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    name        TEXT NOT NULL UNIQUE,
                    level       INTEGER NOT NULL DEFAULT 0,
                    can_kick    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_ban    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_mute    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_manage  BOOLEAN NOT NULL DEFAULT FALSE,
                    can_pin     BOOLEAN NOT NULL DEFAULT FALSE,
                    created_at  REAL NOT NULL
                );

                CREATE TABLE IF NOT EXISTS user_roles (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    role_id     INTEGER NOT NULL,
                    assigned_by TEXT NOT NULL,
                    assigned_at  REAL NOT NULL,
                    UNIQUE(username, channel),
                    FOREIGN KEY (role_id) REFERENCES roles(id)
                );

                CREATE TABLE IF NOT EXISTS bans (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    banned_by   TEXT NOT NULL,
                    reason      TEXT,
                    banned_at   REAL NOT NULL,
                    expires_at  REAL,
                    UNIQUE(username, channel)
                );

                CREATE TABLE IF NOT EXISTS mutes (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    muted_by    TEXT NOT NULL,
                    reason      TEXT,
                    muted_at    REAL NOT NULL,
                    expires_at  REAL,
                    UNIQUE(username, channel)
                );

                CREATE INDEX IF NOT EXISTS idx_user_roles_lookup
                    ON user_roles(username, channel);
                CREATE INDEX IF NOT EXISTS idx_bans_lookup
                    ON bans(username, channel);
                CREATE INDEX IF NOT EXISTS idx_mutes_lookup
                    ON mutes(username, channel);
                ",
            )?;

            let now = chatify::now();
            for (name, level, can_kick, can_ban, can_mute, can_manage, can_pin) in [
                ("admin", 100, true, true, true, true, true),
                ("moderator", 50, true, true, true, false, true),
                ("member", 10, false, false, false, false, false),
                ("readonly", 5, false, false, false, false, false),
                ("guest", 1, false, false, false, false, false),
            ] {
                conn.execute(
                    "INSERT OR IGNORE INTO roles (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![name, level, can_kick, can_ban, can_mute, can_manage, can_pin, now],
                )?;
            }
            version = 5;
            Self::set_schema_version(conn, version)?;
        }

        if version < 6 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS audit_logs (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    action      TEXT NOT NULL,
                    actor       TEXT NOT NULL,
                    target      TEXT,
                    channel     TEXT,
                    reason      TEXT,
                    metadata    TEXT,
                    ts          REAL NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_audit_logs_action
                    ON audit_logs(action, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_actor
                    ON audit_logs(actor, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_target
                    ON audit_logs(target, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_channel
                    ON audit_logs(channel, ts DESC);

                CREATE TABLE IF NOT EXISTS suspicious_activity (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    target_username TEXT NOT NULL,
                    activity_type   TEXT NOT NULL,
                    severity        TEXT NOT NULL DEFAULT 'low',
                    details         TEXT,
                    resolved        BOOLEAN NOT NULL DEFAULT FALSE,
                    resolved_by     TEXT,
                    resolved_at     REAL,
                    ts              REAL NOT NULL,
                    UNIQUE(target_username, activity_type, ts)
                );

                CREATE INDEX IF NOT EXISTS idx_suspicious_activity_lookup
                    ON suspicious_activity(target_username, activity_type, ts DESC);
                ",
            )?;

            conn.execute(
                "ALTER TABLE user_credentials ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0",
                [],
            ).ok();

            conn.execute(
                "ALTER TABLE user_credentials ADD COLUMN locked_until REAL NOT NULL DEFAULT 0",
                [],
            )
            .ok();

            version = 6;
            Self::set_schema_version(conn, version)?;
        }

        if version < 7 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS user_presence_snapshots (
                    username        TEXT PRIMARY KEY,
                    status_payload  TEXT NOT NULL,
                    updated_at      REAL NOT NULL
                );

                CREATE TABLE IF NOT EXISTS user_channel_subscriptions (
                    username      TEXT NOT NULL,
                    channel       TEXT NOT NULL,
                    subscribed_at REAL NOT NULL,
                    last_seen_at  REAL NOT NULL,
                    PRIMARY KEY(username, channel)
                );

                CREATE INDEX IF NOT EXISTS idx_user_channel_subscriptions_user
                    ON user_channel_subscriptions(username, last_seen_at DESC);
                CREATE INDEX IF NOT EXISTS idx_user_channel_subscriptions_channel
                    ON user_channel_subscriptions(channel, last_seen_at DESC);
                ",
            )?;

            version = 7;
            Self::set_schema_version(conn, version)?;
        }

        if version < 8 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS media_objects (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    channel       TEXT NOT NULL,
                    file_id       TEXT NOT NULL,
                    sender        TEXT NOT NULL,
                    filename      TEXT NOT NULL,
                    media_kind    TEXT NOT NULL DEFAULT 'file',
                    mime          TEXT,
                    declared_size INTEGER NOT NULL DEFAULT 0,
                    received_size INTEGER NOT NULL DEFAULT 0,
                    chunk_count   INTEGER NOT NULL DEFAULT 0,
                    completed     BOOLEAN NOT NULL DEFAULT FALSE,
                    created_ts    REAL NOT NULL,
                    completed_ts  REAL,
                    UNIQUE(channel, file_id)
                );

                CREATE INDEX IF NOT EXISTS idx_media_objects_channel_ts
                    ON media_objects(channel, created_ts DESC);
                CREATE INDEX IF NOT EXISTS idx_media_objects_sender_ts
                    ON media_objects(sender, created_ts DESC);
                CREATE INDEX IF NOT EXISTS idx_media_objects_kind_ts
                    ON media_objects(media_kind, created_ts DESC);

                CREATE TABLE IF NOT EXISTS media_chunks (
                    media_id     INTEGER NOT NULL,
                    chunk_index  INTEGER NOT NULL,
                    chunk_blob   BLOB NOT NULL,
                    chunk_size   INTEGER NOT NULL,
                    created_ts   REAL NOT NULL,
                    PRIMARY KEY(media_id, chunk_index),
                    FOREIGN KEY(media_id) REFERENCES media_objects(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_events_channel_event_ts
                    ON events(channel, event_type, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_event_channel_ts
                    ON events(event_type, channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_sender_ts
                    ON events(sender, ts DESC);
                ",
            )?;

            version = 8;
            Self::set_schema_version(conn, version)?;
        }

        if version < 9 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS event_search_terms (
                    event_id INTEGER NOT NULL,
                    term     TEXT NOT NULL,
                    weight   INTEGER NOT NULL DEFAULT 1,
                    PRIMARY KEY(event_id, term),
                    FOREIGN KEY(event_id) REFERENCES events(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_event_search_terms_term_event
                    ON event_search_terms(term, event_id);
                ",
            )?;

            conn.execute(
                "ALTER TABLE media_chunks ADD COLUMN storage_backend TEXT NOT NULL DEFAULT 'sqlite'",
                [],
            )
            .ok();
            conn.execute("ALTER TABLE media_chunks ADD COLUMN storage_path TEXT", [])
                .ok();

            version = 9;
            Self::set_schema_version(conn, version)?;
        }

        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_media_objects_retention
                ON media_objects(completed, completed_ts, created_ts);
            ",
        )
        .ok();
        Self::ensure_builtin_roles(conn)?;

        if version > CURRENT_SCHEMA_VERSION {
            warn!(
                "Database schema version {} is newer than supported version {}",
                version, CURRENT_SCHEMA_VERSION
            );
        }

        Ok(())
    }

    fn ensure_builtin_roles(conn: &Connection) -> rusqlite::Result<()> {
        let roles_table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='roles'",
            [],
            |row| row.get(0),
        )?;
        if roles_table_exists == 0 {
            return Ok(());
        }

        let created_at = chatify::now();
        for (name, level, can_kick, can_ban, can_mute, can_manage, can_pin) in [
            ("admin", 100, true, true, true, true, true),
            ("moderator", 50, true, true, true, false, true),
            ("member", 10, false, false, false, false, false),
            ("readonly", 5, false, false, false, false, false),
            ("guest", 1, false, false, false, false, false),
        ] {
            conn.execute(
                "INSERT OR IGNORE INTO roles
                 (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at],
            )?;
        }
        Ok(())
    }

    fn encrypt_field(&self, plaintext: &str) -> Option<String> {
        if let Some(ref key) = self.pool.encryption_key {
            match crypto::enc_bytes(key, plaintext.as_bytes()) {
                Ok(ct) => Some(format!("{{\"ct\":\"{}\"}}", hex::encode(ct))),
                Err(e) => {
                    warn!("encryption failed; dropping persistence write: {}", e);
                    None
                }
            }
        } else {
            Some(plaintext.to_string())
        }
    }

    fn decrypt_field(&self, stored: &str) -> Option<String> {
        if let Some(ref key) = self.pool.encryption_key {
            let val = match serde_json::from_str::<serde_json::Value>(stored) {
                Ok(v) => v,
                Err(_) => {
                    warn!("legacy plaintext row encountered while encryption is enabled");
                    return Some(stored.to_string());
                }
            };
            let Some(ct_hex) = val.get("ct").and_then(|v| v.as_str()) else {
                warn!("legacy plaintext row encountered while encryption is enabled");
                return Some(stored.to_string());
            };
            let Ok(ct_bytes) = hex::decode(ct_hex) else {
                warn!("encrypted payload has invalid ciphertext encoding; dropping row");
                return None;
            };
            match crypto::dec_bytes(key, &ct_bytes) {
                Ok(pt) => Some(String::from_utf8_lossy(&pt).to_string()),
                Err(e) => {
                    warn!("decryption failed; dropping row: {}", e);
                    None
                }
            }
        } else {
            Some(stored.to_string())
        }
    }

    fn query_events<P>(&self, operation: &str, sql: &str, params: P) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation(operation, started, true);
            return Vec::new();
        };
        let mut stmt = match conn.prepare_cached(sql) {
            Ok(s) => s,
            Err(e) => {
                warn!("event query prepare failed: {}", e);
                self.record_db_observation(operation, started, true);
                return Vec::new();
            }
        };

        let mut rows = match stmt.query(params) {
            Ok(r) => r,
            Err(e) => {
                warn!("event query execute failed: {}", e);
                self.record_db_observation(operation, started, true);
                return Vec::new();
            }
        };

        let mut out = Vec::new();
        let mut had_error = false;
        loop {
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    warn!("event query row iteration failed: {}", e);
                    had_error = true;
                    break;
                }
            };

            let raw = match row.get::<_, String>(0) {
                Ok(v) => v,
                Err(e) => {
                    warn!("event query row decode failed: {}", e);
                    had_error = true;
                    continue;
                }
            };

            let Some(decrypted) = self.decrypt_field(&raw) else {
                continue;
            };

            if let Ok(payload) = serde_json::from_str::<Value>(&decrypted) {
                out.push(payload);
            }
        }

        self.record_db_observation(operation, started, had_error);
        out
    }

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
        let conn = self.get_connection().ok_or(rusqlite::Error::InvalidQuery)?;
        let Some(stored_payload) = self.encrypt_field(payload) else {
            self.record_db_observation("store_event", started, true);
            return Err(rusqlite::Error::InvalidQuery);
        };
        let stored_search_text = match search_text {
            Some(value) => match self.encrypt_field(value) {
                Some(encrypted) => Some(encrypted),
                None => {
                    self.record_db_observation("store_event", started, true);
                    return Err(rusqlite::Error::InvalidQuery);
                }
            },
            None => None,
        };

        conn.execute(
            "INSERT INTO events (event_type, channel, sender, target, payload, search_text, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event_type,
                channel,
                sender,
                target,
                stored_payload,
                stored_search_text,
                ts
            ],
        )?;

        self.record_db_observation("store_event", started, false);
        Ok(())
    }

    /// Serialises `payload` to JSON and persists under `event_type` + `channel`.
    /// This is the higher-level API that most event handlers call (as opposed to
    /// [`store_event`] which takes pre-serialized strings).
    pub fn persist(
        &self,
        event_type: &str,
        channel: &str,
        sender: &str,
        target: Option<&str>,
        payload: &Value,
        search_text: &str,
    ) {
        let payload_str = payload.to_string();
        let _ = self.store_event(
            event_type,
            channel,
            sender,
            target,
            &payload_str,
            Some(search_text),
            chatify::now(),
        );
    }

    pub fn history(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "history",
            "SELECT payload FROM events
             WHERE channel = ?1
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    pub fn history_since(&self, channel: &str, from_ts: f64, limit: usize) -> Vec<Value> {
        self.query_events(
            "history_since",
            "SELECT payload FROM events
                         WHERE channel = ?1 AND ts >= ?2
                         ORDER BY ts DESC
                         LIMIT ?3",
            params![channel, from_ts, limit as i64],
        )
    }

    pub fn dm_history(&self, username: &str, peer: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "dm_history",
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1
             )
             ORDER BY ts DESC
             LIMIT ?3",
            params![username, peer, limit as i64],
        )
    }

    pub fn dm_rewind(&self, username: &str, peer: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "dm_rewind",
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2 AND ts >= ?3
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1 AND ts >= ?3
             )
             ORDER BY ts DESC
             LIMIT ?4",
            params![username, peer, cutoff, limit as i64],
        )
    }

    pub fn dm_history_since(
        &self,
        username: &str,
        peer: &str,
        from_ts: f64,
        limit: usize,
    ) -> Vec<Value> {
        self.query_events(
            "dm_history_since",
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2 AND ts >= ?3
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1 AND ts >= ?3
             )
             ORDER BY ts DESC
             LIMIT ?4",
            params![username, peer, from_ts, limit as i64],
        )
    }

    fn search_encrypted<P>(
        &self,
        sql: &str,
        params: P,
        query_lower: &str,
        limit: usize,
        label: &str,
    ) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let operation = if label == "dm" {
            "search_encrypted_dm"
        } else {
            "search_encrypted_channel"
        };
        if limit == 0 {
            self.record_db_observation(operation, Instant::now(), false);
            return Vec::new();
        }

        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation(operation, started, true);
            return Vec::new();
        };
        let mut stmt = match conn.prepare_cached(sql) {
            Ok(s) => s,
            Err(e) => {
                warn!("{} search query prepare failed: {}", label, e);
                self.record_db_observation(operation, started, true);
                return Vec::new();
            }
        };
        let mut rows = match stmt.query(params) {
            Ok(r) => r,
            Err(e) => {
                warn!("{} search query execute failed: {}", label, e);
                self.record_db_observation(operation, started, true);
                return Vec::new();
            }
        };

        let mut results = Vec::with_capacity(limit.min(64));
        let mut had_error = false;
        let mut scanned_rows = 0usize;
        while results.len() < limit {
            if scanned_rows >= ENCRYPTED_SEARCH_SCAN_CAP {
                warn!(
                    "{} encrypted search scan capped at {} rows",
                    label, ENCRYPTED_SEARCH_SCAN_CAP
                );
                break;
            }
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    warn!("{} search row iteration failed: {}", label, e);
                    had_error = true;
                    break;
                }
            };
            scanned_rows += 1;

            let enc_payload = match row.get::<_, String>(0) {
                Ok(v) => v,
                Err(e) => {
                    warn!("{} search payload decode failed: {}", label, e);
                    had_error = true;
                    continue;
                }
            };
            let enc_search = match row.get::<_, String>(1) {
                Ok(v) => v,
                Err(e) => {
                    warn!("{} search index decode failed: {}", label, e);
                    had_error = true;
                    continue;
                }
            };

            let Some(search_text) = self.decrypt_field(&enc_search) else {
                continue;
            };
            if search_text.contains(query_lower) {
                let Some(decrypted) = self.decrypt_field(&enc_payload) else {
                    continue;
                };
                if let Ok(val) = serde_json::from_str::<Value>(&decrypted) {
                    results.push(val);
                }
            }
        }

        self.record_db_observation(operation, started, had_error);
        results
    }

    fn like_pattern(query: &str) -> String {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{}%", escaped.to_lowercase())
    }

    pub fn search(&self, channel: &str, query: &str, limit: usize) -> Vec<Value> {
        let query_lower = query.to_lowercase();

        if self.pool.encryption_key.is_some() {
            self.search_encrypted(
                "SELECT payload, search_text FROM events
                 WHERE channel = ?1
                 ORDER BY ts DESC",
                params![channel],
                &query_lower,
                limit,
                "channel",
            )
        } else {
            let like = Self::like_pattern(query);
            self.query_events(
                "search_plain",
                "SELECT payload FROM events
                 WHERE channel = ?1 AND search_text LIKE ?2 ESCAPE '\\'
                 ORDER BY ts DESC
                 LIMIT ?3",
                params![channel, like, limit as i64],
            )
        }
    }

    pub fn dm_search(&self, username: &str, peer: &str, query: &str, limit: usize) -> Vec<Value> {
        let query_lower = query.to_lowercase();

        if self.pool.encryption_key.is_some() {
            self.search_encrypted(
                "SELECT payload, search_text FROM (
                     SELECT ts, payload, search_text FROM events
                     WHERE event_type = 'dm' AND sender = ?1 AND target = ?2
                     UNION ALL
                     SELECT ts, payload, search_text FROM events
                     WHERE event_type = 'dm' AND sender = ?2 AND target = ?1
                 )
                 ORDER BY ts DESC",
                params![username, peer],
                &query_lower,
                limit,
                "dm",
            )
        } else {
            let like = Self::like_pattern(query);
            self.query_events(
                "dm_search_plain",
                "SELECT payload FROM (
                     SELECT ts, payload FROM events
                     WHERE event_type = 'dm'
                         AND sender = ?1 AND target = ?2
                         AND search_text LIKE ?3 ESCAPE '\\'
                     UNION ALL
                     SELECT ts, payload FROM events
                     WHERE event_type = 'dm'
                         AND sender = ?2 AND target = ?1
                         AND search_text LIKE ?3 ESCAPE '\\'
                 )
                 ORDER BY ts DESC
                 LIMIT ?4",
                params![username, peer, like, limit as i64],
            )
        }
    }

    pub fn reaction_events(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "reaction_events",
            "SELECT payload FROM events
             WHERE channel = ?1 AND event_type = 'reaction'
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    pub fn rewind(&self, channel: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "rewind",
            "SELECT payload FROM events
             WHERE channel = ?1 AND ts >= ?2
             ORDER BY ts DESC
             LIMIT ?3",
            params![channel, cutoff, limit as i64],
        )
    }

    pub fn load_user_2fa(&self, username: &str) -> Option<chatify::totp::User2FA> {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation("auth_load_user_2fa", started, true);
            return None;
        };
        let row = match conn
            .query_row(
                "SELECT enabled, secret, backup_codes, enabled_at, last_verified
                 FROM user_2fa
                 WHERE username = ?1",
                params![username],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                    ))
                },
            )
            .optional()
        {
            Ok(v) => v,
            Err(_) => {
                self.record_db_observation("auth_load_user_2fa", started, true);
                return None;
            }
        };

        let Some((enabled, secret, backup_codes_json, enabled_at, last_verified)) = row else {
            self.record_db_observation("auth_load_user_2fa", started, false);
            return None;
        };

        let secret = match secret {
            Some(stored) => match self.decrypt_field(&stored) {
                Some(decrypted) => Some(decrypted),
                None => {
                    warn!("failed to decrypt 2fa secret");
                    self.record_db_observation("auth_load_user_2fa", started, true);
                    return None;
                }
            },
            None => None,
        };

        let backup_codes_json = match backup_codes_json {
            Some(stored) => match self.decrypt_field(&stored) {
                Some(decrypted) => Some(decrypted),
                None => {
                    warn!("failed to decrypt 2fa backup codes");
                    self.record_db_observation("auth_load_user_2fa", started, true);
                    return None;
                }
            },
            None => None,
        };

        let backup_codes = backup_codes_json
            .as_deref()
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
            .unwrap_or_default();

        let totp_config = secret.map(|secret| chatify::totp::TotpConfig {
            secret,
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });

        self.record_db_observation("auth_load_user_2fa", started, false);

        Some(chatify::totp::User2FA {
            username: username.to_string(),
            enabled,
            totp_config,
            backup_codes,
            enabled_at,
            last_verified,
        })
    }

    pub fn upsert_user_2fa(&self, user: &chatify::totp::User2FA) {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation("auth_upsert_user_2fa", started, true);
            return;
        };

        let secret = user.totp_config.as_ref().map(|cfg| cfg.secret.clone());
        let backup_codes_json =
            serde_json::to_string(&user.backup_codes).unwrap_or_else(|_| "[]".to_string());

        let secret = match secret {
            Some(plaintext) => match self.encrypt_field(&plaintext) {
                Some(encrypted) => Some(encrypted),
                None => {
                    warn!("2fa upsert skipped because TOTP secret encryption failed");
                    self.record_db_observation("auth_upsert_user_2fa", started, true);
                    return;
                }
            },
            None => None,
        };

        let backup_codes_json = match self.encrypt_field(&backup_codes_json) {
            Some(encrypted) => encrypted,
            None => {
                warn!("2fa upsert skipped because backup code encryption failed");
                self.record_db_observation("auth_upsert_user_2fa", started, true);
                return;
            }
        };

        if let Err(e) = conn.execute(
            "INSERT INTO user_2fa(username, enabled, secret, backup_codes, enabled_at, last_verified)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username) DO UPDATE SET
                 enabled       = excluded.enabled,
                 secret        = excluded.secret,
                 backup_codes  = excluded.backup_codes,
                 enabled_at    = excluded.enabled_at,
                 last_verified = excluded.last_verified",
            params![
                user.username,
                user.enabled,
                secret,
                backup_codes_json,
                user.enabled_at,
                user.last_verified,
            ],
        ) {
            warn!("2fa upsert failed: {}", e);
            self.record_db_observation("auth_upsert_user_2fa", started, true);
            return;
        }

        self.record_db_observation("auth_upsert_user_2fa", started, false);
    }

    pub fn load_pw_hash(&self, username: &str) -> Result<Option<String>, &'static str> {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation("auth_load_pw_hash", started, true);
            return Err("store_unavailable");
        };
        let result = conn
            .query_row(
                "SELECT pw_hash FROM user_credentials WHERE username = ?1",
                params![username],
                |row| row.get::<_, String>(0),
            )
            .optional();

        match result {
            Ok(Some(stored_hash)) => match self.decrypt_field(&stored_hash) {
                Some(hash) => {
                    self.record_db_observation("auth_load_pw_hash", started, false);
                    Ok(Some(hash))
                }
                None => {
                    warn!("credential decrypt failed");
                    self.record_db_observation("auth_load_pw_hash", started, true);
                    Err("store_decrypt_failed")
                }
            },
            Ok(None) => {
                self.record_db_observation("auth_load_pw_hash", started, false);
                Ok(None)
            }
            Err(e) => {
                let code = if let SqlError::SqliteFailure(_, Some(ref msg)) = e {
                    if msg.contains("no such table: user_credentials") {
                        warn!("credential table missing; allowing compatibility auth path");
                        "credentials_table_missing"
                    } else {
                        warn!("credential lookup failed: {}", e);
                        "store_query_failed"
                    }
                } else {
                    warn!("credential lookup failed: {}", e);
                    "store_query_failed"
                };
                self.record_db_observation("auth_load_pw_hash", started, true);
                Err(code)
            }
        }
    }

    pub fn upsert_credentials(&self, username: &str, pw_hash: &str) {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation("auth_upsert_credentials", started, true);
            return;
        };
        let Some(encrypted_pw_hash) = self.encrypt_field(pw_hash) else {
            warn!("credential upsert skipped because encryption failed");
            self.record_db_observation("auth_upsert_credentials", started, true);
            return;
        };
        let ts = now();
        if let Err(e) = conn.execute(
            "INSERT INTO user_credentials(username, pw_hash, created_at, updated_at, login_count, last_login)
             VALUES(?1, ?2, ?3, ?3, 1, ?3)
             ON CONFLICT(username) DO UPDATE SET
                pw_hash     = excluded.pw_hash,
                updated_at  = excluded.updated_at,
                login_count = login_count + 1,
                last_login  = excluded.last_login",
            params![username, encrypted_pw_hash, ts],
        ) {
            warn!("credential upsert failed: {}", e);
            self.record_db_observation("auth_upsert_credentials", started, true);
            return;
        }

        self.record_db_observation("auth_upsert_credentials", started, false);
    }

    pub fn is_user_muted(&self, username: &str, channel: &str) -> Result<bool, &'static str> {
        let conn = self.get_connection().ok_or("db_error")?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mutes WHERE username = ?1 AND channel = ?2",
                params![username, channel],
                |row| row.get(0),
            )
            .map_err(|_| "db_error")?;

        Ok(count > 0)
    }

    pub fn verify_credential(
        &self,
        username: &str,
        submitted_hash: &str,
    ) -> Result<bool, &'static str> {
        match self.load_pw_hash(username) {
            Ok(None) => Err("first_login"),
            Ok(Some(stored)) if stored.starts_with("v2$") => Ok(false),
            Ok(Some(stored)) => Ok(crypto::pw_verify(submitted_hash, &stored)),
            Err("credentials_table_missing") => Err("first_login"),
            Err(e) => Err(e),
        }
    }

    pub fn credential_state(&self, username: &str) -> Result<Option<String>, &'static str> {
        match self.load_pw_hash(username) {
            Ok(Some(stored)) => Ok(Some(stored)),
            Ok(None) => Ok(None),
            Err("credentials_table_missing") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn verify_auth_v2_proof(
        &self,
        username: &str,
        proof: &str,
        client_nonce: &str,
        server_nonce: &str,
    ) -> Result<bool, &'static str> {
        let Some(stored) = self.credential_state(username)? else {
            return Err("first_login");
        };
        let Some(secret) = stored.strip_prefix("v2$") else {
            return Err("legacy_credential");
        };
        let expected = match crypto::auth_proof(secret, username, client_nonce, server_nonce) {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        Ok(crypto::secure_string_eq(proof, &expected))
    }

    pub fn upsert_auth_v2_secret(&self, username: &str, client_secret: &str) {
        self.upsert_credentials(username, &format!("v2${client_secret}"));
    }

    pub fn upsert_presence_snapshot(&self, username: &str, status: &Value) {
        let normalized_status = match validate_status_field(Some(status)) {
            Ok(v) => v,
            Err(e) => {
                warn!("presence snapshot ignored due to invalid status: {}", e);
                return;
            }
        };

        let Some(conn) = self.get_connection() else {
            return;
        };

        let status_json = normalized_status.to_string();
        let Some(encrypted_status) = self.encrypt_field(&status_json) else {
            warn!("presence snapshot upsert skipped due to encryption failure");
            return;
        };
        let ts = now();

        if let Err(e) = conn.execute(
            "INSERT INTO user_presence_snapshots(username, status_payload, updated_at)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(username) DO UPDATE SET
                 status_payload = excluded.status_payload,
                 updated_at = excluded.updated_at",
            params![username, encrypted_status, ts],
        ) {
            warn!("presence snapshot upsert failed: {}", e);
        }
    }

    pub fn load_presence_snapshot(&self, username: &str) -> Option<Value> {
        let conn = self.get_connection()?;
        let stored: Option<String> = conn
            .query_row(
                "SELECT status_payload FROM user_presence_snapshots WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()
            .ok()?;

        let stored = stored?;
        let decrypted = self.decrypt_field(&stored)?;
        let parsed = serde_json::from_str::<Value>(&decrypted).ok()?;
        validate_status_field(Some(&parsed)).ok()
    }

    pub fn upsert_channel_subscription(&self, username: &str, channel: &str) {
        let normalized_channel = safe_ch(channel);
        if normalized_channel.starts_with(DM_CHANNEL_PREFIX) {
            return;
        }

        let Some(conn) = self.get_connection() else {
            return;
        };
        let ts = now();

        if let Err(e) = conn.execute(
            "INSERT INTO user_channel_subscriptions(username, channel, subscribed_at, last_seen_at)
             VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(username, channel) DO UPDATE SET
                 last_seen_at = excluded.last_seen_at",
            params![username, normalized_channel, ts],
        ) {
            warn!("channel subscription upsert failed: {}", e);
        }
    }

    pub fn remove_channel_subscription(&self, username: &str, channel: &str) -> bool {
        let normalized_channel = safe_ch(channel);
        if normalized_channel == "general" || normalized_channel.starts_with(DM_CHANNEL_PREFIX) {
            return false;
        }

        let Some(conn) = self.get_connection() else {
            return false;
        };

        match conn.execute(
            "DELETE FROM user_channel_subscriptions WHERE username = ?1 AND channel = ?2",
            params![username, normalized_channel],
        ) {
            Ok(affected) => affected > 0,
            Err(e) => {
                warn!("channel subscription delete failed: {}", e);
                false
            }
        }
    }

    pub fn list_channel_subscriptions(&self, username: &str) -> Vec<String> {
        let Some(conn) = self.get_connection() else {
            return Vec::new();
        };

        let mut stmt = match conn.prepare_cached(
            "SELECT channel FROM user_channel_subscriptions
             WHERE username = ?1
             ORDER BY last_seen_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                warn!("channel subscription query prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = match stmt.query_map(params![username], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows,
            Err(e) => {
                warn!("channel subscription query failed: {}", e);
                return Vec::new();
            }
        };

        let mut unique = HashSet::new();
        let mut channels = Vec::new();
        for raw in rows.filter_map(|r| r.ok()) {
            let normalized = safe_ch(&raw);
            if normalized.starts_with(DM_CHANNEL_PREFIX) {
                continue;
            }
            if unique.insert(normalized.clone()) {
                channels.push(normalized);
            }
        }

        channels
    }

    #[must_use]
    pub fn get_user_role(&self, username: &str, channel: &str) -> Option<Role> {
        let conn = self.get_connection()?;
        let result: rusqlite::Result<RoleRow> = conn.query_row(
            "SELECT r.id, r.name, r.level, r.can_kick, r.can_ban, r.can_mute, r.can_manage, r.can_pin
             FROM roles r
             JOIN user_roles ur ON r.id = ur.role_id
             WHERE ur.username = ?1 AND ur.channel = ?2",
            params![username, channel],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?,
                row.get::<_, i32>(3)? != 0,
                row.get::<_, i32>(4)? != 0,
                row.get::<_, i32>(5)? != 0,
                row.get::<_, i32>(6)? != 0,
                row.get::<_, i32>(7)? != 0,
            )),
        );

        result.map(Role::from_row).ok()
    }

    #[must_use]
    pub fn get_default_role(&self) -> Option<Role> {
        let conn = self.get_connection()?;
        let result: rusqlite::Result<RoleRow> = conn.query_row(
            "SELECT id, name, level, can_kick, can_ban, can_mute, can_manage, can_pin FROM roles WHERE name = 'member'",
            [],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?,
                row.get::<_, i32>(3)? != 0,
                row.get::<_, i32>(4)? != 0,
                row.get::<_, i32>(5)? != 0,
                row.get::<_, i32>(6)? != 0,
                row.get::<_, i32>(7)? != 0,
            )),
        );

        result.map(Role::from_row).ok()
    }

    pub fn assign_role(
        &self,
        username: &str,
        channel: &str,
        role_name: &str,
        assigned_by: &str,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let role_id = conn
            .query_row(
                "SELECT id FROM roles WHERE name = ?1",
                params![role_name],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| format!("role '{}' not found", role_name))?;

        conn.execute(
            "INSERT INTO user_roles (username, channel, role_id, assigned_by, assigned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(username, channel) DO UPDATE SET role_id = ?3, assigned_by = ?4, assigned_at = ?5",
            params![username, channel, role_id, assigned_by, chatify::now()],
        ).map_err(|e| format!("failed to assign role: {}", e))?;

        Ok(())
    }

    pub fn remove_user_role(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM user_roles WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to remove role: {}", e))?;

        Ok(())
    }

    pub fn list_users(&self, channel: &str, limit: i64) -> Vec<Value> {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut stmt = match conn.prepare(
            "SELECT username, created_at, last_login
             FROM user_credentials
             ORDER BY username ASC
             LIMIT ?1",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<f64>>(2)?,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(|row| row.ok())
            .map(|(username, created_at, last_login)| {
                let role = self
                    .get_user_role(&username, channel)
                    .map(|r| r.name)
                    .unwrap_or_else(|| "member".to_string());
                serde_json::json!({
                    "username": username,
                    "role": role,
                    "channel": channel,
                    "created_at": created_at,
                    "last_login": last_login,
                })
            })
            .collect()
    }

    pub fn ban_user(
        &self,
        username: &str,
        channel: &str,
        banned_by: &str,
        reason: Option<&str>,
        duration_secs: Option<i64>,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let expires_at = duration_secs.map(|secs| chatify::now() + secs as f64);

        conn.execute(
            "INSERT INTO bans (username, channel, banned_by, reason, banned_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username, channel) DO UPDATE SET
                banned_by = ?3, reason = ?4, banned_at = ?5, expires_at = ?6",
            params![
                username,
                channel,
                banned_by,
                reason,
                chatify::now(),
                expires_at
            ],
        )
        .map_err(|e| format!("failed to ban user: {}", e))?;

        Ok(())
    }

    pub fn unban_user(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM bans WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to unban user: {}", e))?;

        Ok(())
    }

    #[must_use]
    pub fn is_banned(&self, username: &str, channel: &str) -> Option<Ban> {
        let conn = self.get_connection()?;

        let result: rusqlite::Result<BanMuteRow> = conn.query_row(
            "SELECT username, channel, banned_by, reason, banned_at, expires_at
             FROM bans WHERE username = ?1 AND channel = ?2",
            params![username, channel],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        );

        match result {
            Ok((username, channel, banned_by, reason, banned_at, expires_at)) => Some(Ban {
                username,
                channel,
                banned_by,
                reason,
                banned_at,
                expires_at,
            }),
            Err(_) => None,
        }
    }

    pub fn mute_user(
        &self,
        username: &str,
        channel: &str,
        muted_by: &str,
        reason: Option<&str>,
        duration_secs: Option<i64>,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let expires_at = duration_secs.map(|secs| chatify::now() + secs as f64);

        conn.execute(
            "INSERT INTO mutes (username, channel, muted_by, reason, muted_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username, channel) DO UPDATE SET
                muted_by = ?3, reason = ?4, muted_at = ?5, expires_at = ?6",
            params![
                username,
                channel,
                muted_by,
                reason,
                chatify::now(),
                expires_at
            ],
        )
        .map_err(|e| format!("failed to mute user: {}", e))?;

        Ok(())
    }

    pub fn unmute_user(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM mutes WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to unmute user: {}", e))?;

        Ok(())
    }

    #[must_use]
    pub fn is_muted(&self, username: &str, channel: &str) -> Option<Mute> {
        let conn = self.get_connection()?;

        let result: rusqlite::Result<BanMuteRow> = conn.query_row(
            "SELECT username, channel, muted_by, reason, muted_at, expires_at
             FROM mutes WHERE username = ?1 AND channel = ?2",
            params![username, channel],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        );

        match result {
            Ok((username, channel, muted_by, reason, muted_at, expires_at)) => Some(Mute {
                username,
                channel,
                muted_by,
                reason,
                muted_at,
                expires_at,
            }),
            Err(_) => None,
        }
    }

    // -------------------------------------------------------------------------
    // Audit Logging
    // -------------------------------------------------------------------------

    pub fn log_audit(
        &self,
        action: &str,
        actor: &str,
        target: Option<&str>,
        channel: Option<&str>,
        reason: Option<&str>,
        metadata: Option<&str>,
    ) {
        if let Some(conn) = self.get_connection() {
            let ts = chatify::now();
            if let Err(e) = conn.execute(
                "INSERT INTO audit_logs (action, actor, target, channel, reason, metadata, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![action, actor, target, channel, reason, metadata, ts],
            ) {
                warn!("audit log insert failed: {}", e);
            }
        }
    }

    pub fn get_audit_logs(
        &self,
        filter: Option<&str>,
        filter_value: Option<&str>,
        limit: i64,
    ) -> Vec<AuditLog> {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return vec![],
        };

        match (filter, filter_value) {
            (Some("user"), Some(user)) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE actor = ?2 OR target = ?2
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit, user], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            (Some("channel"), Some(channel)) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE channel = ?2
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit, channel], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            (Some("channel"), None) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE channel IS NOT NULL
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            _ => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
        }
    }

    // -------------------------------------------------------------------------
    // Account Lockout
    // -------------------------------------------------------------------------

    #[must_use]
    pub fn get_lockout_status(&self, username: &str) -> Option<(i32, f64)> {
        let started = Instant::now();
        let Some(conn) = self.get_connection() else {
            self.record_db_observation("auth_get_lockout_status", started, true);
            return None;
        };
        let result: rusqlite::Result<(i32, f64)> = conn.query_row(
            "SELECT failed_attempts, locked_until FROM user_credentials WHERE username = ?1",
            params![username],
            |row| Ok((row.get::<_, i32>(0)?, row.get::<_, f64>(1)?)),
        );
        match result {
            Ok((failed, locked)) => {
                self.record_db_observation("auth_get_lockout_status", started, false);
                Some((failed, locked))
            }
            Err(_) => {
                self.record_db_observation("auth_get_lockout_status", started, true);
                None
            }
        }
    }

    pub fn record_failed_login(&self, username: &str, max_attempts: i32) -> (bool, i32) {
        let started = Instant::now();
        let mut conn = match self.get_connection() {
            Some(c) => c,
            None => {
                self.record_db_observation("auth_record_failed_login", started, true);
                return (false, 0);
            }
        };

        let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => {
                warn!("failed to start lockout transaction: {}", e);
                self.record_db_observation("auth_record_failed_login", started, true);
                return (false, 0);
            }
        };

        let previous_attempts = match tx
            .query_row(
                "SELECT failed_attempts FROM user_credentials WHERE username = ?1",
                params![username],
                |row| row.get::<_, i32>(0),
            )
            .optional()
        {
            Ok(v) => v.unwrap_or(0),
            Err(e) => {
                warn!("failed to load lockout status: {}", e);
                self.record_db_observation("auth_record_failed_login", started, true);
                return (false, 0);
            }
        };

        let new_attempts = previous_attempts + 1;
        let locked_until = if new_attempts >= max_attempts {
            chatify::now() + 900.0
        } else {
            0.0
        };

        if let Err(e) = tx.execute(
            "UPDATE user_credentials SET failed_attempts = ?1, locked_until = ?2 WHERE username = ?3",
            params![new_attempts, locked_until, username],
        ) {
            warn!("failed to record failed login: {}", e);
            self.record_db_observation("auth_record_failed_login", started, true);
            return (false, new_attempts);
        }

        if let Err(e) = tx.commit() {
            warn!("failed to commit lockout transaction: {}", e);
            self.record_db_observation("auth_record_failed_login", started, true);
            return (false, new_attempts);
        }

        self.record_db_observation("auth_record_failed_login", started, false);

        (locked_until > 0.0, new_attempts)
    }

    pub fn clear_failed_logins(&self, username: &str) {
        let started = Instant::now();
        if let Some(conn) = self.get_connection() {
            if let Err(e) = conn.execute(
                "UPDATE user_credentials SET failed_attempts = 0, locked_until = 0 WHERE username = ?1",
                params![username],
            ) {
                warn!("failed to clear failed logins: {}", e);
                self.record_db_observation("auth_clear_failed_logins", started, true);
                return;
            }
            self.record_db_observation("auth_clear_failed_logins", started, false);
            return;
        }

        self.record_db_observation("auth_clear_failed_logins", started, true);
    }

    pub fn unlock_account(&self, username: &str) -> Result<(), String> {
        let started = Instant::now();
        let conn = self.get_connection().ok_or_else(|| {
            self.record_db_observation("auth_unlock_account", started, true);
            "database connection unavailable"
        })?;

        conn.execute(
            "UPDATE user_credentials SET failed_attempts = 0, locked_until = 0 WHERE username = ?1",
            params![username],
        )
        .map_err(|e| {
            self.record_db_observation("auth_unlock_account", started, true);
            format!("failed to unlock account: {}", e)
        })?;

        self.record_db_observation("auth_unlock_account", started, false);

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Suspicious Activity
    // -------------------------------------------------------------------------

    pub fn log_suspicious_activity(
        &self,
        target: &str,
        activity_type: &str,
        severity: &str,
        details: Option<&str>,
    ) {
        if let Some(conn) = self.get_connection() {
            let ts = chatify::now();
            if let Err(e) = conn.execute(
                "INSERT INTO suspicious_activity (target_username, activity_type, severity, details, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![target, activity_type, severity, details, ts],
            ) {
                warn!("suspicious activity log failed: {}", e);
            }
        }
    }

    pub fn get_recent_activity_count(
        &self,
        username: &str,
        activity_type: &str,
        window_secs: f64,
    ) -> i64 {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return 0,
        };

        let cutoff = chatify::now() - window_secs;
        let result: rusqlite::Result<i64> = conn.query_row(
            "SELECT COUNT(*) FROM suspicious_activity WHERE target_username = ?1 AND activity_type = ?2 AND ts > ?3",
            params![username, activity_type, cutoff],
            |row| row.get(0),
        );

        result.unwrap_or(0)
    }

    pub fn resolve_suspicious_activity(&self, id: i64, resolved_by: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "UPDATE suspicious_activity SET resolved = TRUE, resolved_by = ?1, resolved_at = ?2 WHERE id = ?3",
            params![resolved_by, chatify::now(), id],
        ).map_err(|e| format!("failed to resolve suspicious activity: {}", e))?;

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AuditLog {
    pub action: String,
    pub actor: String,
    pub target: Option<String>,
    pub channel: Option<String>,
    pub reason: Option<String>,
    pub metadata: Option<String>,
    pub ts: f64,
}

fn row_to_audit_log(row: &rusqlite::Row) -> rusqlite::Result<AuditLog> {
    Ok(AuditLog {
        action: row.get(0)?,
        actor: row.get(1)?,
        target: row.get(2)?,
        channel: row.get(3)?,
        reason: row.get(4)?,
        metadata: row.get(5)?,
        ts: row.get(6)?,
    })
}

/// Normalizes a user-supplied channel/room name.
///
/// 1. Trim whitespace.
/// 2. Strip a leading `#` (clients may include it as a UI convention).
/// 3. Keep only ASCII alphanumeric characters, `-`, and `_`.
/// 4. Truncate to 32 characters.
/// 5. Fall back to `"general"` if the result is empty.
pub fn safe_ch(raw: &str) -> String {
    chatify::normalize_channel(raw).unwrap_or_else(|| "general".into())
}

/// Validates the optional `"status"` field value.
pub fn validate_status_field(status: Option<&Value>) -> ChatifyResult<Value> {
    let Some(val) = status else {
        return Ok(serde_json::json!({"text": "Online", "emoji": ""}));
    };

    if !val.is_object() {
        return Err(ChatifyError::Validation(
            "status must be a JSON object".to_string(),
        ));
    }

    if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        if text.len() > MAX_STATUS_TEXT_LEN {
            return Err(ChatifyError::Validation(format!(
                "status text exceeds {} characters",
                MAX_STATUS_TEXT_LEN
            )));
        }
    }

    if let Some(emoji) = val.get("emoji").and_then(|v| v.as_str()) {
        if emoji.len() > MAX_STATUS_EMOJI_LEN {
            return Err(ChatifyError::Validation(format!(
                "status emoji exceeds {} characters",
                MAX_STATUS_EMOJI_LEN
            )));
        }
    }

    if let Some(obj) = val.as_object() {
        for key in obj.keys() {
            if key != "text" && key != "emoji" {
                return Err(ChatifyError::Validation(format!(
                    "unexpected status field: {}",
                    key
                )));
            }
        }
    }

    Ok(val.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn unique_test_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{}-{}.db", name, chatify::fresh_nonce_hex()))
    }

    fn read_raw_field(db_path: &std::path::Path, table: &str, field: &str) -> String {
        let conn = Connection::open(db_path).expect("open sqlite db");
        conn.query_row(
            &format!("SELECT {field} FROM {table} WHERE username = ?1"),
            params!["alice"],
            |row| row.get::<_, String>(0),
        )
        .expect("read raw field")
    }

    #[test]
    fn encrypted_store_roundtrips_credentials_presence_and_2fa() {
        let db_path = unique_test_db_path("chatify-library-store-encrypted");
        let db_path_str = db_path.to_string_lossy().to_string();
        let key = chatify::crypto::new_keypair();
        let store = EventStore::new(db_path_str, Some(key), DbDurabilityMode::Balanced, 8, None);

        let client_hash = chatify::fresh_nonce_hex();
        let server_hash = chatify::crypto::pw_hash(&client_hash);
        store.upsert_credentials("alice", &server_hash);
        assert_eq!(store.verify_credential("alice", &client_hash), Ok(true));

        let status = serde_json::json!({"text":"Away","emoji":"."});
        store.upsert_presence_snapshot("alice", &status);
        assert_eq!(store.load_presence_snapshot("alice"), Some(status));

        let totp_secret = chatify::fresh_nonce_hex();
        let backup_code_hash = chatify::fresh_nonce_hex();
        let mut user_2fa = chatify::totp::User2FA::new("alice".to_string());
        user_2fa.enabled = true;
        user_2fa.totp_config = Some(chatify::totp::TotpConfig {
            secret: totp_secret.clone(),
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });
        user_2fa.backup_codes = vec![backup_code_hash.clone()];
        store.upsert_user_2fa(&user_2fa);
        let loaded_2fa = store.load_user_2fa("alice").expect("2fa row should load");
        assert_eq!(
            loaded_2fa.totp_config.map(|cfg| cfg.secret),
            Some(totp_secret.clone())
        );
        assert_eq!(loaded_2fa.backup_codes, vec![backup_code_hash.clone()]);

        let payload = serde_json::json!({"t":"msg","c":"ciphertext"}).to_string();
        store
            .store_event(
                "msg",
                "general",
                "alice",
                None,
                &payload,
                Some("plaintext index"),
                1.0,
            )
            .expect("store event");
        assert_eq!(
            store.history("general", 10),
            vec![serde_json::json!({"t":"msg","c":"ciphertext"})]
        );

        for (table, field, plaintext) in [
            ("user_credentials", "pw_hash", server_hash.as_str()),
            (
                "user_presence_snapshots",
                "status_payload",
                "{\"text\":\"Away\",\"emoji\":\".\"}",
            ),
            ("user_2fa", "secret", totp_secret.as_str()),
            ("user_2fa", "backup_codes", backup_code_hash.as_str()),
        ] {
            let raw = read_raw_field(&db_path, table, field);
            assert_ne!(raw, plaintext, "{table}.{field} should not store plaintext");
            assert!(
                serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|value| value
                        .get("ct")
                        .and_then(|ct| ct.as_str())
                        .map(str::to_string))
                    .is_some(),
                "{table}.{field} should be stored as an encrypted ct wrapper"
            );
        }

        let conn = Connection::open(&db_path).expect("open sqlite db");
        let raw_payload: String = conn
            .query_row("SELECT payload FROM events LIMIT 1", [], |row| row.get(0))
            .expect("read raw event payload");
        assert_ne!(raw_payload, payload);

        drop(store);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    #[test]
    fn lockout_tracking_roundtrips() {
        let db_path = unique_test_db_path("chatify-library-lockout");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Unknown user returns None
        assert!(store.get_lockout_status("alice").is_none());

        // Register the user first so lockout table exists
        let hash = chatify::crypto::pw_hash("good_password");
        store.upsert_credentials("alice", &hash);

        // Initial status after registration but before failures
        assert_eq!(store.get_lockout_status("alice"), Some((0, 0.0)));

        // 4 failed attempts, not locked yet
        for i in 1..=4 {
            let (locked, attempts) = store.record_failed_login("alice", 5);
            assert!(!locked, "should not lock at attempt {}", i);
            assert_eq!(attempts, i);
        }

        // 5th attempt triggers lockout
        let (locked, attempts) = store.record_failed_login("alice", 5);
        assert!(locked, "5th attempt should lock account");
        assert_eq!(attempts, 5);
        let (_, locked_until) = store.get_lockout_status("alice").unwrap();
        assert!(locked_until > 0.0, "lockout should have a future timestamp");

        // Clear failed logins
        store.clear_failed_logins("alice");
        let (failed, locked_until) = store.get_lockout_status("alice").unwrap();
        assert_eq!(failed, 0);
        assert_eq!(locked_until, 0.0);

        // Lock again then unlock
        store.record_failed_login("alice", 2); // 1st attempt, not locked
        store.record_failed_login("alice", 2); // 2nd attempt, locked
        assert!(
            store.get_lockout_status("alice").unwrap().1 > 0.0,
            "account should be locked after 2 failed attempts with max=2"
        );
        store
            .unlock_account("alice")
            .expect("unlock should succeed");
        let (failed, locked_until) = store.get_lockout_status("alice").unwrap();
        assert_eq!(failed, 0);
        assert_eq!(locked_until, 0.0);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn suspicious_activity_logging() {
        let db_path = unique_test_db_path("chatify-library-suspicious");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Log some activity
        store.log_suspicious_activity("mallory", "spam", "low", Some("rapid messages"));
        store.log_suspicious_activity("mallory", "spam", "low", Some("rapid messages"));

        let count = store.get_recent_activity_count("mallory", "spam", 60.0);
        assert_eq!(count, 2);

        // Different user should have 0 count
        let count = store.get_recent_activity_count("alice", "spam", 60.0);
        assert_eq!(count, 0);

        // Resolve first activity
        store
            .resolve_suspicious_activity(1, "admin")
            .expect("resolve should succeed");

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn channel_roles_roundtrip() {
        let db_path = unique_test_db_path("chatify-library-roles");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // No role yet
        assert!(store.get_user_role("alice", "general").is_none());

        // Default role should exist (builtin) and have send permission
        let default = store.get_default_role();
        assert!(default.is_some(), "default role should exist");
        assert!(default.unwrap().permissions.contains(RolePermissions::SEND));

        // Assign a role
        store
            .assign_role("alice", "general", "moderator", "admin")
            .expect("assign role should succeed");

        let role = store.get_user_role("alice", "general");
        assert!(role.is_some(), "role should be assigned");
        let role = role.unwrap();
        assert_eq!(role.name, "moderator");
        assert!(role.can_kick());

        // Remove role
        store
            .remove_user_role("alice", "general")
            .expect("remove role should succeed");
        assert!(store.get_user_role("alice", "general").is_none());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn ban_mute_lifecycle() {
        let db_path = unique_test_db_path("chatify-library-ban-mute");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Ban with no expiry (permanent — pass None for duration)
        store
            .ban_user("mallory", "general", "admin", Some("spamming"), None)
            .expect("ban should succeed");
        let ban = store.is_banned("mallory", "general");
        assert!(ban.is_some(), "should be banned");
        assert!(ban.unwrap().is_active(), "permanent ban should be active");

        // Unban
        store
            .unban_user("mallory", "general")
            .expect("unban should succeed");
        assert!(store.is_banned("mallory", "general").is_none());

        // Ban with expiry
        store
            .ban_user("mallory", "general", "admin", Some("temp"), Some(3600))
            .expect("ban should succeed");
        let ban = store.is_banned("mallory", "general");
        assert!(ban.is_some());

        // Mute (permanent — pass None for duration)
        store
            .mute_user("mallory", "general", "admin", Some("loud"), None)
            .expect("mute should succeed");
        let mute = store.is_muted("mallory", "general");
        assert!(mute.is_some(), "should be muted");
        assert!(mute.unwrap().is_active(), "permanent mute should be active");

        // Unmute
        store
            .unmute_user("mallory", "general")
            .expect("unmute should succeed");
        assert!(store.is_muted("mallory", "general").is_none());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn channel_subscriptions_roundtrip() {
        let db_path = unique_test_db_path("chatify-library-subscriptions");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Subscribe to channels
        assert!(store.list_channel_subscriptions("alice").is_empty());
        store.upsert_channel_subscription("alice", "music");
        store.upsert_channel_subscription("alice", "random");
        let subs = store.list_channel_subscriptions("alice");
        assert_eq!(subs.len(), 2);
        assert!(subs.contains(&"music".to_string()));

        // Unsubscribe (general and DM channels cannot be removed — use music)
        assert!(store.remove_channel_subscription("alice", "music"));
        let subs = store.list_channel_subscriptions("alice");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], "random");

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn persist_and_reaction_events() {
        let db_path = unique_test_db_path("chatify-library-persist-reaction");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Use persist to write a message
        let msg = serde_json::json!({"t":"msg","c":"hello"});
        store.persist("msg", "general", "alice", None, &msg, "hello");

        // Verify via history
        let hist = store.history("general", 10);
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].get("c").and_then(|v| v.as_str()), Some("hello"));

        // Write a reaction
        let reaction = serde_json::json!({"t":"reaction","emoji":"👍"});
        store.persist("reaction", "general", "bob", None, &reaction, "👍");

        let reactions = store.reaction_events("general", 10);
        assert_eq!(reactions.len(), 1);
        assert_eq!(
            reactions[0].get("emoji").and_then(|v| v.as_str()),
            Some("👍")
        );

        // get_pool_stats returns sane values
        let stats = store.get_pool_stats();
        assert!(
            stats.total_connections >= 1,
            "at least 1 connection in pool"
        );
        assert_eq!(store.configured_pool_size(), 8);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn auth_v2_and_credential_state() {
        let db_path = unique_test_db_path("chatify-library-auth-v2");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // credential_state for unknown user returns Ok(None)
        assert!(store.credential_state("alice").unwrap().is_none());

        // Register with legacy
        let pw = chatify::fresh_nonce_hex();
        let hash = chatify::crypto::pw_hash(&pw);
        store.upsert_credentials("alice", &hash);

        // credential_state should be Some (legacy hash, not v2$)
        let state = store.credential_state("alice").unwrap();
        assert!(state.is_some(), "legacy credential should be present");
        assert!(!state.as_deref().unwrap().starts_with("v2$"));

        // Verify credential
        assert_eq!(store.verify_credential("alice", &pw), Ok(true));
        assert_eq!(store.verify_credential("alice", "wrong"), Ok(false));

        // Migrate to v2
        let server_nonce = chatify::fresh_nonce_hex();
        let client_secret = chatify::fresh_nonce_hex();
        store.upsert_auth_v2_secret("alice", &client_secret);

        // credential_state should now be Some starting with v2$
        let state = store.credential_state("alice").unwrap();
        assert!(state.unwrap().starts_with("v2$"));

        // Verify v2 proof
        let proof = chatify::crypto::auth_proof(&client_secret, "alice", "cn", &server_nonce)
            .expect("proof should generate");
        assert!(store
            .verify_auth_v2_proof("alice", &proof, "cn", &server_nonce)
            .unwrap());

        // Wrong proof should fail
        assert!(!store
            .verify_auth_v2_proof("alice", "badproof", "cn", &server_nonce)
            .unwrap());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn audit_log_roundtrip() {
        let db_path = unique_test_db_path("chatify-library-audit");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // Log some audit entries
        store.log_audit(
            "kick",
            "admin",
            Some("mallory"),
            Some("general"),
            Some("spamming"),
            None,
        );
        store.log_audit(
            "ban",
            "admin",
            Some("mallory"),
            Some("general"),
            Some("evasion"),
            None,
        );

        // Query all (no filter)
        let logs = store.get_audit_logs(None, None, 10);
        assert_eq!(logs.len(), 2, "should have 2 audit entries");

        // Filter by user (matches actor OR target)
        let logs = store.get_audit_logs(Some("user"), Some("mallory"), 10);
        assert_eq!(logs.len(), 2, "mallory referenced in both entries");

        // Filter by channel
        let logs = store.get_audit_logs(Some("channel"), Some("general"), 10);
        assert_eq!(logs.len(), 2, "both entries in general channel");

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn is_user_muted_returns_false_when_not_muted() {
        let db_path = unique_test_db_path("chatify-library-is-muted");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        // No mute record should return Ok(false)
        let result = store.is_user_muted("alice", "general");
        assert_eq!(result, Ok(false));

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn list_users_returns_empty_for_empty_channel() {
        let db_path = unique_test_db_path("chatify-library-list-users");
        let db_path_str = db_path.to_string_lossy().to_string();
        let store = EventStore::new(db_path_str, None, DbDurabilityMode::Balanced, 8, None);

        let users = store.list_users("general", 10);
        assert!(users.is_empty());

        let _ = std::fs::remove_file(&db_path);
    }
}
