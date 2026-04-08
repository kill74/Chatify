//! Connection handlers - WebSocket message handling.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::args::{HISTORY_CAP, MAX_AUTH_BYTES};
use crate::state::{BridgeInfo, State};
use crate::validation::{validate_auth_payload, AuthInfo};

pub async fn handle_connection<S>(stream: S, addr: std::net::SocketAddr, state: Arc<State>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    if !state.ip_connect(&addr) {
        warn!(
            "connection rejected: too many connections from {}",
            addr.ip()
        );
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

    let ws = match accept_async(stream).await {
        Ok(w) => w,
        Err(e) => {
            debug!("WebSocket handshake failed from {}: {}", addr, e);
            return;
        }
    };

    let (mut sink, mut stream) = ws.split();

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

    if !state.ip_auth_allowed(&addr) {
        warn!("auth rate limited from {}", addr.ip());
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
        pw_hash: _,
        status,
        pubkey,
        otp_code: _otp_code,
        is_bridge,
        bridge_type,
        bridge_instance_id,
        bridge_routes,
    } = auth;

    if state.user_statuses.contains_key(&username) {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"username already in use"}).to_string(),
            ))
            .await;
        warn!("auth rejected: username '{}' already connected", username);
        return;
    }

    let _session_token = state.create_session(&username);

    state.user_statuses.insert(username.clone(), status);
    state.user_pubkeys.insert(username.clone(), pubkey);

    if is_bridge {
        let info = BridgeInfo {
            username: username.clone(),
            bridge_type: bridge_type.clone(),
            instance_id: bridge_instance_id.clone(),
            connected_at: clifford::now(),
            route_count: bridge_routes,
        };
        state.bridges.insert(username.clone(), info);
        info!(
            "event=bridge_connected bridge_type={} instance_id={} user={} routes={}",
            bridge_type, bridge_instance_id, username, bridge_routes
        );
    }

    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let hist = general.hist(HISTORY_CAP);

    let ok = crate::http::create_ok_response(&username, &state, hist);
    if sink.send(Message::Text(ok)).await.is_err() {
        return;
    }

    info!("+ {}", username);

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    spawn_broadcast_forwarder(gen_rx, out_tx.clone());

    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() {
                break;
            }
        }
    });

    let mut voice_room: Option<String> = None;

    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                warn!("WebSocket error from {}: {}", addr, e);
                break;
            }
            None => break,
        };

        if let Message::Text(raw) = msg {
            let d: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };

            handle_event(&d, &state, &username, &out_tx, &mut voice_room).await;
        }
    }

    info!("- {}", username);
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    state.bridges.remove(&username);
    state.end_session(&username);
    state.ip_disconnect(&addr);
}

pub async fn handle_event(
    d: &Value,
    state: &Arc<State>,
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    _voice_room: &mut Option<String>,
) {
    let t = d["t"].as_str().unwrap_or("");
    let event_channel = d
        .get("ch")
        .or_else(|| d.get("r"))
        .and_then(|v| v.as_str())
        .map(crate::http::safe_ch);

    if let Some(ch) = event_channel.as_deref() {
        info!("event user={} type={} channel={}", username, t, ch);
    } else {
        info!("event user={} type={}", username, t);
    }

    match t {
        "msg" => {
            let ch = crate::http::safe_ch(d["ch"].as_str().unwrap_or("general"));
            let c = d["c"].as_str().unwrap_or("");
            if c.is_empty() {
                return;
            }

            if !state.can_send(username, &ch) {
                send_err(out_tx, "cannot send to channel".to_string());
                return;
            }

            let payload = serde_json::json!({
                "t": "msg",
                "ch": ch,
                "u": username,
                "c": c,
                "ts": clifford::now()
            });

            if let Some(ch_data) = state.channels.get(&ch) {
                let _ = ch_data.tx.send(payload.to_string());
                ch_data.add_message(c.to_string(), clifford::now());
            }
        }
        "history" => {
            let ch = crate::http::safe_ch(d["ch"].as_str().unwrap_or("general"));
            let limit = d
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(HISTORY_CAP as u64) as usize;
            let hist = if let Some(ch_data) = state.channels.get(&ch) {
                ch_data.hist(limit.min(1000))
            } else {
                Vec::new()
            };
            let resp = serde_json::json!({
                "t": "history",
                "ch": ch,
                "msgs": hist
            });
            let _ = out_tx.send(resp.to_string());
        }
        "channels" => {
            let resp = serde_json::json!({
                "t": "channels",
                "chs": state.channels_json()
            });
            let _ = out_tx.send(resp.to_string());
        }
        "users" => {
            let users: Vec<_> = state
                .user_pubkeys
                .iter()
                .map(|e| serde_json::json!({"u": e.key(), "pk": e.value()}))
                .collect();
            let resp = serde_json::json!({
                "t": "users",
                "users": users
            });
            let _ = out_tx.send(resp.to_string());
        }
        "ping" => {
            let _ = out_tx.send(serde_json::json!({"t": "pong"}).to_string());
        }
        _ => {
            debug!("unhandled event type: {}", t);
        }
    }
}

fn send_err(out_tx: &mpsc::UnboundedSender<String>, msg: String) {
    let _ = out_tx.send(serde_json::json!({"t": "err", "m": msg}).to_string());
}

struct ConnectionGuard {
    state: Arc<State>,
}

impl ConnectionGuard {
    fn new(state: Arc<State>, _addr: std::net::SocketAddr) -> Self {
        state
            .active_connections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self { state }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.state
            .active_connections
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .state
            .active_connections
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            self.state.drained_notify.notify_waiters();
        }
    }
}

fn spawn_broadcast_forwarder(
    mut rx: tokio::sync::broadcast::Receiver<String>,
    out_tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if out_tx.send(msg).is_err() {
                break;
            }
        }
    });
}
