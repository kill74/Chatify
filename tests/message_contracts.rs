use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use clicord_server::crypto::{new_keypair, pub_b64};

const SERVER_START_RETRIES: usize = 50;
const SERVER_START_RETRY_DELAY_MS: u64 = 100;
const RECV_RETRIES: usize = 50;
const RECV_TIMEOUT_SECS: u64 = 3;

struct TestServer {
    child: Child,
    url: String,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct TestClient {
    ws: Ws,
}

impl TestClient {
    async fn connect_and_auth(url: &str, username: &str) -> Self {
        let (client, _) = Self::connect_and_auth_with_ok(url, username).await;
        client
    }

    async fn connect_and_auth_with_ok(url: &str, username: &str) -> (Self, Value) {
        let (mut ws, _) = connect_async(url).await.expect("connect websocket");
        ws.send(Message::Text(build_auth(username).to_string()))
            .await
            .expect("send auth");

        let ok = recv_by_type(&mut ws, "ok").await;
        assert_eq!(ok.get("u").and_then(|v| v.as_str()), Some(username));

        (Self { ws }, ok)
    }

    async fn send_json(&mut self, payload: Value) {
        self.ws
            .send(Message::Text(payload.to_string()))
            .await
            .expect("send websocket message");
    }

    async fn recv_by_type(&mut self, expected_type: &str) -> Value {
        recv_by_type(&mut self.ws, expected_type).await
    }
}

fn allocate_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr").port()
}

fn build_auth(username: &str) -> Value {
    json!({
        "t": "auth",
        "u": username,
        "pw": "test-password-hash",
        "pk": pub_b64(&new_keypair()),
        "status": {"text":"Online","emoji":"🟢"}
    })
}

fn assert_users_payload(users: &[Value]) {
    assert!(
        !users.is_empty(),
        "users array should include at least self"
    );
    for user in users {
        let name = user.get("u").and_then(|v| v.as_str()).unwrap_or_default();
        let pk = user.get("pk").and_then(|v| v.as_str()).unwrap_or_default();
        assert!(!name.is_empty(), "username must be non-empty");
        assert!(!pk.is_empty(), "public key must be non-empty");
    }
}

async fn start_server() -> TestServer {
    let port = allocate_port();
    let url = format!("ws://127.0.0.1:{}", port);
    let server_bin = std::env::var("CARGO_BIN_EXE_clicord-server")
        .expect("CARGO_BIN_EXE_clicord-server must be set by cargo test");

    let child = Command::new(server_bin)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    // Wait until the websocket endpoint is accepting connections.
    let mut ready = false;
    for _ in 0..SERVER_START_RETRIES {
        if let Ok((mut ws, _)) = connect_async(&url).await {
            let _ = ws.close(None).await;
            ready = true;
            break;
        }
        sleep(Duration::from_millis(SERVER_START_RETRY_DELAY_MS)).await;
    }
    assert!(ready, "server did not start in time at {}", url);

    TestServer { child, url }
}

async fn recv_by_type(ws: &mut Ws, expected_type: &str) -> Value {
    for _ in 0..RECV_RETRIES {
        let msg = timeout(Duration::from_secs(RECV_TIMEOUT_SECS), ws.next())
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
    let mut alice = TestClient::connect_and_auth(&server.url, "alice").await;

    alice.send_json(json!({"t":"info"})).await;
    let info = alice.recv_by_type("info").await;
    assert!(info.get("online").and_then(|v| v.as_u64()).is_some());

    let (mut checker, ok) =
        TestClient::connect_and_auth_with_ok(&server.url, "auth-contract-check").await;

    assert_eq!(ok.get("t").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        ok.get("u").and_then(|v| v.as_str()),
        Some("auth-contract-check")
    );
    assert!(ok.get("channels").and_then(|v| v.as_array()).is_some());
    assert!(ok.get("hist").and_then(|v| v.as_array()).is_some());
    assert!(ok.get("users").and_then(|v| v.as_array()).is_some());

    checker.send_json(json!({"t":"users","ts":1})).await;
    let users_msg = checker.recv_by_type("users").await;

    let users = users_msg
        .get("users")
        .and_then(|v| v.as_array())
        .expect("users must be an array");
    assert_users_payload(users);
}

#[tokio::test]
async fn users_contract_returns_user_objects_with_public_keys() {
    let server = start_server().await;
    let mut alice = TestClient::connect_and_auth(&server.url, "alice").await;
    let _bob = TestClient::connect_and_auth(&server.url, "bob").await;

    alice.send_json(json!({"t":"users","ts":1})).await;

    let users_msg = alice.recv_by_type("users").await;
    let users = users_msg
        .get("users")
        .and_then(|v| v.as_array())
        .expect("users response payload should be an array");

    assert!(users.len() >= 2, "expected at least alice and bob");
    assert_users_payload(users);
}

#[tokio::test]
async fn msg_contract_roundtrips_channel_payload() {
    let server = start_server().await;
    let mut alice = TestClient::connect_and_auth(&server.url, "alice").await;

    alice
        .send_json(json!({
            "t": "msg",
            "ch": "general",
            "c": "ciphertext-blob",
            "ts": 123
        }))
        .await;

    let msg = alice.recv_by_type("msg").await;
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
    let mut alice = TestClient::connect_and_auth(&server.url, "alice").await;
    let mut bob = TestClient::connect_and_auth(&server.url, "bob").await;

    alice.send_json(json!({"t":"vjoin","r":"room-a"})).await;
    bob.send_json(json!({"t":"vjoin","r":"room-a"})).await;

    sleep(Duration::from_millis(150)).await;

    alice
        .send_json(json!({"t":"vdata","r":"room-a","a":"ZmFrZS1hdWRpby1wYXlsb2Fk"}))
        .await;

    let vdata = bob.recv_by_type("vdata").await;
    assert_eq!(vdata.get("from").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(
        vdata.get("a").and_then(|v| v.as_str()),
        Some("ZmFrZS1hdWRpby1wYXlsb2Fk")
    );
}
