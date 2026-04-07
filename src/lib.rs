//! Chatify shared crate.
//!
//! This library contains reusable modules that are consumed by the
//! server/client binaries and integration tests.
//! Current shared surface:
//! - `crypto`: key derivation and encryption helpers used by runtime components.
//! - `error`: shared error/result types used by binaries.
//! - `totp`: two-factor authentication implementation.
//! - `config`: configuration management for persistent settings.
//! - utility functions for channel normalization, timestamps, and nonces.

pub mod config;
pub mod crypto;
pub mod error;
pub mod notifications;
pub mod totp;
pub mod ui;
pub mod screen_share;
pub mod performance;
pub mod voice;
pub mod metrics;

use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};

/// Normalizes a raw channel name to a safe, consistent format.
///
/// Rules: lowercase, strip leading `#`, keep only ASCII alphanumeric / `-` / `_`,
/// truncate to 32 chars. Returns `None` if the result is empty.
pub fn normalize_channel(raw: &str) -> Option<String> {
    let s: String = raw
        .to_lowercase()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Returns the current Unix timestamp as seconds (f64).
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Returns the current Unix timestamp as seconds (u64).
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generates a random 32-character hex nonce.
pub fn fresh_nonce_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
