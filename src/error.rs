//! Shared error types used across Chatify binaries.
//!
//! Error variants are deliberately granular so that callers (and clients
//! receiving `err` frames) can distinguish between validation failures,
//! authentication errors, and operational failures without parsing messages.

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
    /// Authentication failure (wrong credentials, missing 2FA, etc.).
    /// Kept separate from [`Validation`] so clients can distinguish
    /// "the request is malformed" from "the credentials are wrong".
    Auth(String),
    /// Rate-limit or connection-limit rejection.
    /// Carries an optional retry-after hint in seconds.
    RateLimit {
        msg: String,
        retry_after_secs: Option<u64>,
    },
    Audio(String),
    /// Database / persistence errors.
    Database(String),
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
            ChatifyError::Auth(msg) => write!(f, "auth error: {}", msg),
            ChatifyError::RateLimit {
                msg,
                retry_after_secs: _,
            } => {
                write!(f, "rate limit: {}", msg)
            }
            ChatifyError::Audio(msg) => write!(f, "audio error: {}", msg),
            ChatifyError::Database(msg) => write!(f, "database error: {}", msg),
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

impl From<rusqlite::Error> for ChatifyError {
    fn from(value: rusqlite::Error) -> Self {
        ChatifyError::Database(format!("{}", value))
    }
}

impl From<base64::DecodeError> for ChatifyError {
    fn from(value: base64::DecodeError) -> Self {
        ChatifyError::Validation(format!("base64 decode error: {}", value))
    }
}

impl From<hex::FromHexError> for ChatifyError {
    fn from(value: hex::FromHexError) -> Self {
        ChatifyError::Validation(format!("hex decode error: {}", value))
    }
}

impl From<std::string::FromUtf8Error> for ChatifyError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        ChatifyError::Validation(format!("UTF-8 decode error: {}", value))
    }
}

impl From<r2d2::Error> for ChatifyError {
    fn from(value: r2d2::Error) -> Self {
        ChatifyError::Database(format!("connection pool error: {}", value))
    }
}
