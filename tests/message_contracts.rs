use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use clicord_server::crypto::{new_keypair, pub_b64};

struct TestServer {
    _child: Child,
    url: String,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn allocate_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr").port()
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
    for _ in 0..50 {
        if let Ok((mut ws, _)) = connect_async(&url).await {
            let _ = ws.close(None).await;
            ready = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "server did not start in time at {}", url);

    TestServer { _child: child, url }
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
