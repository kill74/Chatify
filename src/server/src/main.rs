use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use log::{error, info};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

use clifford::error::ChatifyResult;
use clifford::metrics::PrometheusMetrics;

use clifford_server::{
    args::Args,
    handlers, http, tls,
    state::State,
};

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

    let tls_acceptor: Option<TlsAcceptor> = if args.tls {
        let acceptor = tls::load_tls_config(&args.tls_cert, &args.tls_key)?;
        Some(acceptor)
    } else {
        None
    };

    let metrics: Option<Arc<std::sync::Mutex<PrometheusMetrics>>> = match PrometheusMetrics::new() {
        Ok(m) => Some(Arc::new(std::sync::Mutex::new(m))),
        Err(e) => {
            log::warn!("Failed to initialize metrics: {}; continuing without metrics", e);
            None
        }
    };

    let listener = TcpListener::bind(&addr).await?;
    let state = State::new(
        args.db.clone(),
        db_key,
        metrics.clone(),
        args.max_msgs_per_minute,
        args.enable_user_rate_limit,
    );

    let enc_label = if state.store.is_encrypted() {
        "ChaCha20-Poly1305"
    } else {
        "None (unencrypted)"
    };
    let proto = if tls_acceptor.is_some() { "wss" } else { "ws" };
    println!(" Chatify running on {}://{}", proto, addr);
    println!(" Encryption: {} |   IP Privacy: On", enc_label);
    println!(" Event store: {}", args.db);
    println!(
        " User rate limit: {} msgs/min",
        if args.enable_user_rate_limit {
            args.max_msgs_per_minute.to_string()
        } else {
            "disabled".to_string()
        }
    );

    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    if args.health_port > 0 {
        let health_metrics = metrics.clone();
        let health_state = state.clone();
        let health_enabled = args.metrics_enabled;
        let shutdown_endpoint_enabled = args.shutdown_endpoint;
        let health_shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{}", args.health_port);
            match TcpListener::bind(&addr).await {
                Ok(listener) => {
                    println!(" Health/Metrics server running on http://{}", addr);
                    http::start_health_server(
                        listener,
                        health_state,
                        health_metrics,
                        health_enabled,
                        shutdown_endpoint_enabled,
                        health_shutdown_tx,
                    )
                    .await;
                }
                Err(e) => {
                    log::warn!("Failed to bind health port {}: {}", args.health_port, e);
                }
            }
        });
    }

    println!(" Press Ctrl+C to stop\n");

    let serve = async {
        loop {
            tokio::select! {
                biased;
                _ = state.shutdown_notify.notified() => {
                    info!("Shutdown signal received, stopping accept loop");
                    break;
                }
                result = listener.accept() => {
                    match result {
                        Ok((stream, client_addr)) => {
                            let state = state.clone();
                            let tls_acceptor = tls_acceptor.clone();
                            
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, client_addr, state, tls_acceptor).await {
                                    error!("Connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Accept error: {}", e);
                        }
                    }
                }
            }
        }
    };

    tokio::select! {
        _ = serve => {}
        _ = async {
            tokio::signal::ctrl_c().await.ok();
        } => {
            println!("\nShutting down...");
            initiate_graceful_shutdown(&state).await;
        }
        _ = shutdown_rx.recv() => {
            println!("Shutdown requested via HTTP endpoint...");
            initiate_graceful_shutdown(&state).await;
        }
    }

    println!("Server stopped");
    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    state: Arc<State>,
    tls_acceptor: Option<TlsAcceptor>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(acceptor) = tls_acceptor {
        let stream = acceptor.accept(stream).await?;
        handlers::handle_connection(stream, addr, state).await;
    } else {
        handlers::handle_connection(stream, addr, state).await;
    }
    Ok(())
}

async fn initiate_graceful_shutdown(state: &State) {
    if state.initiate_shutdown() {
        info!("Initiating graceful shutdown...");
        state.drained_notify.notified().await;
    }
}

fn resolve_db_key(db_path: &str, cli_key: Option<&str>) -> ChatifyResult<Option<Vec<u8>>> {
    if db_path == ":memory:" {
        return Ok(None);
    }
    if let Some(key) = cli_key {
        if key.len() != 64 {
            return Err(clifford::error::ChatifyError::Validation(
                "db_key must be exactly 64 hex characters".to_string(),
            ));
        }
        return Ok(Some(hex::decode(key)?));
    }
    let key_path = format!("{}.key", db_path);
    if let Ok(key_hex) = std::fs::read_to_string(&key_path) {
        let key_hex = key_hex.trim();
        if key_hex.len() == 64 {
            return Ok(Some(hex::decode(key_hex)?));
        }
    }
    let mut key = vec![0u8; 32];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut key);
    let key_hex = hex::encode(&key);
    std::fs::write(&key_path, &key_hex)?;
    println!("Generated new database encryption key: {}", key_path);
    Ok(Some(key))
}
