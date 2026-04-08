use std::sync::Arc;

use clap::Parser;
use log::info;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

use clifford::config::Config;
use clifford::error::ChatifyResult;

use clifford_client::{
    args::Args,
    handlers,
    state::{ClientState, SharedState},
};

#[tokio::main]
async fn main() -> ChatifyResult<()> {
    let args = Args::parse();
    let config = Config::load();
    let client_config = args.merge_with_config(&config);

    if client_config.log_enabled {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    let uri = client_config.uri();
    info!("Connecting to {}", uri);

    let (ws_stream, _) = connect_async(&uri).await?;
    let (mut write, mut read) = ws_stream.split();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let state: SharedState = Arc::new(Mutex::new(ClientState::new(
        tx,
        client_config.clone(),
        config,
    )));

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let recv_task = tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(map) = data.as_object() {
                            handlers::dispatch_event(&state, map).await;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });

    let _ = tokio::join!(send_task, recv_task);

    println!("Disconnected");
    Ok(())
}
