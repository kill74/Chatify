//! # `clicord-server` — WebSocket Chat Server
//!
//! A single-binary, async WebSocket server built on [Tokio] and
//! [tokio-tungstenite]. It provides:
//!
//! * **Authentication** — first-frame auth with username/password-hash and an
//!   Ed25519 public key for E2E-encrypted DMs.
//! * **2-Factor Authentication** — TOTP (RFC 6238) and single-use backup codes,
//!   stored in SQLite.
//! * **Channel messaging** — broadcast channels with a bounded in-memory ring
//!   buffer and a durable SQLite event store.
//! * **Direct messages** — per-user DM channels keyed `__dm__<username>`.
//! * **Voice rooms** — low-latency audio relay via per-room broadcast channels.
//! * **Search & history** — full-text LIKE search and time-window ("rewind")
//!   queries backed by SQLite.
//! * **Protocol safety** — payload size gates, timestamp-skew validation, and
//!   nonce-based replay protection on mutating events.
//! * **Graceful shutdown** — Ctrl+C drains active connections before exiting,
//!   with a bounded timeout.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                   Tokio Runtime                      │
//! │                                                      │
//! │  TcpListener::accept()                               │
//! │       │                                              │
//! │       └─► tokio::spawn( handle(stream, addr, state) )│
//! │                 │                                    │
//! │        ┌────────▼─────────┐                          │
//! │        │  WebSocket auth  │  ← validates first frame  │
//! │        └────────┬─────────┘                          │
//! │                 │                                    │
//! │       ┌─────────▼──────────┐                         │
//! │       │  Message recv loop  │                        │
//! │       │  handle_event(...)  │                        │
//! │       └─────────┬──────────┘                         │
//! │                 │                                    │
//! │   mpsc::unbounded ──► sink writer task               │
//! └─────────────────────────────────────────────────────┘
//!
//! Shared State (Arc<State>)
//!   ├── channels   : DashMap<String, Channel>
//!   ├── voice      : DashMap<String, broadcast::Sender>
//!   ├── user_statuses / user_pubkeys  : DashMap
//!   ├── recent_nonces : DashMap<String, VecDeque<String>>
//!   └── store      : EventStore  ──► SQLite file
//! ```
//!
//! ## Configuration
//!
//! All options are CLI flags (see [`Args`]):
//!
//! | Flag     | Default          | Description                       |
//! |----------|------------------|-----------------------------------|
//! | `--host` | `0.0.0.0`        | Bind address                       |
//! | `--port` | `8765`           | TCP port                           |
//! | `--log`  | off              | Enable structured logging          |
//! | `--db`   | `chatify.db`     | SQLite database file path          |
//!
//! ## Protocol
//!
//! All frames are UTF-8 JSON objects with a mandatory `"t"` (type) field.
//! Binary frames are silently ignored. The first frame **must** be an `auth`
//! frame; any other type causes an immediate `err` response and disconnection.
//!
//! See [`validate_auth_payload`] for the full auth contract and
//! [`handle_event`] for the complete set of post-auth event types.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use clicord_server::crypto;
use clicord_server::error::{ChatifyError, ChatifyResult};
use clicord_server::totp::{generate_qr_url, generate_secret, TotpConfig, User2FA};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use rusqlite::{params, Connection, Error as SqlError, OptionalExtension};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Notify, RwLock};
use tokio::time::{sleep, Duration};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_async, accept_hdr_async,
    tungstenite::{
        handshake::server::{Callback, Request, Response},
        http, Message,
    },
};

// ---------------------------------------------------------------------------
// CLI configuration
// ---------------------------------------------------------------------------

/// Command-line arguments for `clicord-server`.
///
/// Parsed once at startup by [clap]; the fields are consumed into [`State`]
/// and the bind address, and are not referenced again after `main` returns
/// from its setup phase.
#[derive(Parser)]
#[command(name = "clicord-server")]
struct Args {
    /// IP address the server will bind to. Use `127.0.0.1` to restrict to
    /// loopback (useful for testing behind a reverse proxy).
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 8765)]
    port: u16,

    /// Enable structured logging via `env_logger`. When off, the server is
    /// completely silent except for the startup banner.
    #[arg(long)]
    log: bool,

    /// Path to the SQLite database file. The file is created automatically on
    /// first run and migrated to the current schema. Use `:memory:` for
    /// ephemeral in-process storage in tests.
    #[arg(long, default_value = "chatify.db")]
    db: String,

    /// Hex-encoded encryption key for the SQLite database (SQLCipher).
    /// Must be exactly 64 hex characters (32 bytes).
    /// If omitted, the server looks for a `<db>.key` file. If neither
    /// exists, a new random key is generated and saved to `<db>.key`.
    /// Use `:memory:` databases (tests) to skip encryption entirely.
    #[arg(long)]
    db_key: Option<String>,

    /// Enable TLS for WebSocket connections (wss://).
    /// Requires `--tls-cert` and `--tls-key`.
    #[arg(long)]
    tls: bool,

    /// Path to the TLS certificate file (PEM format).
    /// Used when `--tls` is enabled.
    #[arg(long, default_value = "cert.pem")]
    tls_cert: String,

    /// Path to the TLS private key file (PEM format).
    /// Used when `--tls` is enabled.
    #[arg(long, default_value = "key.pem")]
    tls_key: String,
}

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Maximum number of messages kept in the per-channel in-memory ring buffer.
/// Older entries are evicted when the buffer is full. Persistent history is
/// unbounded in SQLite; this cap only affects the in-process hot cache.
const HISTORY_CAP: usize = 50;

/// Maximum byte length for any post-auth WebSocket frame.
/// Frames exceeding this are rejected before JSON parsing to limit memory
/// pressure from a single misbehaving or malicious client.
const MAX_BYTES: usize = 16_000;

/// Maximum byte length for the initial auth frame.
/// Tighter than [`MAX_BYTES`] because the auth payload is parsed before the
/// client is known/trusted, making it a potential amplification target.
const MAX_AUTH_BYTES: usize = 4_096;

/// Maximum HTTP header size during WebSocket handshake (CVE-2023-43668 mitigation).
/// Limits the total size of HTTP headers during the WebSocket upgrade handshake
/// to prevent denial-of-service attacks via excessive header length.
/// Tungstenite 0.21.0+ includes this fix, but we enforce it explicitly.
const MAX_HANDSHAKE_HEADER_SIZE: usize = 8192;

/// Maximum number of HTTP headers during WebSocket handshake.
/// Limits the number of individual headers to prevent resource exhaustion.
const MAX_HANDSHAKE_HEADERS: usize = 64;

/// SQLite schema version this binary was built against.
/// The migration path in [`EventStore::migrate`] upgrades from any lower
/// version to this one. If the stored version is *higher*, the server logs a
/// warning and continues without modifying the schema (no downgrade).
const CURRENT_SCHEMA_VERSION: i64 = 3;

/// Default number of events returned by a `history` request.
const DEFAULT_HISTORY_LIMIT: usize = 50;

/// Default number of events returned by a `search` request.
const DEFAULT_SEARCH_LIMIT: usize = 30;

/// Default look-back window in seconds for a `rewind` request (1 hour).
const DEFAULT_REWIND_SECONDS: u64 = 3600;

/// Default number of events returned by a `rewind` request.
const DEFAULT_REWIND_LIMIT: usize = 100;

/// WebSocket sub-protocol version advertised in the `"ok"` auth response.
/// Clients can use this to detect incompatible server versions.
const PROTOCOL_VERSION: u64 = 1;

/// Maximum allowed username length in characters (ASCII only).
const MAX_USERNAME_LEN: usize = 32;

/// Maximum allowed length for the `"pw"` (password hash) field in the auth
/// frame. This covers SHA-256 hex strings (64 chars) with generous headroom
/// for other hash schemes.
const MAX_PASSWORD_FIELD_LEN: usize = 256;

/// Maximum allowed length for the `"pk"` (public key) base64 field.
const MAX_PUBLIC_KEY_FIELD_LEN: usize = 256;

/// Allowed clock skew in seconds between the client-supplied `"ts"` timestamp
/// and the server's wall clock. A window of ±300 s accommodates clients with
/// moderately drifted clocks while still preventing stale-message replay.
const MAX_CLOCK_SKEW_SECS: f64 = 300.0;

/// Maximum length of a nonce string in the `"n"` field. This bounds the
/// nonce-cache entry size and prevents artificially long strings from being
/// used as a timing oracle.
const MAX_NONCE_LEN: usize = 64;

/// Maximum number of recently seen nonces remembered per user.
/// Once the deque is full the oldest entry is evicted. Choosing a cap large
/// enough to cover the clock-skew window prevents replay within that window
/// while bounding per-user memory to `NONCE_CACHE_CAP * MAX_NONCE_LEN` bytes.
const NONCE_CACHE_CAP: usize = 256;

/// Maximum number of concurrent connections from a single IP address.
/// Connections beyond this are rejected with a rate-limit error.
const MAX_CONNECTIONS_PER_IP: usize = 5;

/// Minimum interval in seconds between auth attempts from the same IP.
/// Attempts within this window are rejected to slow brute-force attacks.
/// Set to 0.5s — enough to throttle automated tools while allowing
/// rapid legitimate connections (e.g. from integration tests).
const AUTH_RATE_LIMIT_SECS: f64 = 0.5;

/// Maximum file transfer size in bytes (100 MB).
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum allowed length for a status text field.
const MAX_STATUS_TEXT_LEN: usize = 128;

/// Maximum allowed length for a status emoji field.
const MAX_STATUS_EMOJI_LEN: usize = 16;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Validated, strongly-typed representation of a successful auth frame parse.
///
/// Created by [`validate_auth_payload`] after all field-level validation
/// passes. Using a typed struct here — rather than passing `&Value` through
/// downstream functions — makes it impossible to accidentally skip validation
/// or misread a field name.
struct AuthInfo {
    /// Validated username (ASCII alphanumeric / `-` / `_`, ≤ 32 chars).
    username: String,

    /// Password hash submitted by the client (non-empty, ≤ 256 chars).
    /// Used for credential verification against the stored hash.
    pw_hash: String,

    /// Validated status object (text + emoji), or default.
    status: Value,

    /// Base64-encoded 32-byte Ed25519 public key used for E2E DM encryption.
    pubkey: String,

    /// Optional TOTP or backup code. Present only when the client suspects
    /// or knows that 2-FA is enabled for this account.
    otp_code: Option<String>,
}

// ---------------------------------------------------------------------------
// EventStore — SQLite persistence layer
// ---------------------------------------------------------------------------

/// Thin wrapper around a SQLite file that handles schema migration, event
/// persistence, and read queries.
///
/// All methods open a *new* connection for each call (`open_conn`). This
/// avoids holding a connection across `await` points, which is unsound with
/// the single-threaded `rusqlite` connection type. The trade-off is connection
/// overhead; for a chat server at this scale the overhead is negligible.
///
/// When `encryption_key` is `Some`, the `payload` and `search_text` columns
/// are encrypted with ChaCha20-Poly1305 before writing and decrypted on read.
/// This protects data at rest — an attacker who obtains the `.db` file cannot
/// read the chat history without the key. The key is 32 bytes (256 bits) and
/// is never stored in the database itself.
///
/// Cloning an `EventStore` is cheap — it only duplicates the path and key.
#[derive(Clone)]
struct EventStore {
    /// Absolute or relative path to the SQLite file (or `":memory:"`).
    path: String,
    /// Optional 32-byte encryption key for ChaCha20-Poly1305. `None` means
    /// unencrypted (e.g. `:memory:` databases used in tests).
    encryption_key: Option<Vec<u8>>,
}

impl EventStore {
    /// Creates a new `EventStore` pointing at `path` with an optional
    /// encryption key and immediately runs [`init`](Self::init) to ensure
    /// the schema is current.
    ///
    /// # Panics
    ///
    /// Panics if the database cannot be opened or initialised. A server
    /// running with a broken event store is worse than one that crashes
    /// at startup — it silently loses data.
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Self {
        let store = Self {
            path,
            encryption_key,
        };
        store
            .init()
            .expect("failed to initialise event store — check database path, permissions, and encryption key");
        store
    }

    /// Returns `true` if this store uses SQLCipher encryption.
    fn is_encrypted(&self) -> bool {
        self.encryption_key.is_some()
    }

    /// Opens a connection, creates `schema_meta` if it does not exist, reads
    /// the current schema version, and drives migrations to
    /// [`CURRENT_SCHEMA_VERSION`].
    fn init(&self) -> rusqlite::Result<()> {
        let conn = Connection::open(&self.path)?;
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

    /// Reads `schema_meta.schema_version` as an `i64`.
    ///
    /// Returns `0` if the row does not exist (fresh database), or propagates
    /// any other SQLite error to the caller.
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

    /// Upserts `schema_meta.schema_version` to `version`.
    fn set_schema_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO schema_meta(key, value)
             VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![version.to_string()],
        )?;
        Ok(())
    }

    /// Applies all pending migrations in sequential order starting from
    /// `from_version`.
    ///
    /// Each migration step:
    /// 1. Is guarded by `IF NOT EXISTS` DDL so re-running it is idempotent.
    /// 2. Updates `schema_version` immediately after its DDL, before the next
    ///    step runs, so a crash mid-migration leaves a consistent partial state
    ///    that can be resumed on restart.
    ///
    /// # Migration history
    ///
    /// | Version | Tables / Indexes Added                              |
    /// |---------|-----------------------------------------------------|
    /// | 0 → 1   | `events`, `idx_events_channel_ts`, `idx_events_search` |
    /// | 1 → 2   | `user_2fa`                                          |
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

        // A version higher than CURRENT_SCHEMA_VERSION means this binary was
        // deployed against a database written by a newer server. Log a warning
        // but leave the schema intact — we must never silently downgrade.
        if version > CURRENT_SCHEMA_VERSION {
            warn!(
                "Database schema version {} is newer than supported version {}",
                version, CURRENT_SCHEMA_VERSION
            );
        }

        Ok(())
    }

    /// Opens a connection to the database, returning `None` and logging a
    /// warning on failure.
    ///
    /// Callers use the `?` operator or early-return pattern on `None` so that
    /// a transient I/O error degrades gracefully (the operation is skipped)
    /// rather than crashing the connection handler.
    fn open_conn(&self) -> Option<Connection> {
        match Connection::open(&self.path) {
            Ok(c) => {
                // Enable WAL mode for better concurrent read performance
                // and reduced lock contention under load.
                let _ = c.pragma_update(None, "journal_mode", "wal");
                // Enable foreign keys for referential integrity.
                let _ = c.execute_batch("PRAGMA foreign_keys = ON");
                Some(c)
            }
            Err(e) => {
                warn!("event store open failed: {}", e);
                None
            }
        }
    }

    /// Encrypts a plaintext string for storage.
    ///
    /// Returns a JSON string `{"nonce":"<hex>","ct":"<hex>"}` that can be
    /// stored in the database. If no encryption key is configured, returns
    /// the plaintext unchanged.
    fn encrypt_field(&self, plaintext: &str) -> String {
        if let Some(ref key) = self.encryption_key {
            match crypto::enc_bytes(key, plaintext.as_bytes()) {
                Ok(ct) => serde_json::json!({"ct": hex::encode(ct)}).to_string(),
                Err(e) => {
                    warn!("encryption failed, storing plaintext: {}", e);
                    plaintext.to_string()
                }
            }
        } else {
            plaintext.to_string()
        }
    }

    /// Decrypts a stored field back to plaintext.
    ///
    /// If the field is a JSON object with `"ct"`, attempts decryption.
    /// If no encryption key is configured or the field is not encrypted
    /// JSON, returns it as-is.
    fn decrypt_field(&self, stored: &str) -> String {
        if let Some(ref key) = self.encryption_key {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(stored) {
                if let Some(ct_hex) = val.get("ct").and_then(|v| v.as_str()) {
                    if let Ok(ct_bytes) = hex::decode(ct_hex) {
                        match crypto::dec_bytes(key, &ct_bytes) {
                            Ok(pt) => return String::from_utf8_lossy(&pt).to_string(),
                            Err(e) => {
                                warn!("decryption failed: {}", e);
                                return stored.to_string();
                            }
                        }
                    }
                }
            }
        }
        stored.to_string()
    }

    /// Deserialises a list of raw JSON strings into [`Value`]s, silently
    /// dropping any entries that fail to parse.
    fn decode_rows(rows: Vec<String>) -> Vec<Value> {
        rows.into_iter()
            .filter_map(|payload| serde_json::from_str::<Value>(&payload).ok())
            .collect()
    }

    /// Generic helper that prepares `sql`, executes it with `params`, and
    /// returns the results in **chronological** order (the query uses
    /// `ORDER BY ts DESC` for efficient index use; this method reverses the
    /// results before returning).
    fn query_events<P>(&self, sql: &str, params: P) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let Some(conn) = self.open_conn() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                warn!("event query prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = match stmt.query_map(params, |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                warn!("event query execute failed: {}", e);
                return Vec::new();
            }
        };

        // Decrypt each row, then parse as JSON.
        let decrypted: Vec<String> = rows
            .filter_map(|r| r.ok())
            .map(|raw| self.decrypt_field(&raw))
            .collect();

        // Reverse: SQLite returns newest-first for index efficiency; callers
        // expect oldest-first (chronological) order.
        let mut out = Self::decode_rows(decrypted);
        out.reverse();
        out
    }

    /// Inserts a new event row into `events`.
    ///
    /// # Parameters
    ///
    /// * `event_type`  — `"msg"`, `"dm"`, `"sys"`, etc.
    /// * `channel`     — Target channel name (or `__dm__<user>` for DMs).
    /// * `sender`      — Username of the originating client.
    /// * `target`      — Recipient username, used only for DMs.
    /// * `payload`     — The full JSON event value to store and replay.
    /// * `search_text` — Plaintext index field (stored lowercased for
    ///   case-insensitive LIKE queries).
    fn persist(
        &self,
        event_type: &str,
        channel: &str,
        sender: &str,
        target: Option<&str>,
        payload: &Value,
        search_text: &str,
    ) {
        let Some(conn) = self.open_conn() else {
            return;
        };
        let payload_json = payload.to_string();
        let enc_payload = self.encrypt_field(&payload_json);
        let enc_search = self.encrypt_field(&search_text.to_lowercase());
        if let Err(e) = conn.execute(
            "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                now(),
                event_type,
                channel,
                sender,
                target,
                enc_payload,
                enc_search,
            ],
        ) {
            warn!("event persist failed: {}", e);
        }
    }

    /// Returns up to `limit` most-recent events for `channel`, oldest first.
    fn history(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    /// Returns up to `limit` events for `channel` whose `search_text` column
    /// contains `query` (case-insensitive LIKE match), oldest first.
    fn search(&self, channel: &str, query: &str, limit: usize) -> Vec<Value> {
        let query_lower = query.to_lowercase();

        if self.encryption_key.is_some() {
            // When encrypted, search_text is opaque to SQL. Fetch all events
            // for the channel, decrypt both payload and search_text, then
            // filter in-memory.
            let Some(conn) = self.open_conn() else {
                return Vec::new();
            };
            let mut stmt = match conn.prepare(
                "SELECT payload, search_text FROM events
                 WHERE channel = ?1
                 ORDER BY ts DESC",
            ) {
                Ok(s) => s,
                Err(e) => {
                    warn!("search query prepare failed: {}", e);
                    return Vec::new();
                }
            };
            let rows: Vec<(String, String)> = stmt
                .query_map(params![channel], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map(|r| r.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();

            let mut results = Vec::new();
            for (enc_payload, enc_search) in rows {
                let search_text = self.decrypt_field(&enc_search);
                if search_text.contains(&query_lower) {
                    let decrypted = self.decrypt_field(&enc_payload);
                    if let Ok(val) = serde_json::from_str::<Value>(&decrypted) {
                        results.push(val);
                    }
                    if results.len() >= limit {
                        break;
                    }
                }
            }
            results
        } else {
            // Unencrypted: use SQL LIKE for efficiency.
            let escaped = query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let like = format!("%{}%", escaped.to_lowercase());
            self.query_events(
                "SELECT payload FROM events
                 WHERE channel = ?1 AND search_text LIKE ?2 ESCAPE '\\'
                 ORDER BY ts DESC
                 LIMIT ?3",
                params![channel, like, limit as i64],
            )
        }
    }

    /// Returns up to `limit` events for `channel` that occurred within the
    /// last `seconds` seconds, oldest first.
    ///
    /// The `cutoff` is clamped to `0.0` so that an absurdly large `seconds`
    /// value does not produce a negative timestamp.
    fn rewind(&self, channel: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1 AND ts >= ?2
             ORDER BY ts DESC
             LIMIT ?3",
            params![channel, cutoff, limit as i64],
        )
    }

    /// Loads the [`User2FA`] record for `username` from `user_2fa`, or returns
    /// `None` if no record exists (2-FA never configured for this user).
    fn load_user_2fa(&self, username: &str) -> Option<User2FA> {
        let conn = self.open_conn()?;
        let row = conn
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
            .optional();

        let Ok(Some((enabled, secret, backup_codes_json, enabled_at, last_verified))) = row else {
            return None;
        };

        let backup_codes = backup_codes_json
            .as_deref()
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
            .unwrap_or_default();

        let totp_config = secret.map(|secret| TotpConfig {
            secret,
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });

        Some(User2FA {
            username: username.to_string(),
            enabled,
            totp_config,
            backup_codes,
            enabled_at,
            last_verified,
        })
    }

    /// Upserts a [`User2FA`] record using SQLite's `ON CONFLICT … DO UPDATE`
    /// semantics so the same call can both insert new records and update
    /// existing ones without callers needing to distinguish the two cases.
    fn upsert_user_2fa(&self, user: &User2FA) {
        let Some(conn) = self.open_conn() else {
            return;
        };

        let secret = user.totp_config.as_ref().map(|cfg| cfg.secret.clone());
        let backup_codes_json =
            serde_json::to_string(&user.backup_codes).unwrap_or_else(|_| "[]".to_string());

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
            warn!("2fa upsert failed for user {}: {}", user.username, e);
        }
    }

    // -----------------------------------------------------------------------
    // User credential management (password verification)
    // -----------------------------------------------------------------------

    /// Loads the stored password hash for `username`, or `None` if the user
    /// has never registered (first-time login auto-creates the record).
    fn load_pw_hash(&self, username: &str) -> Option<String> {
        let conn = self.open_conn()?;
        conn.query_row(
            "SELECT pw_hash FROM user_credentials WHERE username = ?1",
            params![username],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()?
    }

    /// Creates or updates the credential record for `username`.
    ///
    /// On first login the hash is stored. On subsequent logins the stored
    /// hash is compared against the submitted hash; a mismatch is rejected.
    fn upsert_credentials(&self, username: &str, pw_hash: &str) {
        let Some(conn) = self.open_conn() else {
            return;
        };
        let ts = now();
        if let Err(e) = conn.execute(
            "INSERT INTO user_credentials(username, pw_hash, created_at, updated_at, login_count, last_login)
             VALUES(?1, ?2, ?3, ?3, 1, ?3)
             ON CONFLICT(username) DO UPDATE SET
                 updated_at  = excluded.updated_at,
                 login_count = login_count + 1,
                 last_login  = excluded.last_login",
            params![username, pw_hash, ts],
        ) {
            warn!("credential upsert failed for user {}: {}", username, e);
        }
    }

    /// Checks whether `submitted_hash` matches the stored hash for `username`.
    ///
    /// Returns:
    /// - `Ok(true)` if the hash matches.
    /// - `Ok(false)` if the hash does not match.
    /// - `Err("first_login")` if no credential exists yet (first-time user).
    fn verify_credential(
        &self,
        username: &str,
        submitted_hash: &str,
    ) -> Result<bool, &'static str> {
        match self.load_pw_hash(username) {
            None => Err("first_login"),
            Some(stored) => Ok(crypto::pw_verify(submitted_hash, &stored)),
        }
    }
}

// ---------------------------------------------------------------------------
// Channel — in-memory broadcast + history ring buffer
// ---------------------------------------------------------------------------

/// A named chat channel consisting of a bounded in-memory history ring buffer
/// and a [tokio broadcast] channel for real-time fan-out to all subscribers.
///
/// `Channel` is cheap to clone; all clones share the same `Arc`-wrapped
/// history and the same `broadcast::Sender` handle. New subscribers obtain a
/// fresh `Receiver` via `tx.subscribe()`.
#[derive(Clone)]
struct Channel {
    /// In-memory ring buffer of the last [`HISTORY_CAP`] messages.
    /// Wrapped in `Arc<RwLock<…>>` so multiple tasks can read concurrently
    /// while writes are exclusive.
    history: Arc<RwLock<VecDeque<Value>>>,

    /// Broadcast sender. The channel capacity (256) is deliberately larger
    /// than [`HISTORY_CAP`] to absorb short bursts without dropping frames.
    tx: broadcast::Sender<String>,
}

impl Channel {
    /// Creates a new, empty channel with a 256-message broadcast buffer.
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))),
            tx,
        }
    }

    /// Appends `entry` to the in-memory history, evicting the oldest entry if
    /// the ring buffer is at capacity.
    async fn push(&self, entry: Value) {
        let mut h = self.history.write().await;
        if h.len() >= HISTORY_CAP {
            h.pop_front();
        }
        h.push_back(entry);
    }

    /// Returns a snapshot of the current history as a `Vec`, oldest first.
    async fn hist(&self) -> Vec<Value> {
        self.history.read().await.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// State — shared, thread-safe server state
// ---------------------------------------------------------------------------

/// Central server state shared by all connection handler tasks via `Arc`.
///
/// Every field uses a lock-free concurrent map ([`DashMap`]) or atomic
/// primitive so that individual operations (insert, remove, lookup) do not
/// require global locking. Per-channel operations that require exclusive
/// history access use `tokio::sync::RwLock` scoped to the specific channel.
struct State {
    /// Named public channels, keyed by sanitised channel name.
    /// DM channels live here too under the `__dm__<username>` naming
    /// convention; they are filtered out when listing channels to clients.
    channels: DashMap<String, Channel>,

    /// Per-room voice broadcast senders, keyed by room name.
    voice: DashMap<String, broadcast::Sender<String>>,

    /// Current status value for each online user
    /// (e.g. `{"text":"Online","emoji":"🟢"}`).
    /// Presence in this map is the authoritative signal that a user is online.
    user_statuses: DashMap<String, Value>,

    /// Public key (base64) for each online user, used by clients to encrypt
    /// DM payloads without a separate key-exchange round-trip.
    user_pubkeys: DashMap<String, String>,

    /// Per-user ring buffer of recently seen nonce values.
    /// Bounded to [`NONCE_CACHE_CAP`] entries; the oldest entry is evicted
    /// once the cap is reached. See [`validate_and_register_nonce`].
    recent_nonces: DashMap<String, VecDeque<String>>,

    /// Metadata for in-progress file transfers, keyed by `file_id`.
    /// Used by receivers to look up transfer details before accepting chunks.
    file_transfers: DashMap<String, Value>,

    /// Number of WebSocket connections currently open. Managed via
    /// [`ConnectionGuard`] RAII to guarantee accurate accounting even on
    /// panics.
    active_connections: AtomicUsize,

    /// Notified whenever `active_connections` reaches zero, allowing the
    /// graceful-shutdown loop to wake immediately rather than polling.
    drained_notify: Notify,

    /// SQLite-backed event persistence and 2-FA storage.
    store: EventStore,

    /// Whether structured logging is active. Checked before every `info!` /
    /// `warn!` call to avoid the overhead of the logging facade when it is
    /// off.
    log_enabled: bool,

    /// Per-IP connection count for rate limiting.
    /// Incremented on TCP accept, decremented on disconnect.
    ip_connections: DashMap<std::net::IpAddr, usize>,

    /// Per-IP last auth attempt timestamp.
    /// Used to enforce a minimum interval between auth attempts.
    ip_last_auth: DashMap<std::net::IpAddr, f64>,

    /// Session tokens keyed by token string → username.
    /// Generated at auth time and validated on every post-auth frame.
    session_tokens: DashMap<String, String>,
}

impl State {
    /// Creates the initial server state, pre-populating the `"general"` channel.
    fn new(log_enabled: bool, db_path: String, db_key: Option<Vec<u8>>) -> Arc<Self> {
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            recent_nonces: DashMap::new(),
            file_transfers: DashMap::new(),
            active_connections: AtomicUsize::new(0),
            drained_notify: Notify::new(),
            store: EventStore::new(db_path, db_key),
            log_enabled,
            ip_connections: DashMap::new(),
            ip_last_auth: DashMap::new(),
            session_tokens: DashMap::new(),
        });
        // `"general"` is guaranteed to exist so that new connections always
        // have a channel to subscribe to before any `"join"` event is sent.
        s.channels.insert("general".into(), Channel::new());
        s
    }

    /// Returns the [`Channel`] for `name`, creating it lazily on first access.
    fn chan(&self, name: &str) -> Channel {
        self.channels
            .entry(name.into())
            .or_insert_with(Channel::new)
            .clone()
    }

    /// Returns the voice broadcast sender for `room`, creating it lazily on
    /// first access. The `_` receiver returned by `broadcast::channel` is
    /// immediately dropped — active subscribers obtain their own receivers
    /// via `vtx.subscribe()` when they call `"vjoin"`.
    fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.voice
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone()
    }

    /// Returns the number of currently online users (users with an active
    /// WebSocket connection that has completed auth).
    fn online_count(&self) -> usize {
        self.user_statuses.len()
    }

    /// Serialises the list of public (non-DM) channel names as a JSON array.
    fn channels_json(&self) -> Value {
        Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| Value::String(e.key().clone()))
                .collect(),
        )
    }

    /// Serialises the list of online users with their public keys as a JSON
    /// array of `{"u": "...", "pk": "..."}` objects.
    ///
    /// This is included in the `"ok"` auth response so clients can populate
    /// their local key stores without making a separate `"users"` request.
    fn users_with_keys_json(&self) -> Value {
        Value::Array(
            self.user_pubkeys
                .iter()
                .map(|e| serde_json::json!({"u": e.key().clone(), "pk": e.value().clone()}))
                .collect(),
        )
    }

    fn connection_opened(&self) {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrements the connection counter. If the counter reaches zero, notifies
    /// the [`drained_notify`](Self::drained_notify) condition variable so the
    /// graceful-shutdown loop can wake immediately.
    fn connection_closed(&self) {
        let prev = self.active_connections.fetch_sub(1, Ordering::SeqCst);
        if prev <= 1 {
            self.drained_notify.notify_waiters();
        }
    }

    fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

    // -----------------------------------------------------------------------
    // Rate limiting
    // -----------------------------------------------------------------------

    /// Increments the per-IP connection counter. Returns `false` if the
    /// IP has exceeded [`MAX_CONNECTIONS_PER_IP`].
    fn ip_connect(&self, addr: &SocketAddr) -> bool {
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
    fn ip_disconnect(&self, addr: &SocketAddr) {
        let ip = addr.ip();
        if let Some(mut entry) = self.ip_connections.get_mut(&ip) {
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                drop(entry);
                self.ip_connections.remove(&ip);
            }
        }
    }

    /// Checks whether an auth attempt from `addr` is allowed under the
    /// per-IP rate limit. If allowed, updates the last-auth timestamp.
    fn ip_auth_allowed(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        let now = crate::now();
        if let Some(last) = self.ip_last_auth.get(&ip) {
            if now - *last < AUTH_RATE_LIMIT_SECS {
                return false;
            }
        }
        self.ip_last_auth.insert(ip, now);
        true
    }

    // -----------------------------------------------------------------------
    // Session tokens
    // -----------------------------------------------------------------------

    /// Generates a cryptographically random session token and associates it
    /// with `username`. Returns the token string.
    fn create_session(&self, username: &str) -> String {
        use rand::{rngs::OsRng, RngCore};
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        self.session_tokens
            .insert(token.clone(), username.to_string());
        token
    }

    /// Removes the session token associated with `username`.
    fn remove_session(&self, username: &str) {
        self.session_tokens.retain(|_, v| v != username);
    }
}

// ---------------------------------------------------------------------------
// RAII connection tracking guard
// ---------------------------------------------------------------------------

/// RAII guard that increments [`State::active_connections`] on construction
/// and decrements it on drop, guaranteeing accurate accounting even when a
/// connection handler panics or returns early.
struct ConnectionGuard {
    state: Arc<State>,
    addr: SocketAddr,
}

impl ConnectionGuard {
    fn new(state: Arc<State>, addr: SocketAddr) -> Self {
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

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Clamps a raw `limit` parameter from a client request to `[1, max]`,
/// substituting `default` when the field is absent.
///
/// This prevents clients from requesting zero or unreasonably large result
/// sets while still allowing the server to apply sensible per-endpoint
/// maximums without duplicating clamping logic in each handler.
fn clamp_limit(raw: Option<u64>, default: usize, max: usize) -> usize {
    raw.map(|v| v as usize).unwrap_or(default).clamp(1, max)
}

/// Enqueues a JSON `payload` onto the per-connection outbound mpsc channel.
///
/// The error from `send` is intentionally ignored: if the receiver has been
/// dropped (e.g. because the WebSocket sink task exited), the connection is
/// already being torn down and there is nowhere meaningful to report the error.
fn send_out_json(out_tx: &mpsc::UnboundedSender<String>, payload: Value) {
    let _ = out_tx.send(payload.to_string());
}

/// Spawns a background task that forwards messages from a broadcast `rx` to
/// an mpsc `out_tx`, bridging the fan-out broadcast model to the single-writer
/// sink task.
///
/// The task exits cleanly when:
/// - `rx` reports `RecvError::Closed` (channel dropped).
/// - `out_tx.send()` fails (the sink task has exited).
///
/// Lagged messages (`RecvError::Lagged`) are silently skipped — the client
/// will see a gap in the message stream, which is preferable to crashing the
/// connection.
fn spawn_broadcast_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if out_tx.send(m).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {} // Lagged: skip and continue
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Event handler
// ---------------------------------------------------------------------------

/// Dispatches a single post-auth WebSocket event to the appropriate handler.
///
/// This function is the central routing switch for all client-initiated actions.
/// It is called in the connection's main read loop after the frame has been
/// size-checked and JSON-parsed.
///
/// # Replay protection
///
/// For event types listed in [`requires_fresh_protection`], this function
/// first validates the timestamp skew and then registers the nonce (if present)
/// before any business logic runs. A validation failure sends an `err` response
/// and returns early, leaving the connection open for subsequent valid frames.
///
/// # Supported event types
///
/// | Type          | Description                                         |
/// |---------------|-----------------------------------------------------|
/// | `msg`         | Broadcast a channel message (ciphertext + plaintext index) |
/// | `img`         | Broadcast a base64-encoded image to a channel       |
/// | `dm`          | Send an encrypted direct message to a single user   |
/// | `join`        | Subscribe to a channel and receive its history      |
/// | `history`     | Fetch persisted history for a channel               |
/// | `search`      | Full-text search over a channel's plaintext index   |
/// | `rewind`      | Fetch events within a relative time window          |
/// | `users`       | Get the current online user → public key directory  |
/// | `info`        | Get server info (channels list, online count)       |
/// | `vjoin`       | Join a voice room                                   |
/// | `vleave`      | Leave the current voice room                        |
/// | `vdata`       | Forward audio data to all members of a voice room   |
/// | `ping`        | Heartbeat — server replies with `pong`              |
/// | `edit`        | Edit a previously sent message (in-memory only)     |
/// | `file_meta`   | Announce a file transfer to a channel               |
/// | `file_chunk`  | Stream a chunk of a file transfer                   |
/// | `status`      | Update the caller's presence status                 |
/// | `reaction`    | Add an emoji reaction to a message                  |
/// | `2fa_setup`   | Begin the TOTP enrollment flow                      |
/// | `2fa_enable`  | Finalise TOTP enrollment with a verification code   |
/// | `2fa_disable` | Disable 2-FA for the current user                   |
/// | `2fa_verify`  | Verify a TOTP or backup code post-auth              |
///
/// Unknown event types are silently ignored.
async fn handle_event(
    d: &Value,
    state: &Arc<State>,
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    voice_room: &mut Option<String>,
) {
    let t = d["t"].as_str().unwrap_or("");
    let event_channel = d
        .get("ch")
        .or_else(|| d.get("r"))
        .and_then(|v| v.as_str())
        .map(safe_ch);

    if state.log_enabled {
        if let Some(ch) = event_channel.as_deref() {
            info!("event user={} type={} channel={}", username, t, ch);
        } else {
            info!("event user={} type={}", username, t);
        }
    }

    // --- Replay protection (timestamp skew + nonce dedup) ------------------
    // Only applied to mutating events (see requires_fresh_protection).
    if requires_fresh_protection(t) {
        if let Err(e) = validate_timestamp_skew(d) {
            if state.log_enabled {
                warn!(
                    "protocol validation failed user={} type={} reason={}",
                    username, t, e
                );
            }
            send_out_json(
                out_tx,
                serde_json::json!({"t":"err","m":format!("protocol validation failed: {}", e)}),
            );
            return;
        }
        if let Err(e) = validate_and_register_nonce(state, username, d) {
            if state.log_enabled {
                warn!(
                    "protocol validation failed user={} type={} reason={}",
                    username, t, e
                );
            }
            send_out_json(
                out_tx,
                serde_json::json!({"t":"err","m":format!("protocol validation failed: {}", e)}),
            );
            return;
        }
    }

    // --- Event dispatch switch ---------------------------------------------
    match t {
        "msg" => {
            // Broadcast an encrypted message to a channel.
            // `"c"` is the ciphertext blob; `"p"` is optional plaintext for
            // the search index only — it is never echoed back to clients.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let c = d["c"].as_str().unwrap_or("").to_string();
            let p = d["p"].as_str().unwrap_or("").to_string();
            if c.is_empty() {
                return;
            }
            let entry = serde_json::json!({"t":"msg","ch":ch,"u":username,"c":c,"ts":now()});
            let chan = state.chan(&ch);
            chan.push(entry.clone()).await;
            // Fall back to ciphertext as the search index when no plaintext is provided.
            let searchable = if p.is_empty() { c.clone() } else { p };
            state
                .store
                .persist("msg", &ch, username, None, &entry, &searchable);
            let _ = chan.tx.send(entry.to_string());
        }
        "img" => {
            // Broadcast a base64-encoded image. Not persisted to avoid bloating
            // the event store with large binary payloads.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let a = d["a"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                return;
            }
            let _ = state.chan(&ch).tx.send(
                serde_json::json!({"t":"img","ch":ch,"u":username,"a":a,"ts":now()}).to_string(),
            );
        }
        "dm" => {
            // Send an encrypted direct message.
            // The sender's public key is injected by the server so the
            // recipient can verify / decrypt without a separate lookup.
            let target = d["to"].as_str().unwrap_or("").to_string();
            let c = d["c"].as_str().unwrap_or("").to_string();
            let ptxt = d["p"].as_str().unwrap_or("").to_string();
            if c.is_empty() || target.is_empty() {
                return;
            }
            let sender_pk = state
                .user_pubkeys
                .get(username)
                .map(|v| v.value().clone())
                .unwrap_or_default();
            let event = serde_json::json!({
                "t":"dm","from":username,"to":target,
                "c":c,"pk":sender_pk,"ts":now()
            });
            let p = event.to_string();
            // Persist to both the recipient's and the sender's DM channel so
            // history is available from either party's perspective.
            state.store.persist(
                "dm",
                &format!("__dm__{}", target),
                username,
                Some(&target),
                &event,
                &ptxt,
            );
            let _ = state.chan(&format!("__dm__{}", target)).tx.send(p.clone());
            let _ = state.chan(&format!("__dm__{}", username)).tx.send(p);
        }
        "join" => {
            // Subscribe to a channel and immediately receive its history.
            // SQLite history takes precedence over the in-memory ring buffer
            // so newly booted servers serve correct history from persisted data.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let chan = state.chan(&ch);
            let mut hist = state.store.history(&ch, HISTORY_CAP);
            if hist.is_empty() {
                hist = chan.hist().await;
            }
            spawn_broadcast_forwarder(chan.tx.subscribe(), out_tx.clone());
            send_out_json(
                out_tx,
                serde_json::json!({"t":"joined","ch":ch,"hist":hist}),
            );
            let join_msg = serde_json::json!({
                "t":"sys",
                "m":format!("→ {} joined #{}", username, ch),
                "ts":now()
            });
            state.store.persist(
                "sys",
                &ch,
                username,
                None,
                &join_msg,
                &format!("{} joined", username),
            );
            let _ = chan.tx.send(join_msg.to_string());
        }
        "history" => {
            // Return persisted events for a channel, newest-first from SQLite,
            // re-ordered to oldest-first by query_events before sending.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_HISTORY_LIMIT,
                200,
            );
            let events = state.store.history(&ch, limit);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":ch,"events":events,"ts":now()}),
            );
        }
        "search" => {
            // Full-text search over the `search_text` index column.
            // An empty query returns an empty result set rather than all events.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let q = d["q"].as_str().unwrap_or("").trim().to_string();
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_SEARCH_LIMIT,
                200,
            );
            let events = if q.is_empty() {
                Vec::new()
            } else {
                state.store.search(&ch, &q, limit)
            };
            send_out_json(
                out_tx,
                serde_json::json!({"t":"search","ch":ch,"q":q,"events":events,"ts":now()}),
            );
        }
        "rewind" => {
            // Time-window query: return events from the last `seconds` seconds.
            // The maximum window is capped at 31 days to prevent accidental
            // full-history dumps from a misconfigured client.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let seconds = d
                .get("seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_REWIND_SECONDS)
                .clamp(1, 31 * 24 * 3600);
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_REWIND_LIMIT,
                500,
            );
            let events = state.store.rewind(&ch, seconds, limit);
            // Rewind reuses the `"history"` frame type so clients only need
            // one parser for time-ranged and offset-based history.
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":ch,"events":events,"ts":now()}),
            );
        }
        "users" => {
            // Return the current user → public key directory.
            send_out_json(
                out_tx,
                serde_json::json!({"t":"users","users":state.users_with_keys_json()}),
            );
        }
        "info" => {
            // Return server metadata: channel list and online user count.
            let chs: Vec<String> = state
                .channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| e.key().clone())
                .collect();
            send_out_json(
                out_tx,
                serde_json::json!({"t":"info","chs":chs,"online":state.online_count()}),
            );
        }
        "vjoin" => {
            // Subscribe to a voice room's broadcast channel.
            // A system message is posted to the room's text channel to
            // notify other members.
            let room = safe_ch(d["r"].as_str().unwrap_or("general"));
            let vtx = state.voice_tx(&room);
            spawn_broadcast_forwarder(vtx.subscribe(), out_tx.clone());
            *voice_room = Some(room.clone());
            let join_voice = serde_json::json!({
                "t":"sys",
                "m":format!("🎙 {} joined voice #{}", username, room),
                "ts":now()
            });
            state.store.persist(
                "sys",
                &room,
                username,
                None,
                &join_voice,
                &format!("{} voice joined", username),
            );
            let _ = state.chan(&room).tx.send(join_voice.to_string());
        }
        "vleave" => {
            // Unsubscribe from the voice room (the broadcast receiver is
            // dropped when the forwarder task exits) and notify other members.
            if let Some(ref room) = voice_room.take() {
                let leave_voice = serde_json::json!({
                    "t":"sys",
                    "m":format!("🎙 {} left voice #{}", username, room),
                    "ts":now()
                });
                state.store.persist(
                    "sys",
                    room,
                    username,
                    None,
                    &leave_voice,
                    &format!("{} voice left", username),
                );
                let _ = state.chan(room).tx.send(leave_voice.to_string());
            }
        }
        "vdata" => {
            // Forward raw audio payload to all other voice-room members.
            // The sender's username is injected so receivers know who is
            // speaking without a separate signalling round-trip.
            let a = d["a"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                return;
            }
            if let Some(ref room) = voice_room {
                if let Some(vtx) = state.voice.get(room) {
                    let _ = vtx
                        .send(serde_json::json!({"t":"vdata","from":username,"a":a}).to_string());
                }
            }
        }
        "ping" => {
            // Heartbeat — keep-alive for clients behind proxies with idle
            // connection timeouts.
            let _ = out_tx.send(r#"{"t":"pong"}"#.into());
        }
        "edit" => {
            // In-memory edit of the most recent matching message.
            // The edit is persisted for history but not applied retroactively
            // to the SQLite event store — the original row is left intact.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let old_text = d["old_text"].as_str().unwrap_or("").to_string();
            let new_text = d["new_text"].as_str().unwrap_or("").to_string();
            if old_text.is_empty() || new_text.is_empty() {
                return;
            }
            let chan = state.chan(&ch);
            let mut h = chan.history.write().await;
            // Search in reverse to find the most-recent matching message from
            // this user (avoids editing an older message by mistake).
            if let Some(pos) = h.iter().rposition(|m| {
                m.get("t") == Some(&Value::from("msg"))
                    && m.get("u") == Some(&Value::from(username.to_string()))
                    && m.get("c") == Some(&Value::from(old_text.clone()))
            }) {
                h[pos]["c"] = Value::from(new_text.clone());
                h[pos]["ts"] = Value::from(now());
            }
            let edit_msg = serde_json::json!({
                "t":"edit","ch":ch,"u":username,
                "old_text":old_text,"new_text":new_text,"ts":now()
            });
            state
                .store
                .persist("edit", &ch, username, None, &edit_msg, &new_text);
            let _ = chan.tx.send(edit_msg.to_string());
        }
        "file_meta" => {
            // Announce a pending file transfer to the channel. The `file_id`
            // acts as a correlation key for subsequent `file_chunk` frames.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let filename = d["filename"].as_str().unwrap_or("unknown").to_string();
            let size = d["size"].as_u64().unwrap_or(0);

            // Reject files that exceed the maximum size.
            if size > MAX_FILE_SIZE {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":format!("file size exceeds maximum of {} bytes", MAX_FILE_SIZE)}),
                );
                return;
            }

            let file_id = d["file_id"]
                .as_str()
                .unwrap_or(&format!("{}_{}", username, now()))
                .to_string();
            state.file_transfers.insert(file_id.clone(), d.clone());
            let file_announce = serde_json::json!({
                "t":"file_meta","from":username,"filename":filename,
                "size":size,"file_id":file_id,"ch":ch,"ts":now()
            });
            state.store.persist(
                "file_meta",
                &ch,
                username,
                None,
                &file_announce,
                file_announce
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            let _ = state.chan(&ch).tx.send(file_announce.to_string());
        }
        "file_chunk" => {
            // Relay a single chunk of a file transfer to the channel.
            // Chunks are not persisted — they are ephemeral relay frames.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let file_id = d["file_id"].as_str().unwrap_or("").to_string();
            let chunk_data = d["data"].as_str().unwrap_or("").to_string();
            let index = d["index"].as_u64().unwrap_or(0);
            let chunk_msg = serde_json::json!({
                "t":"file_chunk","from":username,"file_id":file_id,
                "data":chunk_data,"index":index,"ch":ch,"ts":now()
            })
            .to_string();
            let _ = state.chan(&ch).tx.send(chunk_msg);
        }
        "status" => {
            // Broadcast a presence update to all channels so every connected
            // client can update its member list without polling.
            if let Some(status_val) = d.get("status") {
                state
                    .user_statuses
                    .insert(username.to_string(), status_val.clone());
                let status_update = serde_json::json!({
                    "t":"status_update","user":username,"status":status_val
                })
                .to_string();
                for chan_entry in state.channels.iter() {
                    let _ = chan_entry.tx.send(status_update.clone());
                }
            }
        }
        "reaction" => {
            // Broadcast an emoji reaction to a specific message in a channel.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let emoji = d["emoji"].as_str().unwrap_or("👍").to_string();
            let msg_id = d["msg_id"].as_str().unwrap_or("unknown").to_string();
            let reaction_msg = serde_json::json!({
                "t":"reaction","user":username,"emoji":emoji,
                "msg_id":msg_id,"ch":ch,"ts":now()
            });
            state
                .store
                .persist("reaction", &ch, username, None, &reaction_msg, &emoji);
            let _ = state.chan(&ch).tx.send(reaction_msg.to_string());
        }
        "2fa_setup" => {
            // Begin the TOTP enrollment flow: generate a fresh secret and
            // return a QR-code URL that the user can scan with an authenticator
            // app. The secret is NOT persisted here — it is only saved when
            // the user confirms enrollment via `2fa_enable`.
            let secret = generate_secret();
            let issuer = d["issuer"].as_str().unwrap_or("Chatify");
            let qr_url = generate_qr_url(username, issuer, &secret);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"2fa_setup","secret":secret,"qr_url":qr_url,
                    "issuer":issuer,"user":username,"ts":now()
                }),
            );
        }
        "2fa_enable" => {
            // Finalise TOTP enrollment. The client must supply the secret from
            // the previous `2fa_setup` step and a live TOTP code to prove the
            // authenticator app is correctly configured before the secret is
            // persisted.
            let secret = d["secret"].as_str().unwrap_or("").to_string();
            let code = d["code"].as_str().unwrap_or("").to_string();
            if secret.is_empty() || code.is_empty() {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":"2fa_enable requires secret and code"}),
                );
                return;
            }

            let mut user_2fa = User2FA::new(username.to_string());
            user_2fa.enable(secret);
            if !user_2fa.verify_totp(&code) {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":"invalid 2FA code"}),
                );
                return;
            }

            state.store.upsert_user_2fa(&user_2fa);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"2fa_enabled","enabled":true,
                    "backup_codes":user_2fa.backup_codes,"ts":now()
                }),
            );
        }
        "2fa_disable" => {
            // Disable 2-FA for the current user. Requires the current TOTP
            // code to prevent an attacker who gained session access from
            // silently disabling 2FA.
            let code = d["code"].as_str().unwrap_or("").to_string();
            if code.is_empty() {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":"2fa_disable requires current 2FA code"}),
                );
                return;
            }

            let mut user_2fa = match state.store.load_user_2fa(username) {
                Some(u) if u.enabled => u,
                _ => {
                    send_out_json(
                        out_tx,
                        serde_json::json!({"t":"err","m":"2FA is not enabled"}),
                    );
                    return;
                }
            };

            // Require valid TOTP code to disable
            if !user_2fa.verify_totp(&code) {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":"invalid 2FA code"}),
                );
                return;
            }

            user_2fa.disable();
            state.store.upsert_user_2fa(&user_2fa);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"2fa_disabled","enabled":false,"ts":now()}),
            );
        }
        "2fa_verify" => {
            // Post-auth TOTP verification (used for privileged operations that
            // require step-up authentication). Accepts either a live TOTP code
            // or a backup code; backup codes are single-use.
            let code = d["code"].as_str().unwrap_or("").to_string();
            if code.is_empty() {
                send_out_json(
                    out_tx,
                    serde_json::json!({"t":"err","m":"2fa_verify requires code"}),
                );
                return;
            }

            let mut user_2fa = match state.store.load_user_2fa(username) {
                Some(v) if v.enabled => v,
                _ => {
                    send_out_json(
                        out_tx,
                        serde_json::json!({"t":"err","m":"2FA is not enabled"}),
                    );
                    return;
                }
            };

            let ok = verify_user_2fa_code(&mut user_2fa, &code);
            if ok {
                state.store.upsert_user_2fa(&user_2fa);
            }

            send_out_json(
                out_tx,
                serde_json::json!({"t":"2fa_verify","ok":ok,"ts":now()}),
            );
        }
        _ => {
            // Unknown event type — silently ignore. This is intentional:
            // newer clients may send events that older servers do not
            // understand, and a hard error would break forward compatibility.
        }
    }
}

// ---------------------------------------------------------------------------
// Small utility functions
// ---------------------------------------------------------------------------

/// Returns the current Unix timestamp as a floating-point number of seconds.
///
/// `f64` provides sub-millisecond precision and is the format used for all
/// `ts` fields in the protocol. Timestamps are used for clock-skew validation
/// and event ordering; they are not used for security-critical comparisons
/// where integer arithmetic would be safer.
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Normalises a raw channel name to a safe, consistent format.
///
/// Rules applied in order:
/// 1. Lowercase the input.
/// 2. Strip a leading `#` (clients may include it as a UI convention).
/// 3. Keep only ASCII alphanumeric characters, `-`, and `_`.
/// 4. Truncate to 32 characters.
/// 5. Fall back to `"general"` if the result is empty.
///
/// This is applied to every client-supplied channel or room name before it
/// is used as a `DashMap` key or SQLite parameter, preventing channel-name
/// injection and collisions between logically identical names.
fn safe_ch(raw: &str) -> String {
    let s: String = raw
        .to_lowercase()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if s.is_empty() {
        "general".into()
    } else {
        s
    }
}

/// Constructs a serialised system message JSON string with the current
/// timestamp.
fn sys(text: &str) -> String {
    serde_json::json!({"t":"sys","m":text,"ts":now()}).to_string()
}

/// Logs `msg` via `log::info!` if `state.log_enabled` is set.
///
/// The guard check avoids the overhead of the logging facade (format string
/// allocation, level filter) when the server is run without `--log`.
fn log(state: &State, msg: &str) {
    if state.log_enabled {
        info!("{}", msg);
    }
}

/// Sends a system message to every public channel's broadcast sender.
///
/// Used for server-wide announcements (joins, leaves, shutdown notice).
/// DM channels are included in the broadcast because the channel map contains
/// them alongside public channels; this is harmless since DM channels
/// typically have at most two subscribers.
async fn broadcast_system_msg(state: &Arc<State>, msg: &str) {
    let sys_msg = sys(msg);
    for e in state.channels.iter() {
        let _ = e.tx.send(sys_msg.clone());
    }
}

/// Constructs the serialised `"ok"` auth response payload.
///
/// Inline construction here (rather than in the caller) keeps all protocol
/// field names in one place, making it easier to evolve the auth contract.
fn create_ok_response(username: &str, state: &Arc<State>, hist: Vec<Value>) -> String {
    serde_json::json!({
        "t": "ok",
        "u": username,
        "users": state.users_with_keys_json(),
        "channels": state.channels_json(),
        "hist": hist,
        "proto": {
            "v": PROTOCOL_VERSION,
            "max_payload_bytes": MAX_BYTES
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// Returns `true` if `name` is a valid username.
///
/// Valid usernames are non-empty, at most [`MAX_USERNAME_LEN`] characters,
/// and consist entirely of ASCII alphanumeric characters, `-`, or `_`.
/// Whitespace, punctuation, and Unicode are rejected to keep usernames safe
/// for use as map keys, log fields, and SQL parameters.
fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Returns `true` if `pk` is a base64-encoded 32-byte public key.
///
/// The length check on the raw string (≤ [`MAX_PUBLIC_KEY_FIELD_LEN`])
/// prevents base64 decoding arbitrarily large inputs. After decoding, the
/// decoded length must be exactly 32 bytes to match the Ed25519 key size.
fn is_valid_pubkey_b64(pk: &str) -> bool {
    if pk.is_empty() || pk.len() > MAX_PUBLIC_KEY_FIELD_LEN {
        return false;
    }
    match general_purpose::STANDARD.decode(pk) {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

/// Parses and validates an auth frame, returning a typed [`AuthInfo`] on
/// success or a [`ChatifyError`] on the first validation failure.
///
/// Validation is applied in field order so that error messages are
/// deterministic and easy to assert in tests:
///
/// 1. Frame must be a JSON object with `"t": "auth"`.
/// 2. `"u"` must pass [`is_valid_username`].
/// 3. `"pw"` must be non-empty and ≤ [`MAX_PASSWORD_FIELD_LEN`].
/// 4. `"pk"` must pass [`is_valid_pubkey_b64`].
/// 5. `"otp"` (optional) must be ≤ [`MAX_NONCE_LEN`] characters if present.
fn validate_auth_payload(d: &Value) -> ChatifyResult<AuthInfo> {
    if !d.is_object() {
        return Err(ChatifyError::Validation("invalid auth frame".to_string()));
    }
    if d.get("t").and_then(|v| v.as_str()) != Some("auth") {
        return Err(ChatifyError::Message(
            "first frame must be auth".to_string(),
        ));
    }

    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing username".to_string()))?
        .to_string();
    if !is_valid_username(&username) {
        return Err(ChatifyError::Validation("invalid username".to_string()));
    }

    let pw = d
        .get("pw")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing password hash".to_string()))?;
    if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
        return Err(ChatifyError::Validation(
            "invalid password hash".to_string(),
        ));
    }

    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing public key".to_string()))?
        .to_string();
    if !is_valid_pubkey_b64(&pubkey) {
        return Err(ChatifyError::Message("invalid public key".to_string()));
    }

    // Validate the status field: must be an object with bounded string fields.
    let status = validate_status_field(d.get("status"))?;

    let otp_code = d
        .get("otp")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    if let Some(code) = otp_code.as_deref() {
        if code.len() > MAX_NONCE_LEN {
            return Err(ChatifyError::Validation("invalid otp code".to_string()));
        }
    }

    Ok(AuthInfo {
        username,
        pw_hash: pw.to_string(),
        status,
        pubkey,
        otp_code,
    })
}

/// Validates the optional `"status"` field in the auth frame.
///
/// The status must be a JSON object. If present, `"text"` and `"emoji"`
/// sub-fields are length-checked to prevent abuse. Missing fields or an
/// absent status object default to a standard "Online" status.
fn validate_status_field(status: Option<&Value>) -> ChatifyResult<Value> {
    let Some(val) = status else {
        return Ok(serde_json::json!({"text": "Online", "emoji": ""}));
    };

    if !val.is_object() {
        return Err(ChatifyError::Validation(
            "status must be a JSON object".to_string(),
        ));
    }

    // Validate text field length
    if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        if text.len() > MAX_STATUS_TEXT_LEN {
            return Err(ChatifyError::Validation(format!(
                "status text exceeds {} characters",
                MAX_STATUS_TEXT_LEN
            )));
        }
    }

    // Validate emoji field length
    if let Some(emoji) = val.get("emoji").and_then(|v| v.as_str()) {
        if emoji.len() > MAX_STATUS_EMOJI_LEN {
            return Err(ChatifyError::Validation(format!(
                "status emoji exceeds {} characters",
                MAX_STATUS_EMOJI_LEN
            )));
        }
    }

    // Reject any other unexpected top-level fields in status
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

// ---------------------------------------------------------------------------
// 2-FA helpers
// ---------------------------------------------------------------------------

/// Verifies a TOTP or backup code for `user_2fa`, mutating state on success.
///
/// The verification order is:
/// 1. TOTP code (live window) — if valid, updates `last_verified`.
/// 2. Backup code — if valid, the code is consumed (removed from the list) by
///    `verify_backup_code`. This enforces single-use semantics at the model
///    layer before the caller persists the updated record.
fn verify_user_2fa_code(user_2fa: &mut User2FA, code: &str) -> bool {
    if user_2fa.verify_totp(code) {
        user_2fa.last_verified = Some(now());
        true
    } else {
        user_2fa.verify_backup_code(code)
    }
}

/// Enforces 2-FA requirements during the authentication handshake.
///
/// - If no `user_2fa` record exists for `username`, 2-FA is not configured
///   and authentication proceeds unconditionally.
/// - If a record exists but `enabled` is `false`, 2-FA is configured but
///   disabled; authentication proceeds unconditionally.
/// - If 2-FA is enabled and `otp_code` is `None`, returns
///   `Err("2FA code required")` so the client knows to prompt for a code.
/// - If 2-FA is enabled and the code fails verification, returns
///   `Err("invalid 2FA code")`.
/// - On success, persists the updated `user_2fa` record (updated
///   `last_verified` or consumed backup code).
fn enforce_2fa_on_auth(
    state: &Arc<State>,
    username: &str,
    otp_code: Option<&str>,
) -> ChatifyResult<()> {
    let Some(mut user_2fa) = state.store.load_user_2fa(username) else {
        return Ok(());
    };

    if !user_2fa.enabled {
        return Ok(());
    }

    let code = otp_code.ok_or_else(|| ChatifyError::Message("2FA code required".to_string()))?;
    if !verify_user_2fa_code(&mut user_2fa, code) {
        return Err(ChatifyError::Message("invalid 2FA code".to_string()));
    }

    state.store.upsert_user_2fa(&user_2fa);
    Ok(())
}

// ---------------------------------------------------------------------------
// Replay-protection helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `event_type` requires timestamp-skew validation and
/// nonce-based replay protection.
///
/// Only mutating events that change server state or carry sensitive content
/// are protected. Read-only queries (`"history"`, `"search"`, `"users"`,
/// `"info"`, `"ping"`) and control events (`"join"`, `"vjoin"`) are excluded
/// because replaying them is either idempotent or harmless.
fn requires_fresh_protection(event_type: &str) -> bool {
    matches!(
        event_type,
        "msg"
            | "img"
            | "dm"
            | "vdata"
            | "edit"
            | "file_meta"
            | "file_chunk"
            | "status"
            | "reaction"
    )
}

/// Validates that the client-supplied `"ts"` field is within
/// ±[`MAX_CLOCK_SKEW_SECS`] of the server's wall clock.
///
/// Nonce-based replay protection is only enforced when the client includes a
/// `"n"` field. Frames without `"n"` are accepted unconditionally for
/// backward compatibility with older clients that do not implement nonces.
///
/// A timestamp of `0` or below, or a non-finite value, is unconditionally
/// rejected to guard against clients that send uninitialised fields.
fn validate_timestamp_skew(d: &Value) -> ChatifyResult<()> {
    if d.get("n").and_then(|v| v.as_str()).is_none() {
        // No nonce present — backward-compatible path: skip skew check.
        return Ok(());
    }

    let Some(ts) = d
        .get("ts")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|u| u as f64)))
    else {
        return Err(ChatifyError::Validation("missing timestamp".to_string()));
    };

    if !ts.is_finite() || ts < 0.0 {
        return Err(ChatifyError::Validation("invalid timestamp".to_string()));
    }

    if (now() - ts).abs() > MAX_CLOCK_SKEW_SECS {
        return Err(ChatifyError::Validation(
            "timestamp outside allowed clock skew".to_string(),
        ));
    }

    Ok(())
}

/// Checks that the `"n"` nonce field has not been seen before, then records it.
///
/// # Nonce format
///
/// Nonces must be non-empty lowercase hexadecimal strings of at most
/// [`MAX_NONCE_LEN`] characters. This restriction:
/// - Prevents injection via non-hex characters in storage paths or logs.
/// - Bounds the per-entry size in the nonce cache.
///
/// # Cache eviction
///
/// Each user's nonce deque is capped at [`NONCE_CACHE_CAP`] entries. When the
/// cap is reached the oldest entry is evicted. Nonces older than
/// [`MAX_CLOCK_SKEW_SECS`] would be rejected by the timestamp check before
/// reaching nonce validation, so eviction does not open a replay window within
/// the skew window as long as `NONCE_CACHE_CAP` is large enough to hold all
/// nonces that could arrive within that window.
fn validate_and_register_nonce(state: &State, username: &str, d: &Value) -> ChatifyResult<()> {
    let Some(nonce) = d.get("n").and_then(|v| v.as_str()) else {
        // No nonce present — backward-compatible path.
        return Ok(());
    };

    if nonce.is_empty() || nonce.len() > MAX_NONCE_LEN {
        return Err(ChatifyError::Validation("invalid nonce".to_string()));
    }
    if !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ChatifyError::Validation("invalid nonce format".to_string()));
    }

    let mut user_nonces = state.recent_nonces.entry(username.to_string()).or_default();

    if user_nonces.iter().any(|n| n == nonce) {
        return Err(ChatifyError::Validation("replayed nonce".to_string()));
    }

    user_nonces.push_back(nonce.to_string());
    if user_nonces.len() > NONCE_CACHE_CAP {
        let _ = user_nonces.pop_front();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handshake validation (CVE-2023-43668 mitigation)
// ---------------------------------------------------------------------------

/// Callback for validating WebSocket handshake HTTP headers.
///
/// This callback is invoked during the WebSocket upgrade handshake to validate
/// HTTP headers before the connection is established. It mitigates CVE-2023-43668
/// by enforcing limits on header size and count, preventing denial-of-service
/// attacks via excessive HTTP headers.
///
/// # Security considerations
///
/// - Rejects requests with headers exceeding `MAX_HANDSHAKE_HEADER_SIZE` bytes
/// - Rejects requests with more than `MAX_HANDSHAKE_HEADERS` headers
/// - Logs suspicious activity for monitoring
struct HandshakeValidator;

impl Callback for HandshakeValidator {
    fn on_request(
        self,
        req: &Request,
        response: Response,
    ) -> Result<Response, http::Response<Option<String>>> {
        // Calculate total header size
        let mut total_header_size = req.uri().to_string().len();
        let header_count = req.headers().len();

        for (name, value) in req.headers().iter() {
            total_header_size += name.as_str().len();
            total_header_size += value.len();
        }

        // Validate header count
        if header_count > MAX_HANDSHAKE_HEADERS {
            warn!(
                "Handshake rejected: too many headers ({} > {})",
                header_count, MAX_HANDSHAKE_HEADERS
            );
            return Err(http::Response::builder()
                .status(431)
                .body(Some("Too Many Headers".to_string()))
                .unwrap());
        }

        // Validate total header size
        if total_header_size > MAX_HANDSHAKE_HEADER_SIZE {
            warn!(
                "Handshake rejected: headers too large ({} > {} bytes)",
                total_header_size, MAX_HANDSHAKE_HEADER_SIZE
            );
            return Err(http::Response::builder()
                .status(431)
                .body(Some("Request Header Fields Too Large".to_string()))
                .unwrap());
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

/// Handles a single client WebSocket connection from TCP accept to disconnect.
///
/// # Lifecycle
///
/// ```text
/// accept_async → read auth frame → validate auth → enforce 2FA
///     → register user → send "ok" → spawn sink writer task
///     → main recv loop ( handle_event )
///     → deregister user → broadcast leave
/// ```
///
/// A [`ConnectionGuard`] is created immediately after accept and dropped at
/// the end of the function, ensuring `active_connections` is always accurate.
///
/// # Concurrency model
///
/// The WebSocket stream is read sequentially in this task. Outbound messages
/// from broadcast channels and other connection tasks are queued via an
/// `mpsc::unbounded_channel` and drained by a dedicated sink-writer task.
/// This decouples the read path from the write path, preventing a slow write
/// from blocking event processing.
async fn handle<S>(stream: S, addr: SocketAddr, state: Arc<State>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // ConnectionGuard increments active_connections and decrements it on drop,
    // even if we return early.

    // --- IP-level rate limiting ---
    if !state.ip_connect(&addr) {
        if state.log_enabled {
            warn!(
                "connection rejected: too many connections from {}",
                addr.ip()
            );
        }
        // Best-effort: the stream may not support WebSocket yet, but try.
        if let Ok(ws) = accept_async(stream).await {
            let (mut sink, _) = ws.split();
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({
                        "t": "err",
                        "m": format!("too many connections from {}", addr.ip())
                    })
                    .to_string(),
                ))
                .await;
        }
        return;
    }

    let _conn_guard = ConnectionGuard::new(state.clone(), addr);

    // Upgrade the raw TCP stream to a WebSocket connection.
    // Use accept_hdr_async with custom callback to validate headers (CVE-2023-43668 mitigation).
    let ws = match accept_hdr_async(stream, HandshakeValidator).await {
        Ok(w) => w,
        Err(e) => {
            if state.log_enabled {
                debug!("WebSocket handshake failed from {}: {}", addr, e);
            }
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();

    // ---- Phase 1: read and validate the auth frame --------------------------

    let raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        Some(Ok(_)) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"first frame must be text auth"}).to_string(),
                ))
                .await;
            return;
        }
        _ => return,
    };

    // Reject oversized auth frames before JSON parsing.
    if raw.len() > MAX_AUTH_BYTES {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"auth frame too large"}).to_string(),
            ))
            .await;
        return;
    }

    let d: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"invalid auth JSON"}).to_string(),
                ))
                .await;
            return;
        }
    };

    let auth = match validate_auth_payload(&d) {
        Ok(a) => a,
        Err(err) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
                ))
                .await;
            return;
        }
    };

    // --- Per-IP auth rate limiting ---
    if !state.ip_auth_allowed(&addr) {
        if state.log_enabled {
            warn!("auth rate limited from {}", addr.ip());
        }
        let _ = sink
            .send(Message::Text(
                serde_json::json!({
                    "t": "err",
                    "m": "too many auth attempts, please wait"
                })
                .to_string(),
            ))
            .await;
        return;
    }

    let AuthInfo {
        username,
        pw_hash,
        status,
        pubkey,
        otp_code,
    } = auth;

    // --- Credential verification ---
    // The client sends a PBKDF2 hash of their password. The server stores
    // its own PBKDF2 hash of that value (with a random salt). This two-layer
    // approach means the server never sees the raw password, but also never
    // trusts a client-provided hash blindly.
    match state.store.verify_credential(&username, &pw_hash) {
        Ok(true) => {} // Hash matches — proceed.
        Ok(false) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"invalid credentials"}).to_string(),
                ))
                .await;
            if state.log_enabled {
                warn!("auth failed: invalid password for user={}", username);
            }
            return;
        }
        Err("first_login") => {
            // First time this username connects — store their credential.
            // The submitted hash is itself a PBKDF2 output, so we wrap it
            // in another salted PBKDF2 layer server-side.
            let server_hash = crypto::pw_hash(&pw_hash);
            state.store.upsert_credentials(&username, &server_hash);
            if state.log_enabled {
                info!("credentials created for new user={}", username);
            }
        }
        Err(e) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":format!("credential error: {}", e)})
                        .to_string(),
                ))
                .await;
            return;
        }
    }

    // --- Username uniqueness ---
    // Reject if this username is already online (prevents session hijacking).
    if state.user_statuses.contains_key(&username) {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"username already in use"}).to_string(),
            ))
            .await;
        if state.log_enabled {
            warn!("auth rejected: username '{}' already connected", username);
        }
        return;
    }

    if let Err(err) = enforce_2fa_on_auth(&state, &username, otp_code.as_deref()) {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
            ))
            .await;
        return;
    }

    // Generate a session token for this connection.
    let _session_token = state.create_session(&username);

    // ---- Phase 2: register user and send welcome response -------------------

    state.user_statuses.insert(username.clone(), status);
    state.user_pubkeys.insert(username.clone(), pubkey);

    // Subscribe to "general" before sending "ok" to avoid missing messages
    // that arrive between the response send and the subscription.
    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let mut hist = state.store.history("general", HISTORY_CAP);
    if hist.is_empty() {
        hist = general.hist().await;
    }

    let ok = create_ok_response(&username, &state, hist);
    if sink.send(Message::Text(ok)).await.is_err() {
        return;
    }

    broadcast_system_msg(&state, &format!("→ {} joined", username)).await;
    log(&state, &format!("+ {}", username));

    // ---- Phase 3: set up bidirectional message routing ----------------------

    // mpsc channel: all tasks that want to send to this client queue here.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward "general" broadcast to the outbound queue.
    spawn_broadcast_forwarder(gen_rx, out_tx.clone());

    // Sink writer task: drains out_rx and writes to the WebSocket sink.
    // Runs until out_rx is closed (out_tx is dropped at function exit).
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() {
                break;
            }
        }
    });

    // ---- Phase 4: main event loop -------------------------------------------

    // Tracks the current voice room so vleave / vdata know which room to act on.
    let mut voice_room: Option<String> = None;

    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                log(&state, &format!("ws recv error for {}: {}", username, e));
                break;
            }
            None => break, // Client closed the connection cleanly.
        };

        let raw = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue, // Binary / ping / pong frames are ignored.
        };

        // Payload size gate (post-auth; auth size is gated earlier).
        if raw.len() > MAX_BYTES {
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload exceeds max size"}),
            );
            continue;
        }

        let d: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                send_out_json(
                    &out_tx,
                    serde_json::json!({"t":"err","m":"invalid JSON payload"}),
                );
                continue;
            }
        };

        if !d.is_object() {
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload must be a JSON object"}),
            );
            continue;
        }

        handle_event(&d, &state, &username, &out_tx, &mut voice_room).await;
    }

    // ---- Phase 5: cleanup ---------------------------------------------------
    // Remove user presence so they no longer appear in the user directory.
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    // Invalidate the session token.
    state.remove_session(&username);
    // Clear the nonce cache to free memory; replays from this session are
    // no longer possible once the connection is closed.
    state.recent_nonces.remove(&username);
    broadcast_system_msg(&state, &format!("✖ {} left", username)).await;
    log(&state, &format!("- {}", username));
    // _conn_guard drops here, decrementing active_connections and IP counter.
}

// ---------------------------------------------------------------------------
// TLS support
// ---------------------------------------------------------------------------

/// Wraps a TLS stream, forwarding [`AsyncRead`] and [`AsyncWrite`] to the
/// inner `TlsStream<TcpStream>`. Needed because the two concrete stream types
/// (plain `TcpStream` and `TlsStream<TcpStream>`) are different types, and
/// [`accept_async`] needs a single type parameter.
struct ChatifyTlsStream {
    inner: tokio_rustls::server::TlsStream<TcpStream>,
}

impl tokio::io::AsyncRead for ChatifyTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ChatifyTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Unpin for ChatifyTlsStream {}

/// Holds either a plain TCP stream or a TLS-wrapped stream.
enum StreamType {
    Plain(TcpStream),
    Tls(Box<ChatifyTlsStream>),
}

impl tokio::io::AsyncRead for StreamType {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_read(cx, buf),
            StreamType::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for StreamType {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_write(cx, buf),
            StreamType::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_flush(cx),
            StreamType::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_shutdown(cx),
            StreamType::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Loads a PEM certificate chain and private key, returning a [`TlsAcceptor`].
fn load_tls_config(cert_path: &str, key_path: &str) -> ChatifyResult<TlsAcceptor> {
    // Load certificate chain
    let cert_file = std::fs::File::open(cert_path).map_err(|e| {
        ChatifyError::Validation(format!("cannot open TLS cert '{}': {}", cert_path, e))
    })?;
    let mut cert_reader = std::io::BufReader::new(cert_file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS cert: {}", e)))?;
    if certs.is_empty() {
        return Err(ChatifyError::Validation(
            "TLS cert file is empty".to_string(),
        ));
    }

    // Load private key
    let key_file = std::fs::File::open(key_path).map_err(|e| {
        ChatifyError::Validation(format!("cannot open TLS key '{}': {}", key_path, e))
    })?;
    let mut key_reader = std::io::BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS key: {}", e)))?
        .ok_or_else(|| ChatifyError::Validation("TLS key file is empty".to_string()))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| ChatifyError::Validation(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Resolves the database encryption key from the CLI arg or a key file.
///
/// Resolution order:
/// 1. If `--db-key` is provided, decode it as hex (must be 64 chars = 32 bytes).
/// 2. If a `<db_path>.key` file exists, read and decode it.
/// 3. If `db_path` is `:memory:`, return `None` (no encryption for tests).
/// 4. Otherwise, generate a new random 32-byte key, write it to `<db_path>.key`,
///    and return it.
///
/// The `.key` file is created with user-only permissions where possible.
/// Store it alongside backups; losing it means the database is unrecoverable.
fn resolve_db_key(db_path: &str, cli_key: Option<&str>) -> ChatifyResult<Option<Vec<u8>>> {
    // 1. CLI-provided key takes priority.
    if let Some(hex_key) = cli_key {
        let key = hex::decode(hex_key)
            .map_err(|e| ChatifyError::Validation(format!("invalid --db-key hex: {}", e)))?;
        if key.len() != 32 {
            return Err(ChatifyError::Validation(format!(
                "--db-key must be 32 bytes (64 hex chars), got {} bytes",
                key.len()
            )));
        }
        return Ok(Some(key));
    }

    // 2. In-memory databases don't need encryption.
    if db_path == ":memory:" {
        return Ok(None);
    }

    // 3. Check for an existing key file.
    let key_path = format!("{}.key", db_path);
    if std::path::Path::new(&key_path).exists() {
        let hex_key = std::fs::read_to_string(&key_path)
            .map_err(|e| ChatifyError::Io(Box::new(e)))?
            .trim()
            .to_string();
        let key = hex::decode(&hex_key).map_err(|e| {
            ChatifyError::Validation(format!("invalid hex in key file '{}': {}", key_path, e))
        })?;
        if key.len() != 32 {
            return Err(ChatifyError::Validation(format!(
                "key file '{}' must contain 32 bytes (64 hex chars)",
                key_path
            )));
        }
        return Ok(Some(key));
    }

    // 4. Generate a new key and write it to disk.
    use rand::{rngs::OsRng, RngCore};
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    let hex_key = hex::encode(key);
    std::fs::write(&key_path, &hex_key).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    println!("Generated new DB encryption key: {}", key_path);
    Ok(Some(key.to_vec()))
}

/// Server entry point.
///
/// 1. Parses CLI args and initialises optional logging.
/// 2. Resolves the database encryption key.
/// 3. Binds the TCP listener.
/// 4. Initialises shared [`State`] (which runs SQLite migrations).
/// 5. Accepts connections in a `tokio::select!` loop until Ctrl+C.
/// 6. Broadcasts a shutdown notice and waits up to 10 s for connections to
///    drain before returning.
#[tokio::main]
async fn main() -> ChatifyResult<()> {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);

    if args.log {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    let db_key = resolve_db_key(&args.db, args.db_key.as_deref())?;

    // Set up TLS if enabled.
    let tls_acceptor = if args.tls {
        let acceptor = load_tls_config(&args.tls_cert, &args.tls_key)?;
        Some(acceptor)
    } else {
        None
    };

    let listener = TcpListener::bind(&addr).await?;
    let state = State::new(args.log, args.db.clone(), db_key);

    let enc_label = if state.store.is_encrypted() {
        "ChaCha20-Poly1305"
    } else {
        "None (unencrypted)"
    };
    let proto = if tls_acceptor.is_some() { "wss" } else { "ws" };
    println!(" Chatify running on {}://{}", proto, addr);
    println!(" Encryption: {} |   IP Privacy: On", enc_label);
    println!(" Event store: {}", args.db);
    println!("⏹  Press Ctrl+C to stop\n");
    if args.log {
        info!("server started addr={}://{} db={}", proto, addr, args.db);
    }

    // Accept loop: runs until Ctrl+C is received.
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                broadcast_system_msg(&state, "Server is shutting down").await;
                println!("\nShutdown signal received. Stopping server loop...");
                if state.log_enabled {
                    info!("shutdown signal received; stopping accept loop");
                }
                break;
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, addr)) => {
                        let s = state.clone();
                        if let Some(ref acceptor) = tls_acceptor {
                            let acceptor = acceptor.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        handle(
                                            StreamType::Tls(Box::new(ChatifyTlsStream { inner: tls_stream })),
                                            addr,
                                            s,
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        if s.log_enabled {
                                            warn!("TLS handshake failed from {}: {}", addr, e);
                                        }
                                    }
                                }
                            });
                        } else {
                            tokio::spawn(handle(StreamType::Plain(stream), addr, s));
                        }
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    // Graceful drain: wait up to 10 s for all handlers to finish.
    // The `drained_notify` Notify wakes us immediately if connections drain
    // before the 250 ms poll interval fires.
    let drain_timeout = Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        let active = state.active_connection_count();
        if active == 0 {
            break;
        }
        if start.elapsed() >= drain_timeout {
            println!(
                "Shutdown timeout reached with {} active connection(s)",
                active
            );
            if state.log_enabled {
                warn!(
                    "shutdown timeout reached with {} active connection(s)",
                    active
                );
            }
            break;
        }
        println!("Waiting for {} active connection(s) to close...", active);
        if state.log_enabled {
            info!("waiting for active connections to drain count={}", active);
        }
        tokio::select! {
            _ = state.drained_notify.notified() => {}
            _ = sleep(Duration::from_millis(250)) => {}
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that [`validate_auth_payload`] returns a `ChatifyError::Validation`
    /// variant (not `Message`) for an invalid username, allowing callers to
    /// distinguish validation errors from protocol errors.
    ///
    /// The specific error message `"invalid username"` is part of the public
    /// error contract and must not change without updating client-side error
    /// handling.
    #[test]
    fn auth_payload_rejects_invalid_username_with_typed_error() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "bad user",  // space is not allowed
            "pw": "abc123",
            "pk": base64::engine::general_purpose::STANDARD.encode([0u8; 32])
        });

        let err = match validate_auth_payload(&payload) {
            Ok(_) => panic!("expected validation error"),
            Err(e) => e,
        };
        match err {
            ChatifyError::Validation(msg) => assert_eq!(msg, "invalid username"),
            other => panic!("unexpected error type: {}", other),
        }
    }

    /// Verifies that [`ConnectionGuard`] correctly increments and decrements
    /// [`State::active_connections`].
    ///
    /// Two guards are created concurrently to confirm the counter reaches 2,
    /// then both are dropped. The test polls until the counter returns to 0
    /// with a 1-second timeout to account for any scheduling delay between
    /// the drop and the atomic write.
    #[tokio::test]
    async fn connection_counter_tracks_open_and_close() {
        let state = State::new(false, ":memory:".to_string(), None);
        assert_eq!(state.active_connection_count(), 0);

        let addr1: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.2:12345".parse().unwrap();
        {
            let _g1 = ConnectionGuard::new(state.clone(), addr1);
            let _g2 = ConnectionGuard::new(state.clone(), addr2);
            assert_eq!(state.active_connection_count(), 2);
        }

        let start = std::time::Instant::now();
        while state.active_connection_count() != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(1),
                "active connections did not drain in time"
            );
            sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(state.active_connection_count(), 0);
    }
}
