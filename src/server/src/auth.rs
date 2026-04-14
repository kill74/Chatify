//! Authentication handling for Chatify server.
//!
//! This module provides authentication validation, credential verification,
//! and 2FA support.

use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;

use crate::protocol::{
    is_valid_username, msg_type, MAX_NONCE_LEN, MAX_PASSWORD_FIELD_LEN, MAX_PUBLIC_KEY_FIELD_LEN,
    MAX_STATUS_EMOJI_LEN, MAX_STATUS_TEXT_LEN,
};

/// Validated, strongly-typed representation of a successful auth frame parse.
#[derive(Debug, Clone)]
pub struct AuthInfo {
    /// Validated username (ASCII alphanumeric / `-` / `_`, ≤ 32 chars).
    pub username: String,
    /// Password hash submitted by the client.
    pub pw_hash: String,
    /// Validated status object (text + emoji), or default.
    pub status: Value,
    /// Base64-encoded 32-byte Ed25519 public key used for E2E DM encryption.
    pub pubkey: String,
    /// Optional TOTP or backup code.
    pub otp_code: Option<String>,
    /// If true, the connecting client identifies as a bridge (e.g. Discord bot).
    pub is_bridge: bool,
    /// Bridge type identifier.
    pub bridge_type: String,
    /// Instance ID for loop prevention.
    pub bridge_instance_id: String,
    /// Number of bridge routes.
    pub bridge_routes: usize,
}

/// Error type for authentication failures.
#[derive(Debug, Clone)]
pub enum AuthError {
    /// Not a JSON object.
    NotObject,
    /// Missing or invalid type field.
    InvalidType,
    /// Missing username.
    MissingUsername,
    /// Invalid username format.
    InvalidUsername,
    /// Missing password hash.
    MissingPassword,
    /// Invalid password hash length.
    InvalidPassword,
    /// Missing public key.
    MissingPubkey,
    /// Invalid public key format.
    InvalidPubkey,
    /// Invalid status field.
    InvalidStatus(String),
    /// Invalid OTP code.
    InvalidOtp,
    /// Custom error message.
    Custom(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::NotObject => write!(f, "invalid auth frame"),
            AuthError::InvalidType => write!(f, "first frame must be auth"),
            AuthError::MissingUsername => write!(f, "missing username"),
            AuthError::InvalidUsername => write!(f, "invalid username"),
            AuthError::MissingPassword => write!(f, "missing password hash"),
            AuthError::InvalidPassword => write!(f, "invalid password hash"),
            AuthError::MissingPubkey => write!(f, "missing public key"),
            AuthError::InvalidPubkey => write!(f, "invalid public key"),
            AuthError::InvalidStatus(msg) => write!(f, "invalid status: {}", msg),
            AuthError::InvalidOtp => write!(f, "invalid otp code"),
            AuthError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

/// Validates that a public key is a valid base64-encoded 32-byte key.
pub fn is_valid_pubkey_b64(pk: &str) -> bool {
    if pk.is_empty() || pk.len() > MAX_PUBLIC_KEY_FIELD_LEN {
        return false;
    }
    match general_purpose::STANDARD.decode(pk) {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

/// Validates an auth frame and returns a typed AuthInfo on success.
pub fn validate_auth_payload(d: &Value) -> Result<AuthInfo, AuthError> {
    // 1. Must be a JSON object
    if !d.is_object() {
        return Err(AuthError::NotObject);
    }

    // 2. Type must be "auth"
    if d.get("t").and_then(|v| v.as_str()) != Some(msg_type::AUTH) {
        return Err(AuthError::InvalidType);
    }

    // 3. Username validation
    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or(AuthError::MissingUsername)?
        .to_string();
    if !is_valid_username(&username) {
        return Err(AuthError::InvalidUsername);
    }

    // 4. Password validation
    let pw = d
        .get("pw")
        .and_then(|v| v.as_str())
        .ok_or(AuthError::MissingPassword)?;
    if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
        return Err(AuthError::InvalidPassword);
    }

    // 5. Public key validation
    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or(AuthError::MissingPubkey)?
        .to_string();
    if !is_valid_pubkey_b64(&pubkey) {
        return Err(AuthError::InvalidPubkey);
    }

    // 6. Status field validation
    let status = validate_status_field(d.get("status"))?;

    // 7. OTP code (optional)
    let otp_code = d
        .get("otp")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    if let Some(ref code) = otp_code {
        if code.len() > MAX_NONCE_LEN {
            return Err(AuthError::InvalidOtp);
        }
    }

    // 8. Bridge info (optional)
    let is_bridge = d.get("bridge").and_then(|v| v.as_bool()).unwrap_or(false);
    let bridge_type = d
        .get("bridge_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let bridge_instance_id = d
        .get("bridge_instance_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let bridge_routes = d.get("bridge_routes").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    Ok(AuthInfo {
        username,
        pw_hash: pw.to_string(),
        status,
        pubkey,
        otp_code,
        is_bridge,
        bridge_type,
        bridge_instance_id,
        bridge_routes,
    })
}

/// Validates the status field of an auth frame.
fn validate_status_field(status: Option<&Value>) -> Result<Value, AuthError> {
    let Some(val) = status else {
        return Ok(serde_json::json!({"text": "Online", "emoji": ""}));
    };

    if !val.is_object() {
        return Err(AuthError::InvalidStatus(
            "must be a JSON object".to_string(),
        ));
    }

    let text = val
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("Online")
        .to_string();
    if text.len() > MAX_STATUS_TEXT_LEN {
        return Err(AuthError::InvalidStatus("text too long".to_string()));
    }

    let emoji = val
        .get("emoji")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if emoji.len() > MAX_STATUS_EMOJI_LEN {
        return Err(AuthError::InvalidStatus("emoji too long".to_string()));
    }

    Ok(serde_json::json!({
        "text": text,
        "emoji": emoji
    }))
}

/// Checks if a status value is the default online status.
pub fn is_default_online_status(status: &Value) -> bool {
    let text = status.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let emoji = status.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    text == "Online" && emoji.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_auth_payload() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "testuser",
            "pw": "test-hash",
            "pk": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "status": {"text": "Online", "emoji": ""}
        });

        let result = validate_auth_payload(&payload);
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert_eq!(auth.username, "testuser");
        assert_eq!(auth.pw_hash, "test-hash");
    }

    #[test]
    fn test_invalid_username() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "invalid user",
            "pw": "test-hash",
            "pk": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "status": {"text": "Online", "emoji": ""}
        });

        let result = validate_auth_payload(&payload);
        assert!(matches!(result, Err(AuthError::InvalidUsername)));
    }

    #[test]
    fn test_missing_pubkey() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "testuser",
            "pw": "test-hash",
            "status": {"text": "Online", "emoji": ""}
        });

        let result = validate_auth_payload(&payload);
        assert!(matches!(result, Err(AuthError::MissingPubkey)));
    }

    #[test]
    fn test_default_status() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "testuser",
            "pw": "test-hash",
            "pk": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        });

        let result = validate_auth_payload(&payload);
        assert!(result.is_ok());
        assert!(is_default_online_status(&result.unwrap().status));
    }
}
