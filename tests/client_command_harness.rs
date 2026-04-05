use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use clicord_server::crypto::{new_keypair, pub_b64};

const SERVER_START_RETRIES: usize = 60;
const SERVER_START_RETRY_DELAY_MS: u64 = 100;

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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
        "status": {"text":"Online","emoji":"online"}
    })
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
    for _ in 0..60 {
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

async fn recv_sys_contains(ws: &mut Ws, needle: &str, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);

        let next = match timeout(remaining, ws.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(_))) => return false,
            Ok(None) => return false,
            Err(_) => return false,
        };

        if let Message::Text(text) = next {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value.get("t").and_then(|v| v.as_str()) == Some("sys") {
                    let message = value.get("m").and_then(|v| v.as_str()).unwrap_or("");
                    if message.contains(needle) {
                        return true;
                    }
                }
            }
        }
    }
}

async fn capture_stream<R>(reader: R, sink: Arc<Mutex<String>>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut buf = sink.lock().await;
        buf.push_str(&line);
        buf.push('\n');
    }
}

async fn wait_for_output(output: Arc<Mutex<String>>, needle: &str, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        {
            let buf = output.lock().await;
            if buf.contains(needle) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_any_output(
    output: Arc<Mutex<String>>,
    patterns: &[&str],
    wait: Duration,
) -> Option<String> {
    let deadline = Instant::now() + wait;
    loop {
        {
            let buf = output.lock().await;
            if let Some(found) = patterns.iter().find(|pattern| buf.contains(**pattern)) {
                return Some((*found).to_string());
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn send_line(stdin: &mut ChildStdin, line: &str) {
    stdin
        .write_all(format!("{}\n", line).as_bytes())
        .await
        .expect("write command to client stdin");
    stdin.flush().await.expect("flush client stdin");
}

fn output_contains_any(haystack: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| haystack.contains(p))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_interactive_command_harness() {
    let server = start_server().await;

    let (mut observer, _) = connect_async(&server.url).await.expect("connect observer");
    observer
        .send(Message::Text(build_auth("observer").to_string()))
        .await
        .expect("send observer auth");
    let _ = recv_by_type(&mut observer, "ok").await;

    observer
        .send(Message::Text(
            json!({"t": "join", "ch": "room-e2e"}).to_string(),
        ))
        .await
        .expect("observer join room-e2e");
    let _ = recv_by_type(&mut observer, "joined").await;

    let client_bin =
        std::env::var("CARGO_BIN_EXE_clicord-client").expect("CARGO_BIN_EXE_clicord-client set");

    let mut client = tokio::process::Command::new(client_bin)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(
            server
                .url
                .split(':')
                .next_back()
                .expect("extract server port"),
        )
        .arg("--log")
        .env("CHATIFY_USERNAME", "runner")
        .env("CHATIFY_PASSWORD", "runner-pass")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn client");

    let mut stdin = client.stdin.take().expect("client stdin available");
    let stdout: ChildStdout = client.stdout.take().expect("client stdout available");
    let stderr: ChildStderr = client.stderr.take().expect("client stderr available");

    let output = Arc::new(Mutex::new(String::new()));
    let out_task = tokio::spawn(capture_stream(stdout, output.clone()));
    let err_task = tokio::spawn(capture_stream(stderr, output.clone()));

    let connected = wait_for_output(
        output.clone(),
        "Connected to server",
        Duration::from_secs(10),
    )
    .await;
    assert!(connected, "client failed to connect within timeout");

    send_line(&mut stdin, "/join room-e2e").await;

    let saw_join = recv_sys_contains(
        &mut observer,
        "runner joined #room-e2e",
        Duration::from_secs(8),
    )
    .await;
    assert!(
        saw_join,
        "observer did not receive runner join system event"
    );

    send_line(&mut stdin, "/screen view room-e2e").await;
    let saw_screen_view = wait_for_any_output(
        output.clone(),
        &["Subscribed to screen stream in #room-e2e"],
        Duration::from_secs(8),
    )
    .await;
    assert!(
        saw_screen_view.is_some(),
        "missing /screen view acknowledgement in logs"
    );

    let screen_stop_patterns = [
        "Screen stream unsubscribed",
        "No active screen stream. Use /screen start [room]",
        "Screen share stopped",
    ];

    send_line(&mut stdin, "/screen stop").await;
    let saw_screen_stop = wait_for_any_output(
        output.clone(),
        &screen_stop_patterns,
        Duration::from_secs(8),
    )
    .await;
    assert!(
        saw_screen_stop.is_some(),
        "missing /screen stop feedback in logs"
    );

    send_line(&mut stdin, "/voice room-e2e").await;
    let voice_outcome = wait_for_any_output(
        output.clone(),
        &["Voice started in #room-e2e", "Voice start failed:"],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        voice_outcome.is_some(),
        "missing /voice command outcome in logs"
    );

    send_line(&mut stdin, "/screen start room-e2e").await;
    let screen_start_outcome = wait_for_any_output(
        output.clone(),
        &["Screen share started in #room-e2e", "Screen share failed:"],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        screen_start_outcome.is_some(),
        "missing /screen start command outcome in logs"
    );

    send_line(&mut stdin, "/screen stop").await;
    let saw_second_screen_stop = wait_for_any_output(
        output.clone(),
        &screen_stop_patterns,
        Duration::from_secs(8),
    )
    .await;
    assert!(
        saw_second_screen_stop.is_some(),
        "missing second /screen stop feedback in logs"
    );

    send_line(&mut stdin, "/quit").await;

    let exit = timeout(Duration::from_secs(20), client.wait())
        .await
        .expect("client did not exit in time")
        .expect("client wait failed");
    assert!(exit.success(), "client exited with status: {}", exit);

    drop(stdin);
    let _ = out_task.await;
    let _ = err_task.await;

    let logs = output.lock().await.clone();
    assert!(
        output_contains_any(
            &logs,
            &[
                "Subscribed to screen stream in #room-e2e",
                "Screen share started in #room-e2e"
            ]
        ),
        "harness logs did not capture expected command output:\n{}",
        logs
    );
}
