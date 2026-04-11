//! HTTP health check and metrics server.

use std::sync::Arc;
use std::time::Instant;

use clifford::metrics::PrometheusMetrics;
use log::{info, warn};
use prometheus::Encoder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::state::State;

pub async fn start_health_server(
    listener: TcpListener,
    state: Arc<State>,
    metrics: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
    metrics_enabled: bool,
    shutdown_endpoint_enabled: bool,
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
) {
    loop {
        tokio::select! {
            biased;

            _ = state.shutdown_notify.notified() => {
                info!("health server shutting down");
                break;
            }

            accept_result = listener.accept() => {
                match accept_result {
                    Ok((mut stream, addr)) => {
                        let state = state.clone();
                        let metrics = metrics.clone();
                        let shutdown_tx = shutdown_tx.clone();

                        tokio::spawn(async move {
                            let start = Instant::now();
                            let mut buffer = vec![0u8; 8192];

                            match stream.read(&mut buffer).await {
                                Ok(n) => {
                                    let request = String::from_utf8_lossy(&buffer[..n]);

                                    let (endpoint, method) = parse_http_request(&request);

                                    let response = match endpoint {
                                        "/health" | "/health/" => {
                                            create_health_response(&state)
                                        }
                                        "/metrics" | "/metrics/" if metrics_enabled => {
                                            create_metrics_response(&metrics)
                                        }
                                        "/ready" | "/ready/" => {
                                            create_ready_response(&state)
                                        }
                                        "/shutdown" | "/shutdown/" if shutdown_endpoint_enabled && method == "POST" => {
                                            if state.initiate_shutdown() {
                                                let _ = shutdown_tx.send(()).await;
                                                create_shutdown_response("initiated")
                                            } else {
                                                create_shutdown_response("already_in_progress")
                                            }
                                        }
                                        "/shutdown" | "/shutdown/" if shutdown_endpoint_enabled => {
                                            create_method_not_allowed_response()
                                        }
                                        "/live" | "/live/" => {
                                            create_live_response()
                                        }
                                        _ => {
                                            create_not_found_response()
                                        }
                                    };

                                    let duration = start.elapsed();

                                    if let Some(ref m) = metrics {
                                        if let Ok(mutex_guard) = m.lock() {
                                            mutex_guard.record_http_request(endpoint, method, 200);
                                            mutex_guard.record_http_duration(endpoint, duration);
                                        }
                                    }

                                    let _ = stream.write_all(response.as_bytes()).await;
                                }
                                Err(e) => {
                                    warn!("Failed to read from health connection {}: {}", addr, e);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Health server accept error: {}", e);
                    }
                }
            }
        }
    }
}

pub fn parse_http_request(request: &str) -> (&str, &str) {
    let lines: Vec<&str> = request.lines().collect();
    if let Some(first_line) = lines.first() {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            return (parts[1], parts[0]);
        }
    }
    ("/", "GET")
}

pub fn create_health_response(state: &Arc<State>) -> String {
    let channels = state.channels.len();
    let online = state.online_count();
    let connections = state.active_connection_count();

    let response = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": 0,
        "channels": channels,
        "online_users": online,
        "active_connections": connections
    });

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.to_string().len(),
        response
    )
}

pub fn create_ready_response(state: &Arc<State>) -> String {
    let db_ready = state.store.health_check();

    let response = serde_json::json!({
        "ready": db_ready,
        "checks": {
            "database": if db_ready { "ok" } else { "error" }
        }
    });

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.to_string().len(),
        response
    )
}

pub fn create_metrics_response(
    metrics: &Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
) -> String {
    let metrics_text = if let Some(ref m) = metrics {
        if let Ok(mutex) = m.lock() {
            let encoder = prometheus::TextEncoder::new();
            let metric_families = mutex.registry.gather();
            let mut buffer = Vec::new();
            if encoder.encode(&metric_families, &mut buffer).is_ok() {
                String::from_utf8_lossy(&buffer).to_string()
            } else {
                "Error encoding metrics".to_string()
            }
        } else {
            "Error acquiring metrics lock".to_string()
        }
    } else {
        "Metrics not enabled".to_string()
    };

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
        metrics_text.len(),
        metrics_text
    )
}

pub fn create_not_found_response() -> String {
    let body = "Not Found";
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn create_shutdown_response(status: &str) -> String {
    let response = serde_json::json!({
        "status": status,
        "message": if status == "initiated" {
            "Shutdown initiated"
        } else {
            "Shutdown already in progress"
        }
    });
    let body = response.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn create_method_not_allowed_response() -> String {
    let body = "Method Not Allowed";
    format!(
        "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nAllow: POST\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn create_live_response() -> String {
    let body = "OK";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn safe_ch(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

pub fn create_ok_response(
    username: &str,
    state: &crate::state::State,
    hist: Vec<serde_json::Value>,
    session_token: Option<&str>,
) -> String {
    let users: Vec<_> = state
        .user_pubkeys
        .iter()
        .map(|e| serde_json::json!({"u": e.key(), "pk": e.value()}))
        .collect();

    let mut response = serde_json::json!({
        "t": "ok",
        "u": username,
        "channels": state.channels_json(),
        "users": users,
        "history": hist
    });

    if let Some(token) = session_token {
        response["token"] = token.into();
    }

    response.to_string()
}
