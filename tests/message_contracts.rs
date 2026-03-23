use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use clicord_server::crypto::{new_keypair, pub_b64};

struct TestServer {
    _child: Child,
    url: String,
    db_path: PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
        let _ = std::fs::remove_file(&self.db_path);
    }
}

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn allocate_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr").port()
}

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

fn seed_schema_version(db_path: &PathBuf, version: &str) {
    let conn = Connection::open(db_path).expect("create sqlite db");
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_meta (
            key TEXT PRIMARY KEY,
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

fn read_schema_version(db_path: &PathBuf) -> String {
    let conn = Connection::open(db_path).expect("open sqlite db");
    conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )
    .expect("schema_version row exists")
}

async fn start_server_with_db(db_path: PathBuf) -> TestServer {
    let port = allocate_port();
    let url = format!("ws://127.0.0.1:{}", port);
    let server_bin = std::env::var("CARGO_BIN_EXE_clicord-server")
        .expect("CARGO_BIN_EXE_clicord-server must be set by cargo test");

    let child = Command::new(server_bin)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--db")
        .arg(db_path.to_string_lossy().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    // Wait until the websocket endpoint is accepting connections.
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
    }
}

async fn start_server() -> TestServer {
    let port = allocate_port();
    let db_path = temp_db_path(port);
    start_server_with_db(db_path).await
}

async fn connect_and_auth(url: &str, username: &str) -> Ws {
    let (mut ws, _) = connect_async(url).await.expect("connect websocket");

    let auth = json!({
        "t": "auth",
        "u": username,
        "pw": "test-password-hash",
        "pk": pub_b64(&new_keypair()),
        "status": {"text":"Online","emoji":"🟢"}
    });

    ws.send(Message::Text(auth.to_string()))
        .await
        .expect("send auth");

    let ok = recv_by_type(&mut ws, "ok").await;
    assert_eq!(ok.get("u").and_then(|v| v.as_str()), Some(username));
    ws
}

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

#[tokio::test]
async fn auth_contract_returns_expected_fields() {
    let server = start_server().await;
    let mut ws = connect_and_auth(&server.url, "alice").await;

    ws.send(Message::Text(json!({"t":"info"}).to_string()))
        .await
        .expect("send info");
    let info = recv_by_type(&mut ws, "info").await;
    assert!(info.get("online").and_then(|v| v.as_u64()).is_some());

    let (mut ws2, _) = connect_async(&server.url)
        .await
        .expect("connect second websocket");
    let auth = json!({
        "t": "auth",
        "u": "auth-contract-check",
        "pw": "test",
        "pk": pub_b64(&new_keypair()),
        "status": {"text":"Online","emoji":"🟢"}
    });
    ws2.send(Message::Text(auth.to_string()))
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
}

#[tokio::test]
async fn auth_contract_rejects_invalid_username() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url).await.expect("connect websocket");

    let bad_auth = json!({
        "t": "auth",
        "u": "invalid user",
        "pw": "test-password-hash",
        "pk": pub_b64(&new_keypair()),
        "status": {"text":"Online","emoji":"🟢"}
    });

    ws.send(Message::Text(bad_auth.to_string()))
        .await
        .expect("send invalid auth payload");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("validation error: invalid username")
    );
}

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

#[tokio::test]
async fn msg_contract_roundtrips_channel_payload() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
                "c": "ciphertext-blob",
                "ts": 123
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

#[tokio::test]
async fn history_contract_returns_persisted_events() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
                "c": "history-cipher",
                "p": "history plain text marker"
            })
            .to_string(),
        ))
        .await
        .expect("send message for history");

    let _ = recv_by_type(&mut alice, "msg").await;

    alice
        .send(Message::Text(
            json!({
                "t": "history",
                "ch": "general",
                "limit": 25
            })
            .to_string(),
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

#[tokio::test]
async fn search_contract_filters_by_plaintext_index() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
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
            json!({
                "t": "search",
                "ch": "general",
                "q": "deploy",
                "limit": 25
            })
            .to_string(),
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

#[tokio::test]
async fn rewind_contract_returns_recent_events() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
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
            json!({
                "t": "rewind",
                "ch": "general",
                "seconds": 300,
                "limit": 25
            })
            .to_string(),
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

#[tokio::test]
async fn schema_meta_contains_current_version() {
    let server = start_server().await;
    let _alice = connect_and_auth(&server.url, "alice").await;

    let version = read_schema_version(&server.db_path);

    assert_eq!(version, "1");
}

#[tokio::test]
async fn schema_migrates_from_version_zero_to_one() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_schema_version(&db_path, "0");

    let server = start_server_with_db(db_path.clone()).await;
    let _alice = connect_and_auth(&server.url, "alice").await;

    let conn = Connection::open(&db_path).expect("open migrated db");
    let version: String = read_schema_version(&db_path);
    assert_eq!(
        version, "1",
        "expected migration to set schema version to 1"
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

    drop(server);
}

#[tokio::test]
async fn schema_newer_version_is_not_downgraded_and_server_auth_still_works() {
    let seed_port = allocate_port();
    let db_path = temp_db_path(seed_port);
    seed_schema_version(&db_path, "999");

    let server = start_server_with_db(db_path.clone()).await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

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

#[tokio::test]
async fn auth_contract_rejects_non_auth_first_frame() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for invalid auth test");

    ws.send(Message::Text(
        json!({
            "t": "ping",
            "u": "alice"
        })
        .to_string(),
    ))
    .await
    .expect("send invalid first frame");

    let err = recv_by_type(&mut ws, "err").await;
    assert_eq!(
        err.get("m").and_then(|v| v.as_str()),
        Some("first frame must be auth")
    );
}

#[tokio::test]
async fn auth_contract_rejects_invalid_public_key() {
    let server = start_server().await;
    let (mut ws, _) = connect_async(&server.url)
        .await
        .expect("connect websocket for invalid key test");

    ws.send(Message::Text(
        json!({
            "t": "auth",
            "u": "alice",
            "pw": "test-password-hash",
            "pk": "not-base64",
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

#[tokio::test]
async fn protocol_contract_rejects_stale_timestamp_on_mutating_event() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;

    alice
        .send(Message::Text(
            json!({
                "t": "msg",
                "ch": "general",
                "c": "stale-cipher",
                "ts": 1,
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

#[tokio::test]
async fn protocol_contract_rejects_replayed_nonce() {
    let server = start_server().await;
    let mut alice = connect_and_auth(&server.url, "alice").await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let payload = json!({
        "t": "msg",
        "ch": "general",
        "c": "replay-cipher",
        "ts": now,
        "n": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });

    alice
        .send(Message::Text(payload.to_string()))
        .await
        .expect("send first message with nonce");
    let _ = recv_by_type(&mut alice, "msg").await;

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
