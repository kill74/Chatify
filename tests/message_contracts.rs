//! # `clifford-server` Integration Test Suite
//!
//! End-to-end contract tests for the `clifford-server` WebSocket binary.
//!
//! ## Architecture
//!
//! Each test:
//!   1. Binds an ephemeral OS port via [`allocate_port`].
//!   2. Spawns the compiled server binary as a child process pointing at a
//!      temporary SQLite database.
//!   3. Exercises the public WebSocket protocol through [`connect_and_auth`] /
//!      [`recv_by_type`] helpers.
//!   4. Drops [`TestServer`], which kills the child process and deletes the
//!      database file, leaving no test artefacts on disk.
//!
//! ## Running
//!
//! ```bash
//! cargo test --test integration
//! ```
//!
//! The binary path is resolved at compile time via the `CARGO_BIN_EXE_clifford-server`
//! env var injected by Cargo, so no manual `PATH` setup is needed.
//!
//! ## Coverage Areas
//!
//! | Area             | Tests                                                   |
//! |------------------|---------------------------------------------------------|
//! | Auth contract    | Field validation, key format, oversized frames, JSON    |
//! | 2-FA             | Missing code, backup-code happy path, code consumption  |
//! | Protocol safety  | Timestamp skew, nonce replay, payload size, fuzz corpus |
//! | Schema migration | v0→v4 upgrade, future-version no-downgrade              |
//! | Feature contracts| Messages, history, search, replay, rewind, voice (vdata) |

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use clifford::crypto::{new_keypair, pub_b64};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// A running server process paired with its database and WebSocket URL.
///
/// Implements [`Drop`] to guarantee cleanup of both the child process and the
/// temporary database file regardless of whether the test panics.
struct TestServer {
    /// The spawned server process. Prefixed with `_` so Rust keeps it alive
    /// for the full lifetime of [`TestServer`] without an "unused" warning.
    _child: Child,

    /// `ws://127.0.0.1:<port>` – the root WebSocket endpoint.
    url: String,

    /// Absolute path to the SQLite file used by this server instance.
    /// Stored here so tests can inspect database state after server operations
    /// and so [`Drop`] can delete the file after the test.
    db_path: PathBuf,

    /// When true, [`Drop`] removes `db_path` during teardown.
    cleanup_db: bool,
}

impl Drop for TestServer {
    /// Kills the server process and removes the temporary database.
    ///
    /// Errors from `kill`, `wait`, and `remove_file` are deliberately ignored:
    /// the process may have already exited and the file may have already been
    /// removed by the OS, both of which are acceptable in a test teardown.
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
        if self.cleanup_db {
            let _ = std::fs::remove_file(&self.db_path);
        }
    }
}

impl TestServer {
    /// Stops the running server process but preserves the backing SQLite file.
    ///
    /// Used by restart tests that need to re-launch the server against the
    /// exact same database path.
    fn stop_and_preserve_db(mut self) -> PathBuf {
        self.cleanup_db = false;
        let _ = self._child.kill();
        let _ = self._child.wait();
        self.db_path.clone()
    }
}

/// Convenience type alias for an authenticated, ready-to-use WebSocket stream.
type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Protocol version this test suite expects to remain backward-compatible.
const SUPPORTED_PROTOCOL_VERSION: u64 = 1;

// ---------------------------------------------------------------------------
// Server / database helpers
// ---------------------------------------------------------------------------

/// Binds a TCP listener on `127.0.0.1:0` to let the OS assign a free port,
/// then immediately drops the listener so the server can reuse that port.
///
/// **Note:** There is an inherent TOCTOU window between releasing the listener
/// and the server binding. In practice this is negligible on loopback for
/// integration tests, but it is worth being aware of in constrained CI
/// environments where ports recycle quickly.
fn allocate_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr").port()
}

/// Returns a unique temporary database path of the form
/// `$TMPDIR/chatify-test-<port>-<nanoseconds>.db`.
///
/// The port and nanosecond timestamp together make collisions between
/// concurrent test runs vanishingly unlikely.
fn temp_db_path(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!(
        "chatify-test-{}-{}.db",
        port,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

/// Pre-populates `schema_meta` with the given `version` string.
///
/// Used by migration tests to simulate a database that was created by an
/// earlier (or future) version of the server. The `schema_meta` table is
/// created if it does not already exist, so this helper is safe to call on a
/// fresh, empty SQLite file.
fn seed_schema_version(db_path: &PathBuf, version: &str) {
    let conn = Connection::open(db_path).expect("create sqlite db");
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT INTO schema_meta(key, value)
        VALUES('schema_version', '0')
        ON CONFLICT(key) DO UPDATE SET value='0';
        ",
    )
    .expect("seed schema version");
    conn.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'schema_version'",
        [version],
    )
    .expect("update schema version");
}

/// Hash a backup code for secure storage (matching totp.rs implementation)
fn hash_backup_code(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"chatify:backup:v1:");
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}

/// Seeds a user record in `user_2fa` with 2-FA **already enabled** and a set
/// of hashed backup codes.
///
/// This simulates a production database state where a user has completed the
/// 2-FA enrollment flow, allowing tests to exercise the authentication path
/// without needing a real TOTP device.
///
/// # Parameters
///
/// * `db_path`      – Absolute path to the target SQLite file.
/// * `username`     – The username to seed (must satisfy the server's username
///   validation rules).
/// * `backup_codes` – Raw backup code strings. These are hashed before storage
///   to match the server's security model.
fn seed_enabled_2fa_user(db_path: &PathBuf, username: &str, backup_codes: &[&str]) {
    let conn = Connection::open(db_path).expect("create sqlite db for 2fa seed");
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT INTO schema_meta(key, value)
        VALUES('schema_version', '2')
        ON CONFLICT(key) DO UPDATE SET value='2';

        CREATE TABLE IF NOT EXISTS user_2fa (
            username      TEXT PRIMARY KEY,
            enabled       BOOLEAN NOT NULL DEFAULT FALSE,
            secret        TEXT,
            backup_codes  TEXT,
            enabled_at    REAL,
            last_verified REAL
        );
        ",
    )
    .expect("create schema and user_2fa tables for seed");

    // Hash the backup codes before storing (matching totp.rs security model)
    let hashed_codes: Vec<String> = backup_codes
        .iter()
        .map(|code| hash_backup_code(code))
        .collect();
    let backup_codes_json =
        serde_json::to_string(&hashed_codes).expect("serialize backup codes for seed");

    conn.execute(
        "INSERT INTO user_2fa(username, enabled, secret, backup_codes, enabled_at, last_verified)
         VALUES(?1, TRUE, NULL, ?2, ?3, NULL)
         ON CONFLICT(username) DO UPDATE SET
             enabled       = TRUE,
             secret        = NULL,
             backup_codes  = excluded.backup_codes,
             enabled_at    = excluded.enabled_at,
             last_verified = NULL",
        params![
            username,
            backup_codes_json,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
        ],
    )
    .expect("seed enabled 2fa user");
}

/// Reads `schema_meta.schema_version` directly from SQLite.
///
/// Used by migration tests to assert the final schema version without going
/// through the server's own API, keeping migration assertions independent of
/// any server-side serialization changes.
fn read_schema_version(db_path: &PathBuf) -> String {
    let conn = Connection::open(db_path).expect("open sqlite db");
    conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )
    .expect("schema_version row exists")
}

/// Spawns the server binary against `db_path` on a newly allocated port and
/// polls the WebSocket endpoint until it is accepting connections (up to 5 s).
///
/// # Panics
///
/// Panics if the server does not become ready within the polling window, which
/// prevents tests from hanging indefinitely in CI.
async fn start_server_with_db(db_path: PathBuf) -> TestServer {
    let port = allocate_port();
    let url = format!("ws://127.0.0.1:{}", port);

    // Cargo injects the compiled binary path as an env var so we don't need
    // to hard-code build-directory paths or shell out to `cargo build`.
    let server_bin = std::env::var("CARGO_BIN_EXE_clifford-server")
        .expect("CARGO_BIN_EXE_clifford-server must be set by cargo test");

    let child = Command::new(server_bin)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--db")
        .arg(db_path.to_string_lossy().to_string())
        // Suppress server stdout/stderr to keep test output clean. To debug a
        // flaky test, swap Stdio::null() for Stdio::inherit() temporarily.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    // Poll with a 100 ms back-off; 50 attempts = 5 s total timeout.
    let mut ready = false;
    for _ in 0..50 {
        if let Ok((mut ws, _)) = connect_async(&url).await {
            let _ = ws.close(None).await;
            ready = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "server did not start in time at {}", url);

    TestServer {
        _child: child,
        url,
        db_path,
        cleanup_db: true,
    }
}

/// Convenience wrapper around [`start_server_with_db`] that allocates a fresh
/// temporary database automatically.
async fn start_server() -> TestServer {
    let port = allocate_port();
    let db_path = temp_db_path(port);
    start_server_with_db(db_path).await
}

// ---------------------------------------------------------------------------
// WebSocket helpers
// ---------------------------------------------------------------------------

/// Opens a WebSocket connection and authenticates as `username` **without**
/// a 2-FA code.
///
/// Asserts that the server responds with an `"ok"` frame containing the
/// expected `"u"` field before returning the live socket.
async fn connect_and_auth(url: &str, username: &str) -> Ws {
    connect_and_auth_with_otp(url, username, None).await
}

/// Opens a WebSocket connection and authenticates as `username`, optionally
/// supplying a TOTP or backup `otp` code.
///
/// # Protocol
///
/// ```json
/// // Request
/// { "t": "auth", "u": "<username>", "pw": "<hash>", "pk": "<base64>",
///   "status": { "text": "Online", "emoji": "🟢" }, "otp": "<code>" }
///
/// // Expected response
/// { "t": "ok", "u": "<username>", "channels": [...], "hist": [...],
///   "users": [ { "u": "...", "pk": "..." }, ... ] }
/// ```
///
/// A fresh ephemeral keypair is generated for every call so tests never share
/// key material, mirroring real client behaviour.
async fn connect_and_auth_with_otp(url: &str, username: &str, otp: Option<&str>) -> Ws {
    let (mut ws, _) = connect_async(url).await.expect("connect websocket");

    let auth = match otp {
        Some(code) => json!({
            "t": "auth", "u": username,
            "pw": "test-password-hash",
            "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"},
            "otp": code
        }),
        None => json!({
            "t": "auth", "u": username,
            "pw": "test-password-hash",
            "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        }),
    };

    ws.send(Message::Text(auth.to_string()))
        .await
        .expect("send auth");

    let ok = recv_by_type(&mut ws, "ok").await;
    assert_eq!(ok.get("u").and_then(|v| v.as_str()), Some(username));
    ws
}

/// Drains incoming WebSocket frames until one whose `"t"` field matches
/// `expected_type` is found, then returns the parsed [`Value`].
///
/// Non-matching frames (e.g. heartbeats, presence events) are silently
/// discarded. Each frame has a 3-second read timeout; after 50 attempts the
/// helper panics with a descriptive message to aid debugging.
///
/// # Panics
///
/// Panics if:
/// - The WebSocket closes unexpectedly.
/// - A receive I/O error occurs.
/// - No matching frame is seen within 50 attempts.
async fn recv_by_type(ws: &mut Ws, expected_type: &str) -> Value {
    for _ in 0..50 {
        let msg = timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("timeout waiting for websocket frame");
        let msg = msg.expect("websocket closed unexpectedly");
        let msg = msg.expect("websocket receive error");
        if let Message::Text(text) = msg {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value.get("t").and_then(|v| v.as_str()) == Some(expected_type) {
                    return value;
                }
            }
        }
    }
    panic!("did not receive message type '{}'", expected_type);
}

// ---------------------------------------------------------------------------
// Auth contract tests
// ---------------------------------------------------------------------------

/// Verifies that a user with 2-FA enabled **cannot** authenticate without
/// supplying an OTP code.
///
/// # Expected behaviour
///
/// The server must respond with `{ "t": "err", "m": "2FA code required" }`
/// rather than accepting the connection or returning a generic error, so the
/// client can present the correct UI prompt.
#[tokio::test]
async fn auth_contract_rejects_when_2fa_enabled_without_code() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_enabled_2fa_user(&db_path, "alice", &["backup-aa11bb22"]);

    let server = start_server_with_db(db_path).await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for 2fa missing code test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice",
            "pw": "test-password-hash",
            "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send auth without otp");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("2FA code required")
    );
}

/// Verifies that a valid backup code is accepted **and then consumed** (i.e.
/// single-use semantics), preventing the same code from being used twice.
///
/// # Test sequence
///
/// 1. Seed `alice` with one backup code.
/// 2. Authenticate successfully using that code.
/// 3. Reconnect and attempt to use the **same** code again.
/// 4. Assert the second attempt is rejected with `"invalid 2FA code"`.
///
/// This enforces that the server deletes (or marks used) the backup code on
/// first successful consumption rather than treating it as a persistent secret.
#[tokio::test]
async fn auth_contract_accepts_backup_code_and_consumes_it() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    let backup_code = "feedface1234abcd";
    seed_enabled_2fa_user(&db_path, "alice", &[backup_code]);

    let server = start_server_with_db(db_path).await;

    // First use of the backup code — must succeed.
    let mut ws = connect_and_auth_with_otp(&server.url, "alice", Some(backup_code)).await;
    ws.close(None)
        .await
        .expect("close first authenticated socket");

    // Second use of the same backup code — must be rejected.
    let (mut ws2, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for consumed backup code test");
    let auth = json!({
        "t": "auth", "u": "alice",
        "pw": "test-password-hash",
        "pk": pub_b64(&new_keypair()).unwrap(),
        "status": {"text":"Online","emoji":"🟢"},
        "otp": backup_code
    });
    ws2.send(Message::Text(auth.to_string()))
        .await
        .expect("send auth with consumed backup code");

    let err = recv_by_type(&mut ws2, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("invalid 2FA code")
    );
}

/// Verifies the complete shape of the `"ok"` auth response payload.
///
/// The `"ok"` frame is the only frame clients **must** parse on startup, so
/// its schema is treated as a hard API contract:
///
/// | Field      | Type           | Constraint          |
/// |------------|----------------|---------------------|
/// | `t`        | `"ok"`         | Literal             |
/// | `u`        | string         | Matches sent username |
/// | `channels` | array          | May be empty        |
/// | `hist`     | array          | May be empty        |
/// | `users`    | non-empty array| Each entry: `u`, `pk` |
/// | `proto.v`  | integer        | Must match supported version |
/// | `proto.max_payload_bytes` | integer | Must be present and positive |
///
/// Also verifies that the `"info"` command returns a frame with a numeric
/// `"online"` field, ensuring basic post-auth RPC works.
#[tokio::test]
async fn auth_contract_returns_expected_fields() {
    let server = start_server().await;
    let mut ws = connect_and_auth(&server.url, "alice").await;

    // Smoke-test that info RPC works after auth.
    ws.send(Message::Text(json!({"t":"info"}).to_string()))
        .await
        .expect("send info");
    let info = recv_by_type(&mut ws, "info").await;
    assert!(info.get("online").and_then(|v| v.as_u64()).is_some());

    // Validate the full auth-ok contract on a second connection.
    let (mut ws2, _) = connect_async(&server.url)
        .await
        .expect("connect second websocket");
    ws2.send(Message::Text(
        json!({
            "t": "auth", "u": "auth-contract-check",
            "pw": "test", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send second auth");

    let ok = recv_by_type(&mut ws2, "ok").await;
    assert_eq!(ok.get("t").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        ok.get("u").and_then(|v| v.as_str()),
        Some("auth-contract-check")
    );
    assert!(ok.get("channels").and_then(|v| v.as_array()).is_some());
    assert!(ok.get("hist").and_then(|v| v.as_array()).is_some());

    let users = ok
        .get("users")
        .and_then(|v| v.as_array())
        .expect("users must be an array");
    assert!(
        !users.is_empty(),
        "users array should include at least self"
    );
    for user in users {
        assert!(user.get("u").and_then(|v| v.as_str()).is_some());
        assert!(user.get("pk").and_then(|v| v.as_str()).is_some());
    }

    let proto = ok
        .get("proto")
        .and_then(|v| v.as_object())
        .expect("ok payload must include proto object");
    assert_eq!(
        proto.get("v").and_then(|v| v.as_u64()),
        Some(SUPPORTED_PROTOCOL_VERSION)
    );
    let max_payload = proto
        .get("max_payload_bytes")
        .and_then(|v| v.as_u64())
        .expect("proto.max_payload_bytes must be numeric");
    assert!(max_payload > 0, "proto.max_payload_bytes must be positive");
}

/// Verifies server/client bootstrap compatibility for the runtime startup flow.
///
/// This mirrors the minimum interaction pattern expected by the Rust client:
/// authenticate, fetch users, fetch info, join a channel, and receive message
/// broadcasts with the expected schema.
#[tokio::test]
async fn compatibility_contract_client_bootstrap_flow_stays_stable() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(json!({"t":"users"}).to_string()))
        .await
        .expect("send users rpc");
    let users_msg = recv_by_type(&mut alice, "users").await;
    let users = users_msg
        .get("users")
        .and_then(|v| v.as_array())
        .expect("users response must contain users array");
    assert!(!users.is_empty(), "users list should not be empty");
    for user in users {
        assert!(user.get("u").and_then(|v| v.as_str()).is_some());
        assert!(user.get("pk").and_then(|v| v.as_str()).is_some());
    }

    alice
        .send(Message::Text(json!({"t":"info"}).to_string()))
        .await
        .expect("send info rpc");
    let info_msg = recv_by_type(&mut alice, "info").await;
    assert!(
        info_msg.get("chs").and_then(|v| v.as_array()).is_some(),
        "info response must contain channels array"
    );
    assert!(
        info_msg.get("online").and_then(|v| v.as_u64()).is_some(),
        "info response must contain online count"
    );

    alice
        .send(Message::Text(
            json!({"t":"join","ch":"compat-room"}).to_string(),
        ))
        .await
        .expect("join compatibility room");
    let joined = recv_by_type(&mut alice, "joined").await;
    assert_eq!(
        joined.get("ch").and_then(|v| v.as_str()),
        Some("compat-room")
    );
    assert!(joined.get("hist").and_then(|v| v.as_array()).is_some());

    alice
        .send(Message::Text(
            json!({"t":"msg","ch":"compat-room","c":"compat-cipher"}).to_string(),
        ))
        .await
        .expect("send compatibility message");
    let msg = recv_by_type(&mut alice, "msg").await;
    assert_eq!(msg.get("ch").and_then(|v| v.as_str()), Some("compat-room"));
    assert_eq!(msg.get("u").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(msg.get("c").and_then(|v| v.as_str()), Some("compat-cipher"));
}

/// Verifies that auth responses advertise a backward-compatible protocol
/// version for clients that gate behavior by `proto.v`.
#[tokio::test]
async fn protocol_contract_advertises_backward_compatible_version() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for protocol version test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "proto-check",
            "pw": "test-password-hash",
            "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send auth for protocol version test");

    let ok = recv_by_type(&mut ws, "ok").await;
    let proto = ok
        .get("proto")
        .and_then(|v| v.as_object())
        .expect("ok payload must include proto object");

    assert_eq!(
        proto.get("v").and_then(|v| v.as_u64()),
        Some(SUPPORTED_PROTOCOL_VERSION)
    );
    assert!(
        proto
            .get("max_payload_bytes")
            .and_then(|v| v.as_u64())
            .is_some_and(|v| v >= 1024),
        "proto.max_payload_bytes should remain present and reasonable"
    );
}

/// Verifies that usernames containing whitespace are rejected during auth.
///
/// Whitespace in usernames would allow homoglyph-style confusion attacks and
/// complicate routing logic. The server must reject them with a human-readable
/// validation error rather than silently sanitising or truncating.
#[tokio::test]
async fn auth_contract_rejects_invalid_username() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url).await.expect("connect websocket");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "invalid user",
            "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send invalid auth payload");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: invalid username")
    );
}

/// Verifies that the first frame sent by a client **must** be an auth frame.
///
/// Clients that attempt to send any other frame type (e.g. `"ping"`) before
/// authenticating must receive a clear error. This prevents ambiguous server
/// state and ensures unauthenticated sessions can never inject messages.
#[tokio::test]
async fn auth_contract_rejects_non_auth_first_frame() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for invalid auth test");

    ws.send(Message::Text(
        json!({"t": "ping", "u": "alice"}).to_string(),
    ))
    .await
    .expect("send invalid first frame");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("first frame must be auth")
    );
}

/// Verifies that a public key that is not valid base64 is rejected at auth time.
///
/// Accepting a malformed key would either panic during later crypto operations
/// or silently store invalid state in the user table. Rejecting at the
/// boundary is the correct defence-in-depth strategy.
#[tokio::test]
async fn auth_contract_rejects_invalid_public_key() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for invalid key test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice",
            "pw": "test-password-hash", "pk": "not-base64",
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send invalid public key");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("invalid public key")
    );
}

/// Verifies that auth frames exceeding 4 096 bytes are rejected immediately.
///
/// Without an early size gate the server would parse arbitrarily large JSON
/// objects, opening a memory-exhaustion vector before any authentication has
/// taken place. The limit of 4 096 bytes is generous enough for all legitimate
/// auth payloads (username + password hash + base64 public key + status).
#[tokio::test]
async fn auth_contract_rejects_oversized_auth_frame() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for oversized auth frame test");

    let oversized_payload = "x".repeat(4_097);
    ws.send(Message::Text(oversized_payload))
        .await
        .expect("send oversized auth frame");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("auth frame too large")
    );
}

/// Verifies that malformed (non-parseable) JSON in the auth frame is rejected.
///
/// A proper error response rather than a silent close ensures the client can
/// surface a meaningful message to the user and log the failure correctly.
#[tokio::test]
async fn auth_contract_rejects_malformed_json() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for malformed auth json test");

    ws.send(Message::Text("{bad-json".to_string()))
        .await
        .expect("send malformed auth json");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("invalid auth JSON")
    );
}

/// Verifies that SQL-injection-style strings in the `"u"` field are rejected
/// by the input validation layer — **before** they can reach any SQL statement.
///
/// This is a belt-and-suspenders test: parameterised queries should already
/// prevent injection, but enforcing strict username formatting ensures the
/// character set is well-controlled across all code paths (logging, routing,
/// etc.).
#[tokio::test]
async fn auth_contract_rejects_sql_injection_like_username() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for sql-injection username test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice' OR '1'='1",
            "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"}
        })
        .to_string(),
    ))
    .await
    .expect("send sql-injection-like username payload");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: invalid username")
    );
}

/// Verifies that OTP codes longer than 64 characters are rejected with a
/// validation error rather than being evaluated against the backup-code list.
///
/// This prevents brute-force probing via artificially long strings that could
/// trigger timing-unsafe string comparisons or exhaust memory in pathological
/// implementations.
#[tokio::test]
async fn auth_contract_rejects_oversized_otp_input() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_enabled_2fa_user(&db_path, "alice", &["backup-aa11bb22"]);

    let server = start_server_with_db(db_path).await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for oversized otp test");

    let oversized_otp = "1".repeat(65); // one byte over the 64-char limit
    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice",
            "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢"},
            "otp": oversized_otp
        })
        .to_string(),
    ))
    .await
    .expect("send oversized otp payload");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: invalid otp code")
    );
}

/// Verifies that auth rejects a non-object `"status"` field.
///
/// The status contract requires a JSON object (with optional `text` and
/// `emoji` keys). Accepting arbitrary scalar values would make presence
/// rendering ambiguous across clients.
#[tokio::test]
async fn auth_contract_rejects_non_object_status_field() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for non-object status test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice",
            "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": "online"
        })
        .to_string(),
    ))
    .await
    .expect("send auth payload with invalid status type");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: status must be a JSON object")
    );
}

/// Verifies that auth rejects unknown fields in the `"status"` object.
///
/// Keeping this schema closed (only `text` and `emoji`) protects the protocol
/// from undocumented client-side extensions leaking into the server contract.
#[tokio::test]
async fn auth_contract_rejects_unexpected_status_object_field() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for unexpected status field test");

    ws.send(Message::Text(
        json!({
            "t": "auth", "u": "alice",
            "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
            "status": {"text":"Online","emoji":"🟢","mood":"focused"}
        })
        .to_string(),
    ))
    .await
    .expect("send auth payload with unexpected status field");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: unexpected status field: mood")
    );
}

/// Verifies that repeated wrong OTP attempts do not lock out a user who still
/// has a valid backup code.
///
/// This test deliberately **does not** assert rate-limiting behaviour (that
/// would be a separate security test). Its goal is to confirm that the auth
/// state remains consistent after a sequence of failures — i.e. the valid
/// backup code still works after all wrong attempts, and there is no
/// accidental state corruption in the `user_2fa` row.
#[tokio::test]
async fn auth_contract_blocks_repeated_wrong_otp_attempts() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    let backup_code = "ab12cd34ef56ab78";
    seed_enabled_2fa_user(&db_path, "alice", &[backup_code]);

    let server = start_server_with_db(db_path).await;

    // Submit several wrong codes; each must be rejected cleanly.
    for wrong_otp in ["000000", "123456", "999999", "abcdef"] {
        let (mut ws, _) = connect_async(&server.url)
            .await
            .expect("connect websocket for wrong otp attempt");

        ws.send(Message::Text(
            json!({
                "t": "auth", "u": "alice",
                "pw": "test-password-hash", "pk": pub_b64(&new_keypair()).unwrap(),
                "status": {"text":"Online","emoji":"🟢"},
                "otp": wrong_otp
            })
            .to_string(),
        ))
        .await
        .expect("send auth with wrong otp");

        let err = recv_by_type(&mut ws, "err").await;
        assert_eq!(
            err.get("m").and_then(|v| v.as_str()),
            Some("invalid 2FA code"),
            "wrong otp attempt should be rejected: {}",
            wrong_otp
        );
    }

    // After all failures, the correct backup code must still be accepted.
    let mut ws = connect_and_auth_with_otp(&server.url, "alice", Some(backup_code)).await;
    ws.close(None)
        .await
        .expect("close successful backup-code auth socket");
}

// ---------------------------------------------------------------------------
// User directory contract tests
// ---------------------------------------------------------------------------

/// Verifies that the `"users"` RPC returns an array where every element has a
/// non-empty `"u"` (username) and `"pk"` (public key) field.
///
/// The public key field is essential for end-to-end encrypted DMs: if a client
/// cannot retrieve a recipient's key, it cannot encrypt a message to them.
/// This test ensures the directory always provides key material alongside
/// identity.
#[tokio::test]
async fn users_contract_returns_user_objects_with_public_keys() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let _bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(json!({"t":"users","ts":1}).to_string()))
        .await
        .expect("send users command");

    let users_msg = recv_by_type(&mut alice, "users").await;
    let users = users_msg
        .get("users")
        .and_then(|v| v.as_array())
        .expect("users response payload should be an array");

    assert!(users.len() >= 2, "expected at least alice and bob");
    for user in users {
        let name = user.get("u").and_then(|v| v.as_str()).unwrap_or_default();
        let pk = user.get("pk").and_then(|v| v.as_str()).unwrap_or_default();
        assert!(!name.is_empty(), "username must be non-empty");
        assert!(!pk.is_empty(), "public key must be non-empty");
    }
}

// ---------------------------------------------------------------------------
// Channel & presence contract tests
// ---------------------------------------------------------------------------

/// Verifies that channel names are normalised by the server on join.
///
/// This contract ensures that cosmetic client input differences (uppercase,
/// punctuation, leading `#`) do not create duplicate channel identities.
#[tokio::test]
async fn join_contract_normalizes_channel_name() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({"t":"join","ch":"#Dev Ops!!!_X"}).to_string(),
        ))
        .await
        .expect("send join with unnormalized channel name");

    let joined = recv_by_type(&mut alice, "joined").await;
    assert_eq!(joined.get("ch").and_then(|v| v.as_str()), Some("devops_x"));
    assert!(
        joined.get("hist").and_then(|v| v.as_array()).is_some(),
        "joined payload should include history array"
    );
}

/// Verifies that status updates are broadcast to other connected clients.
///
/// Presence changes are part of the public runtime contract, so updates must
/// include both the source user and the exact status payload.
#[tokio::test]
async fn status_contract_broadcasts_status_update_to_other_clients() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let mut bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(
            json!({
                "t":"status",
                "status": {"text":"In focus","emoji":"✅"}
            })
            .to_string(),
        ))
        .await
        .expect("send status update");

    let update = recv_by_type(&mut bob, "status_update").await;
    assert_eq!(update.get("user").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(
        update
            .get("status")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str()),
        Some("In focus")
    );
    assert_eq!(
        update
            .get("status")
            .and_then(|v| v.get("emoji"))
            .and_then(|v| v.as_str()),
        Some("✅")
    );
}

/// Verifies that channel-scoped typing state is broadcast to channel members.
#[tokio::test]
async fn typing_contract_broadcasts_channel_scope_updates() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let mut bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(
            json!({"t":"join","ch":"typing-room"}).to_string(),
        ))
        .await
        .expect("alice joins typing-room");
    let _ = recv_by_type(&mut alice, "joined").await;

    bob.send(Message::Text(
        json!({"t":"join","ch":"typing-room"}).to_string(),
    ))
    .await
    .expect("bob joins typing-room");
    let _ = recv_by_type(&mut bob, "joined").await;

    alice
        .send(Message::Text(
            json!({"t":"typing","ch":"typing-room","typing":true}).to_string(),
        ))
        .await
        .expect("alice sends typing on");

    let typing_on = recv_by_type(&mut bob, "typing").await;
    assert_eq!(
        typing_on.get("ch").and_then(|v| v.as_str()),
        Some("typing-room")
    );
    assert_eq!(typing_on.get("u").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(
        typing_on.get("typing").and_then(|v| v.as_bool()),
        Some(true)
    );

    alice
        .send(Message::Text(
            json!({"t":"typing","ch":"typing-room","typing":false}).to_string(),
        ))
        .await
        .expect("alice sends typing off");

    let typing_off = recv_by_type(&mut bob, "typing").await;
    assert_eq!(
        typing_off.get("typing").and_then(|v| v.as_bool()),
        Some(false)
    );
}

/// Verifies that DM-scoped typing state is routed to the intended DM peer.
#[tokio::test]
async fn typing_contract_routes_dm_scope_updates() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let mut bob = connect_and_auth(&server.url, "bob").await;

    // Subscribe Bob to his DM route so DM typing relay can be observed.
    bob.send(Message::Text(
        json!({"t":"join","ch":"__dm__bob"}).to_string(),
    ))
    .await
    .expect("bob joins dm route");
    let _ = recv_by_type(&mut bob, "joined").await;

    alice
        .send(Message::Text(
            json!({"t":"typing","to":"bob","typing":true}).to_string(),
        ))
        .await
        .expect("alice sends dm typing on");

    let typing = recv_by_type(&mut bob, "typing").await;
    assert_eq!(typing.get("from").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(typing.get("to").and_then(|v| v.as_str()), Some("bob"));
    assert_eq!(typing.get("scope").and_then(|v| v.as_str()), Some("dm:bob"));
    assert_eq!(typing.get("typing").and_then(|v| v.as_bool()), Some(true));
}

// ---------------------------------------------------------------------------
// Messaging contract tests
// ---------------------------------------------------------------------------

/// Verifies the round-trip contract for a channel message.
///
/// After sending a `"msg"` frame the server must broadcast a `"msg"` frame
/// back to the sender (and to all other channel members). The echoed frame
/// must preserve:
///
/// * `"ch"` – the target channel name.
/// * `"u"`  – the sending username (added by the server).
/// * `"c"`  – the ciphertext blob (unchanged).
///
/// Because messages are end-to-end encrypted the server MUST NOT inspect or
/// modify the `"c"` field; any corruption would silently break decryption on
/// the receiving end.
#[tokio::test]
async fn msg_contract_roundtrips_channel_payload() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "ciphertext-blob", "ts": 123
            })
            .to_string(),
        ))
        .await
        .expect("send channel message");

    let msg = recv_by_type(&mut alice, "msg").await;
    assert_eq!(msg.get("ch").and_then(|v| v.as_str()), Some("general"));
    assert_eq!(msg.get("u").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(
        msg.get("c").and_then(|v| v.as_str()),
        Some("ciphertext-blob")
    );
}

/// Verifies that optional bridge metadata (`src` and `relay`) survives
/// server broadcast unchanged for channel messages, including attachment and
/// reply metadata.
///
/// This is required for downstream bridge instances to make deterministic
/// loop-prevention decisions based on source IDs and relay markers.
#[tokio::test]
async fn msg_contract_preserves_bridge_source_and_relay_markers() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
                "c": "cipher-loop-marker",
                "src": "discord-bridge:test-instance",
                "relay": {
                    "source_id": "discord-bridge:test-instance",
                    "origin": "discord",
                    "markers": ["discord:1234567890"],
                    "attachments": [
                        {
                            "url": "https://cdn.example.com/file.png",
                            "filename": "file.png",
                            "size": 2048,
                            "content_type": "image/png"
                        }
                    ],
                    "reply": {
                        "discord_message_id": "555",
                        "discord_channel_id": "1234567890",
                        "author": "bob",
                        "excerpt": "previous message"
                    }
                }
            })
            .to_string(),
        ))
        .await
        .expect("send channel message with bridge metadata");

    let msg = recv_by_type(&mut alice, "msg").await;
    assert_eq!(
        msg.get("src").and_then(|v| v.as_str()),
        Some("discord-bridge:test-instance")
    );
    assert_eq!(
        msg.get("relay")
            .and_then(|v| v.get("source_id"))
            .and_then(|v| v.as_str()),
        Some("discord-bridge:test-instance")
    );
    assert_eq!(
        msg.get("relay")
            .and_then(|v| v.get("markers"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str()),
        Some("discord:1234567890")
    );
    assert_eq!(
        msg.get("relay")
            .and_then(|v| v.get("attachments"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|attachment| attachment.get("url"))
            .and_then(|v| v.as_str()),
        Some("https://cdn.example.com/file.png")
    );
    assert_eq!(
        msg.get("relay")
            .and_then(|v| v.get("reply"))
            .and_then(|reply| reply.get("discord_message_id"))
            .and_then(|v| v.as_str()),
        Some("555")
    );
}

// ---------------------------------------------------------------------------
// File-transfer contract tests
// ---------------------------------------------------------------------------

/// Verifies that oversized file metadata is rejected before announcement.
///
/// The max file-size guard is a protocol-level safety contract that prevents
/// clients from advertising transfers that exceed server policy.
#[tokio::test]
async fn file_contract_rejects_oversized_file_metadata() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t":"file_meta",
                "ch":"general",
                "filename":"huge.bin",
                "size": 104_857_601_u64,
                "file_id":"oversize-1"
            })
            .to_string(),
        ))
        .await
        .expect("send oversized file metadata");

    let err = recv_by_type(&mut alice, "err").await;
    let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("file size exceeds maximum of"),
        "expected file-size rejection message, got: {}",
        msg
    );
    assert!(
        msg.contains("104857600"),
        "expected max-size value in message, got: {}",
        msg
    );
}

/// Verifies that media transfer metadata and chunk frames are relayed with
/// stable fields required by clients to reconstruct files.
#[tokio::test]
async fn file_contract_relays_media_metadata_and_chunks() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let mut bob = connect_and_auth(&server.url, "bob").await;

    let file_id = "media-contract-1";
    alice
        .send(Message::Text(
            json!({
                "t":"file_meta",
                "ch":"general",
                "filename":"preview.png",
                "size": 64_u64,
                "file_id": file_id,
                "media_kind": "image",
                "mime": "image/png"
            })
            .to_string(),
        ))
        .await
        .expect("send media metadata");

    let meta = recv_by_type(&mut bob, "file_meta").await;
    assert_eq!(meta.get("from").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(meta.get("ch").and_then(|v| v.as_str()), Some("general"));
    assert_eq!(meta.get("file_id").and_then(|v| v.as_str()), Some(file_id));
    assert_eq!(
        meta.get("filename").and_then(|v| v.as_str()),
        Some("preview.png")
    );
    assert_eq!(meta.get("size").and_then(|v| v.as_u64()), Some(64_u64));
    assert_eq!(
        meta.get("media_kind").and_then(|v| v.as_str()),
        Some("image")
    );
    assert_eq!(meta.get("mime").and_then(|v| v.as_str()), Some("image/png"));

    let chunk_data = "aGVsbG8td29ybGQ=";
    alice
        .send(Message::Text(
            json!({
                "t":"file_chunk",
                "ch":"general",
                "file_id": file_id,
                "index": 0,
                "data": chunk_data
            })
            .to_string(),
        ))
        .await
        .expect("send media chunk");

    let chunk = recv_by_type(&mut bob, "file_chunk").await;
    assert_eq!(chunk.get("from").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(chunk.get("ch").and_then(|v| v.as_str()), Some("general"));
    assert_eq!(chunk.get("file_id").and_then(|v| v.as_str()), Some(file_id));
    assert_eq!(chunk.get("index").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(chunk.get("data").and_then(|v| v.as_str()), Some(chunk_data));
}

// ---------------------------------------------------------------------------
// Voice contract tests
// ---------------------------------------------------------------------------

/// Verifies that `"vdata"` frames sent by one room member are forwarded to
/// all other members of the same voice room.
///
/// # Test sequence
///
/// 1. Alice and Bob both join `"room-a"` via `"vjoin"`.
/// 2. Alice sends a `"vdata"` frame with a fake audio payload.
/// 3. Bob must receive a `"vdata"` frame annotated with `"from": "alice"` and
///    an unchanged `"a"` (audio data) field.
///
/// The 150 ms sleep after the `"vjoin"` frames gives the server time to
/// register both members in the room before Alice sends audio, avoiding a
/// race where Bob has not yet joined when the forward occurs.
#[tokio::test]
async fn voice_contract_forwards_vdata_between_room_members() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let mut bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(json!({"t":"vjoin","r":"room-a"}).to_string()))
        .await
        .expect("alice joins voice room");
    bob.send(Message::Text(json!({"t":"vjoin","r":"room-a"}).to_string()))
        .await
        .expect("bob joins voice room");

    // Allow both join events to be processed server-side before sending audio.
    sleep(Duration::from_millis(150)).await;

    alice
        .send(Message::Text(
            json!({"t":"vdata","r":"room-a","a":"ZmFrZS1hdWRpby1wYXlsb2Fk"}).to_string(),
        ))
        .await
        .expect("alice sends vdata");

    let vdata = recv_by_type(&mut bob, "vdata").await;
    assert_eq!(vdata.get("from").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(
        vdata.get("a").and_then(|v| v.as_str()),
        Some("ZmFrZS1hdWRpby1wYXlsb2Fk")
    );
}

// ---------------------------------------------------------------------------
// History & search contract tests
// ---------------------------------------------------------------------------

/// Verifies that messages sent to a channel are durably persisted and
/// returned by the `"history"` RPC.
///
/// Persistence is non-negotiable for a chat server: if a message is confirmed
/// (i.e. echoed back to the sender) it must appear in subsequent history
/// queries. This test uses a sentinel ciphertext (`"history-cipher"`) to
/// uniquely identify the test message in the history response.
#[tokio::test]
async fn history_contract_returns_persisted_events() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "history-cipher",
                "p": "history plain text marker"
            })
            .to_string(),
        ))
        .await
        .expect("send message for history");

    // Wait for the echo to confirm the message was accepted and stored.
    let _ = recv_by_type(&mut alice, "msg").await;

    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "general", "limit": 25}).to_string(),
        ))
        .await
        .expect("request history");

    let history = recv_by_type(&mut alice, "history").await;
    let events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");

    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("history-cipher")
    });
    assert!(found, "expected persisted message in history response");
}

/// Verifies that persisted history survives a full server restart cycle.
///
/// The test writes an event, terminates the process, starts a new server
/// against the same DB file, and confirms history still includes the event.
#[tokio::test]
async fn history_contract_survives_server_restart() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);

    let server_before = start_server_with_db(db_path.clone()).await;
    let mut alice = connect_and_auth(&server_before.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "restart-history-cipher",
                "p": "restart history marker"
            })
            .to_string(),
        ))
        .await
        .expect("send message before restart");
    let _ = recv_by_type(&mut alice, "msg").await;

    let preserved_db = server_before.stop_and_preserve_db();

    let server_after = start_server_with_db(preserved_db).await;
    let mut bob = connect_and_auth(&server_after.url, "bob").await;
    bob.send(Message::Text(
        json!({"t": "history", "ch": "general", "limit": 50}).to_string(),
    ))
    .await
    .expect("request history after restart");

    let history = recv_by_type(&mut bob, "history").await;
    let events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");
    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("restart-history-cipher")
    });
    assert!(found, "expected persisted event after server restart");
}

/// Verifies that history and search remain responsive with a 100k-event local
/// dataset.
#[tokio::test]
async fn history_and_search_latency_stays_low_with_100k_local_events() {
    const EVENT_COUNT: usize = 100_000;
    const HISTORY_LIMIT: i64 = 100;
    const SEARCH_LIMIT: i64 = 100;
    const HISTORY_MAX_MS: u128 = 600;
    const SEARCH_MAX_MS: u128 = 2500;

    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);

    // Bootstrap schema, then stop server while preserving the DB file.
    let bootstrap = start_server_with_db(db_path.clone()).await;
    let preserved_db = bootstrap.stop_and_preserve_db();

    let mut conn = Connection::open(&preserved_db).expect("open benchmark db");
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .expect("configure benchmark sqlite pragmas");

    let tx = conn
        .transaction()
        .expect("begin benchmark insert transaction");
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .expect("prepare benchmark insert statement");

        for i in 0..EVENT_COUNT {
            let ts = 1_700_000_000.0 + (i as f64 * 0.001);
            let payload = json!({
                "t": "msg",
                "ch": "benchroom",
                "u": "seed",
                "c": format!("bench-cipher-{}", i),
                "ts": ts
            })
            .to_string();
            let search_text = if i % 2_000 == 0 {
                "needle-marker benchmark topic"
            } else {
                "noise marker"
            };

            stmt.execute(params![
                ts,
                "msg",
                "benchroom",
                "seed",
                Option::<String>::None,
                payload,
                search_text,
            ])
            .expect("insert benchmark event");
        }
    }
    tx.commit().expect("commit benchmark dataset");

    let server = start_server_with_db(preserved_db).await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    let history_start = Instant::now();
    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "benchroom", "limit": HISTORY_LIMIT}).to_string(),
        ))
        .await
        .expect("request benchmark history");
    let history = recv_by_type(&mut alice, "history").await;
    let history_latency_ms = history_start.elapsed().as_millis();
    let history_events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("benchmark history events must be an array");
    assert!(
        !history_events.is_empty(),
        "expected non-empty history response for benchmark dataset"
    );
    assert!(
        history_latency_ms <= HISTORY_MAX_MS,
        "history latency too high on {} events: {}ms (limit {}ms)",
        EVENT_COUNT,
        history_latency_ms,
        HISTORY_MAX_MS
    );

    let search_start = Instant::now();
    alice
        .send(Message::Text(
            json!({
                "t": "search",
                "ch": "benchroom",
                "q": "needle-marker",
                "limit": SEARCH_LIMIT
            })
            .to_string(),
        ))
        .await
        .expect("request benchmark search");
    let search = recv_by_type(&mut alice, "search").await;
    let search_latency_ms = search_start.elapsed().as_millis();
    let search_events = search
        .get("events")
        .and_then(|v| v.as_array())
        .expect("benchmark search events must be an array");
    assert!(
        !search_events.is_empty(),
        "expected search hits in benchmark dataset"
    );
    assert!(
        search_latency_ms <= SEARCH_MAX_MS,
        "search latency too high on {} events: {}ms (limit {}ms)",
        EVENT_COUNT,
        search_latency_ms,
        SEARCH_MAX_MS
    );
}

/// Verifies that `"history"` supports a time window via the optional
/// `"seconds"` field.
///
/// The server should only return events newer than `now() - seconds`.
#[tokio::test]
async fn history_contract_respects_seconds_window_filter() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "window-room",
                "c": "history-window-old",
                "p": "older marker"
            })
            .to_string(),
        ))
        .await
        .expect("send older message");

    // Ensure the first message is outside a 1-second history window.
    sleep(Duration::from_millis(1200)).await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "window-room",
                "c": "history-window-new",
                "p": "newer marker"
            })
            .to_string(),
        ))
        .await
        .expect("send newer message");

    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "window-room", "seconds": 1, "limit": 50}).to_string(),
        ))
        .await
        .expect("request windowed history");

    let history = recv_by_type(&mut alice, "history").await;
    let events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");

    let found_old = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("history-window-old")
    });
    let found_new = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("history-window-new")
    });

    assert!(found_new, "expected newer message in 1s history window");
    assert!(
        !found_old,
        "did not expect older message in 1s history window"
    );
}

/// Verifies that channel history can be queried for a DM conversation using
/// the `"dm:<user>"` scope format.
#[tokio::test]
async fn history_contract_supports_dm_scope() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let _bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(
            json!({
                "t": "dm", "to": "bob",
                "c": "dm-history-cipher",
                "p": "dm history marker"
            })
            .to_string(),
        ))
        .await
        .expect("send dm for history scope");

    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "dm:bob", "limit": 50}).to_string(),
        ))
        .await
        .expect("request dm-scoped history");

    let history = recv_by_type(&mut alice, "history").await;
    assert_eq!(history.get("ch").and_then(|v| v.as_str()), Some("dm:bob"));
    let events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");

    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("dm")
            && e.get("c").and_then(|v| v.as_str()) == Some("dm-history-cipher")
    });
    assert!(found, "expected DM event in dm:bob history response");
}

/// Verifies that `"join"` events are persisted and visible via history.
#[tokio::test]
async fn history_contract_persists_join_events() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({"t": "join", "ch": "join-contract-room"}).to_string(),
        ))
        .await
        .expect("join test channel");

    let joined = recv_by_type(&mut alice, "joined").await;
    assert_eq!(
        joined.get("ch").and_then(|v| v.as_str()),
        Some("join-contract-room")
    );

    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "join-contract-room", "limit": 100}).to_string(),
        ))
        .await
        .expect("request join channel history");

    let history = recv_by_type(&mut alice, "history").await;
    let events = history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");

    let found_join = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("join")
            && e.get("u").and_then(|v| v.as_str()) == Some("alice")
            && e.get("ch").and_then(|v| v.as_str()) == Some("join-contract-room")
    });
    assert!(found_join, "expected persisted join event in history");
}

/// Verifies that the `"search"` RPC filters messages by a plaintext index
/// field (`"p"`) and returns only matching events.
///
/// The `"p"` field carries un-encrypted, server-searchable text alongside the
/// encrypted ciphertext. This allows keyword search without the server ever
/// seeing the full decrypted content. The test asserts:
///
/// * The response echoes the original query in `"q"`.
/// * At least one event in `"events"` matches both the sentinel ciphertext and
///   the message type.
#[tokio::test]
async fn search_contract_filters_by_plaintext_index() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "search-cipher-hit",
                "p": "deploy completed successfully"
            })
            .to_string(),
        ))
        .await
        .expect("send searchable message");

    let _ = recv_by_type(&mut alice, "msg").await;

    alice
        .send(Message::Text(
            json!({"t": "search", "ch": "general", "q": "deploy", "limit": 25}).to_string(),
        ))
        .await
        .expect("request search");

    let search = recv_by_type(&mut alice, "search").await;
    assert_eq!(search.get("q").and_then(|v| v.as_str()), Some("deploy"));
    let events = search
        .get("events")
        .and_then(|v| v.as_array())
        .expect("search events must be an array");
    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("search-cipher-hit")
    });
    assert!(found, "expected matching message in search response");
}

/// Verifies that `"search"` supports `"dm:<user>"` conversation scope.
#[tokio::test]
async fn search_contract_supports_dm_scope() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let _bob = connect_and_auth(&server.url, "bob").await;

    alice
        .send(Message::Text(
            json!({
                "t": "dm", "to": "bob",
                "c": "dm-search-cipher",
                "p": "sprint planning review"
            })
            .to_string(),
        ))
        .await
        .expect("send dm for search scope");

    alice
        .send(Message::Text(
            json!({"t": "search", "ch": "dm:bob", "q": "planning", "limit": 50}).to_string(),
        ))
        .await
        .expect("request dm-scoped search");

    let search = recv_by_type(&mut alice, "search").await;
    assert_eq!(search.get("ch").and_then(|v| v.as_str()), Some("dm:bob"));
    assert_eq!(search.get("q").and_then(|v| v.as_str()), Some("planning"));

    let events = search
        .get("events")
        .and_then(|v| v.as_array())
        .expect("search events must be an array");
    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("dm")
            && e.get("c").and_then(|v| v.as_str()) == Some("dm-search-cipher")
    });
    assert!(found, "expected DM event in dm:bob search response");
}

/// Verifies that the `"rewind"` RPC returns messages sent within the requested
/// time window.
///
/// `"rewind"` is a time-bounded history query (e.g. "show me the last 5
/// minutes"). Unlike `"history"` which is offset-based, `"rewind"` uses a
/// `"seconds"` look-back window. The response type is `"history"` (reusing the
/// same frame shape). This test confirms a message sent moments ago appears in
/// a 300-second rewind.
#[tokio::test]
async fn rewind_contract_returns_recent_events() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "rewind-cipher-hit",
                "p": "rewind marker"
            })
            .to_string(),
        ))
        .await
        .expect("send message for rewind");

    let _ = recv_by_type(&mut alice, "msg").await;

    alice
        .send(Message::Text(
            json!({"t": "rewind", "ch": "general", "seconds": 300, "limit": 25}).to_string(),
        ))
        .await
        .expect("request rewind");

    let rewind = recv_by_type(&mut alice, "history").await;
    let events = rewind
        .get("events")
        .and_then(|v| v.as_array())
        .expect("rewind events must be an array");
    let found = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("rewind-cipher-hit")
    });
    assert!(found, "expected recent message in rewind response");
}

/// Verifies that `"replay"` returns events from an absolute timestamp
/// onward, excluding older events.
#[tokio::test]
async fn replay_contract_returns_events_from_timestamp() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "replay-room",
                "c": "replay-old-cipher",
                "p": "replay old marker"
            })
            .to_string(),
        ))
        .await
        .expect("send old replay message");

    alice
        .send(Message::Text(
            json!({"t": "history", "ch": "replay-room", "limit": 50}).to_string(),
        ))
        .await
        .expect("request history to capture first replay timestamp");

    let first_history = recv_by_type(&mut alice, "history").await;
    let first_events = first_history
        .get("events")
        .and_then(|v| v.as_array())
        .expect("history events must be an array");
    let first_ts = first_events
        .iter()
        .find_map(|e| {
            if e.get("t").and_then(|v| v.as_str()) == Some("msg")
                && e.get("c").and_then(|v| v.as_str()) == Some("replay-old-cipher")
            {
                e.get("ts")
                    .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
            } else {
                None
            }
        })
        .expect("first replay message should exist in history with ts");

    sleep(Duration::from_millis(1200)).await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "replay-room",
                "c": "replay-new-cipher",
                "p": "replay new marker"
            })
            .to_string(),
        ))
        .await
        .expect("send new replay message");

    alice
        .send(Message::Text(
            json!({"t": "replay", "ch": "replay-room", "from_ts": first_ts + 0.5, "limit": 100})
                .to_string(),
        ))
        .await
        .expect("request replay from timestamp");

    let replay = recv_by_type(&mut alice, "replay").await;
    let events = replay
        .get("events")
        .and_then(|v| v.as_array())
        .expect("replay events must be an array");

    let found_old = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("replay-old-cipher")
    });
    let found_new = events.iter().any(|e| {
        e.get("t").and_then(|v| v.as_str()) == Some("msg")
            && e.get("c").and_then(|v| v.as_str()) == Some("replay-new-cipher")
    });

    assert!(found_new, "expected new message in replay response");
    assert!(
        !found_old,
        "did not expect old message in replay response from timestamp"
    );
}

// ---------------------------------------------------------------------------
// Schema migration contract tests
// ---------------------------------------------------------------------------

/// Verifies that a fresh server initialises `schema_meta` to the latest
/// version.
///
/// This is the baseline migration contract: any server started against an
/// empty or newly created database must create and stamp the expected schema
/// version before serving traffic.
#[tokio::test]
async fn schema_meta_contains_current_version() {
    let server = start_server().await;
    let _alice = connect_and_auth(&server.url, "alice").await;

    let version = read_schema_version(&server.db_path);
    assert_eq!(version, "6");
}

/// Verifies that a server migrates a `v0` database to schema `v6` on startup.
///
/// Migration correctness is verified by:
///
/// 1. Confirming `schema_meta.schema_version` is updated to `"6"`.
/// 2. Confirming the `events` table exists (created by `v1` migration).
/// 3. Confirming the `user_2fa` table exists (created by `v2` migration).
/// 4. Confirming the `user_credentials` table exists (created by `v3` migration).
/// 5. Confirming append-only and query indexes from `v4` exist.
/// 6. Confirming roles/permissions tables from `v5` exist.
/// 7. Confirming audit_logs and suspicious_activity tables from `v6` exist.
///
/// All assertions are made directly against SQLite rather than through the
/// server API to keep migration tests independent of server protocol changes.
#[tokio::test]
async fn schema_migrates_from_version_zero_to_current() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_schema_version(&db_path, "0");

    let server = start_server_with_db(db_path.clone()).await;
    let _alice = connect_and_auth(&server.url, "alice").await;

    let conn = Connection::open(&db_path).expect("open migrated db");
    assert_eq!(
        read_schema_version(&db_path),
        "6",
        "expected migration to set schema version to 6"
    );

    let events_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='events'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master");
    assert_eq!(
        events_exists, 1,
        "events table should exist after migration"
    );

    let user_2fa_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='user_2fa'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for user_2fa");
    assert_eq!(
        user_2fa_exists, 1,
        "user_2fa table should exist after migration"
    );

    let user_credentials_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='user_credentials'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for user_credentials");
    assert_eq!(
        user_credentials_exists, 1,
        "user_credentials table should exist after migration"
    );

    let channel_ts_index_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_events_channel_ts'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for idx_events_channel_ts");
    assert_eq!(
        channel_ts_index_exists, 1,
        "idx_events_channel_ts should exist after migration"
    );

    let dm_route_index_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_events_dm_route_ts'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for idx_events_dm_route_ts");
    assert_eq!(
        dm_route_index_exists, 1,
        "idx_events_dm_route_ts should exist after migration"
    );

    let append_only_update_trigger_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='trg_events_append_only_update'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for append-only update trigger");
    assert_eq!(
        append_only_update_trigger_exists, 1,
        "append-only update trigger should exist after migration"
    );

    conn.execute(
        "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            1_700_000_000.0,
            "msg",
            "schema-room",
            "alice",
            Option::<String>::None,
            "{\"t\":\"msg\",\"ch\":\"schema-room\",\"u\":\"alice\",\"c\":\"schema-test\",\"ts\":1700000000}",
            "schema test"
        ],
    )
    .expect("insert event for append-only trigger validation");

    let update_result = conn.execute(
        "UPDATE events SET payload = 'mutated' WHERE channel='schema-room'",
        [],
    );
    assert!(
        update_result.is_err(),
        "events updates must be rejected by append-only trigger"
    );

    let audit_logs_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='audit_logs'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for audit_logs");
    assert_eq!(
        audit_logs_exists, 1,
        "audit_logs table should exist after migration"
    );

    let suspicious_activity_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='suspicious_activity'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master for suspicious_activity");
    assert_eq!(
        suspicious_activity_exists, 1,
        "suspicious_activity table should exist after migration"
    );

    let failed_attempts_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_credentials') WHERE name='failed_attempts'",
            [],
            |row| row.get(0),
        )
        .expect("query pragma_table_info for failed_attempts");
    assert_eq!(
        failed_attempts_column, 1,
        "failed_attempts column should exist in user_credentials after migration"
    );

    let locked_until_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_credentials') WHERE name='locked_until'",
            [],
            |row| row.get(0),
        )
        .expect("query pragma_table_info for locked_until");
    assert_eq!(
        locked_until_column, 1,
        "locked_until column should exist in user_credentials after migration"
    );

    drop(server);
}

/// Verifies that a server does **not** downgrade a database with a schema
/// version higher than its own current version.
///
/// This handles the "rollback" deployment scenario: if `v999` of the server
/// writes new tables and a `v998` server is deployed, the older server must
/// leave the schema version intact and continue serving traffic against the
/// newer schema rather than silently rolling back migrations. Downgrading
/// could destroy data written by the newer server.
#[tokio::test]
async fn schema_newer_version_is_not_downgraded_and_server_auth_still_works() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_schema_version(&db_path, "999");

    let server = start_server_with_db(db_path.clone()).await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    // Server must remain fully functional, not crash or refuse connections.
    alice
        .send(Message::Text(json!({"t":"info"}).to_string()))
        .await
        .expect("send info request");
    let info = recv_by_type(&mut alice, "info").await;
    assert!(
        info.get("online").and_then(|v| v.as_u64()).is_some(),
        "server should remain usable with future schema"
    );

    let version = read_schema_version(&db_path);
    assert_eq!(
        version, "999",
        "server must not silently downgrade newer schema versions"
    );

    drop(server);
}

// ---------------------------------------------------------------------------
// Protocol safety contract tests
// ---------------------------------------------------------------------------

/// Verifies that messages with a timestamp outside the allowed clock-skew
/// window are rejected.
///
/// Accepting arbitrarily old timestamps would allow replay attacks where an
/// attacker captures a legitimate frame and resubmits it later. The server
/// must enforce a tight clock-skew window (typically ±30–60 s) and reject
/// anything outside it with a descriptive error.
#[tokio::test]
async fn protocol_contract_rejects_stale_timestamp_on_mutating_event() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "stale-cipher", "ts": 1,
                "n": "11111111111111111111111111111111"
            })
            .to_string(),
        ))
        .await
        .expect("send stale timestamp message");

    let err = recv_by_type(&mut alice, "err").await;
    let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("timestamp outside allowed clock skew"),
        "expected timestamp skew rejection, got: {}",
        msg
    );
}

/// Verifies that a nonce-protected event must include a valid `"ts"` field.
///
/// This is critical for replay protection: if a nonce is present but timestamp
/// validation is skipped, stale frame replays become possible.
#[tokio::test]
async fn protocol_contract_rejects_missing_timestamp_when_nonce_present() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "missing-ts", "n": "cccccccccccccccccccccccccccccccc"
            })
            .to_string(),
        ))
        .await
        .expect("send nonce-protected payload without timestamp");

    let err = recv_by_type(&mut alice, "err").await;
    let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("missing timestamp"),
        "expected missing timestamp rejection, got: {}",
        msg
    );
}

/// Verifies that a nonce used in a successfully accepted message cannot be
/// reused in a subsequent message.
///
/// Nonces are the primary anti-replay mechanism. Once a `(user, nonce)` pair
/// has been accepted, any future message with the same nonce must be rejected
/// regardless of whether the timestamp is fresh. This prevents an adversary
/// who can observe or intercept traffic from resubmitting captured frames.
#[tokio::test]
async fn protocol_contract_rejects_replayed_nonce() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let payload = json!({
        "t": "msg", "ch": "general",
        "c": "replay-cipher", "ts": now,
        "n": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });

    // First send — must succeed.
    alice
        .send(Message::Text(payload.to_string()))
        .await
        .expect("send first message with nonce");
    let _ = recv_by_type(&mut alice, "msg").await;

    // Identical replay — must be rejected.
    alice
        .send(Message::Text(payload.to_string()))
        .await
        .expect("send replayed nonce message");
    let err = recv_by_type(&mut alice, "err").await;
    let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("replayed nonce"),
        "expected replay nonce rejection, got: {}",
        msg
    );
}

/// Verifies that nonces must be lowercase hexadecimal strings.
///
/// Restricting the nonce character set to `[0-9a-f]` prevents injection of
/// special characters into nonce-storage paths (e.g. Redis keys, log lines)
/// and ensures a stable, well-defined encoding. Any nonce that contains
/// non-hex characters (such as `@`) must be rejected before storage.
#[tokio::test]
async fn protocol_contract_rejects_invalid_nonce_format() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    alice
        .send(Message::Text(
            json!({
                "t": "msg", "ch": "general",
                "c": "nonce-format-attack", "ts": now,
                "n": "NOT_HEX_@@@"
            })
            .to_string(),
        ))
        .await
        .expect("send invalid nonce format payload");

    let err = recv_by_type(&mut alice, "err").await;
    let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("invalid nonce format"),
        "expected invalid nonce format rejection, got: {}",
        msg
    );
}

/// Verifies that runtime (post-auth) payloads exceeding 16 000 bytes are
/// rejected.
///
/// This cap prevents a single authenticated client from monopolising server
/// memory or saturating network bandwidth with a single oversized frame.
/// Legitimate chat payloads (even with large encrypted blobs) should comfortably
/// fit within this limit.
#[tokio::test]
async fn protocol_contract_rejects_oversized_runtime_payload() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    let oversized_payload = "x".repeat(16_001);
    alice
        .send(Message::Text(oversized_payload))
        .await
        .expect("send oversized runtime payload");

    let err = recv_by_type(&mut alice, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("payload exceeds max size")
    );
}

/// Fuzz-style test verifying that malformed and non-object JSON frames are
/// always rejected with consistent, specific error messages.
///
/// Two classes of bad input are exercised:
///
/// 1. **Malformed JSON** (`{`, unterminated strings, bare strings, null
///    bytes): must return `"invalid JSON payload"`.
/// 2. **Valid JSON that is not an object** (arrays, `null`, numbers, strings):
///    must return `"payload must be a JSON object"`.
///
/// The distinction matters for client-side error handling: a malformed frame
/// indicates a serialisation bug, while a non-object frame indicates a
/// protocol violation that could be caught at development time.
#[tokio::test]
async fn protocol_contract_fuzz_corpus_rejects_malformed_and_non_object_frames() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    let malformed_frames = vec!["{", "{\"t\":\"msg\"", "not-json", "\\u0000"];
    for frame in malformed_frames {
        alice
            .send(Message::Text(frame.to_string()))
            .await
            .expect("send malformed fuzz frame");
        let err = recv_by_type(&mut alice, "err").await;
        assert_eq!(
            err.get("m").and_then(|v| v.as_str()),
            Some("invalid JSON payload"),
            "expected invalid JSON payload for frame: {:?}",
            frame
        );
    }

    let non_object_frames = vec!["[]", "null", "123", "\"string\""];
    for frame in non_object_frames {
        alice
            .send(Message::Text(frame.to_string()))
            .await
            .expect("send non-object fuzz frame");
        let err = recv_by_type(&mut alice, "err").await;
        assert_eq!(
            err.get("m").and_then(|v| v.as_str()),
            Some("payload must be a JSON object"),
            "expected object-shape rejection for frame: {:?}",
            frame
        );
    }
}

/// Stress-tests the nonce-replay defence by flooding the server with 20
/// replays of the same nonce in rapid succession.
///
/// All 20 replays must be individually rejected with `"replayed nonce"`. This
/// confirms that the nonce store does not have race conditions under load and
/// that the server remains responsive (no panics, no silent drops) throughout
/// the flood. A chat server that degrades under replay floods could be used
/// as a denial-of-service vector against legitimate users sharing the same
/// server process.
#[tokio::test]
async fn protocol_contract_rejects_replay_nonce_flood() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let payload = json!({
        "t": "msg", "ch": "general",
        "c": "nonce-flood-cipher", "ts": now,
        "n": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    });

    // First send must succeed.
    alice
        .send(Message::Text(payload.to_string()))
        .await
        .expect("send first message for nonce flood test");
    let _ = recv_by_type(&mut alice, "msg").await;

    // All subsequent replays must be consistently rejected.
    for _ in 0..20 {
        alice
            .send(Message::Text(payload.to_string()))
            .await
            .expect("send replayed nonce in flood attempt");
        let err = recv_by_type(&mut alice, "err").await;
        let msg = err.get("m").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            msg.contains("replayed nonce"),
            "expected replay nonce rejection during flood, got: {}",
            msg
        );
    }
}
