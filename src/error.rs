//! Shared error types used across Chatify binaries.

use std::error::Error;
use std::fmt;

/// Canonical result type for Chatify runtime code.
pub type ChatifyResult<T> = Result<T, ChatifyError>;

#[derive(Debug)]
pub enum ChatifyError {
    Io(Box<std::io::Error>),
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),
    Json(Box<serde_json::Error>),
    Crypto(String),
    Validation(String),
    Audio(String),
    Message(String),
}

impl fmt::Display for ChatifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChatifyError::Io(err) => write!(f, "io error: {}", err),
            ChatifyError::WebSocket(err) => write!(f, "websocket error: {}", err),
            ChatifyError::Json(err) => write!(f, "json error: {}", err),
            ChatifyError::Crypto(msg) => write!(f, "crypto error: {}", msg),
            ChatifyError::Validation(msg) => write!(f, "validation error: {}", msg),
            ChatifyError::Audio(msg) => write!(f, "audio error: {}", msg),
            ChatifyError::Message(msg) => write!(f, "{}", msg),
        }
    }
}

impl Error for ChatifyError {}

impl From<std::io::Error> for ChatifyError {
    fn from(value: std::io::Error) -> Self {
        ChatifyError::Io(Box::new(value))
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for ChatifyError {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        ChatifyError::WebSocket(Box::new(value))
    }
}

impl From<serde_json::Error> for ChatifyError {
    fn from(value: serde_json::Error) -> Self {
        ChatifyError::Json(Box::new(value))
    }
}
