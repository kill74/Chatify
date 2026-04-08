//! Authentication payload validation.

use base64::{engine::general_purpose, Engine as _};
use clifford::error::{ChatifyError, ChatifyResult};
use serde_json::Value;

use crate::args::{
    MAX_NONCE_LEN, MAX_PASSWORD_FIELD_LEN, MAX_PUBLIC_KEY_FIELD_LEN, MAX_STATUS_EMOJI_LEN,
    MAX_STATUS_TEXT_LEN, MAX_USERNAME_LEN,
};

pub struct AuthInfo {
    pub username: String,
    pub pw_hash: String,
    pub status: Value,
    pub pubkey: String,
    pub otp_code: Option<String>,
    pub is_bridge: bool,
    pub bridge_type: String,
    pub bridge_instance_id: String,
    pub bridge_routes: usize,
}

pub fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn is_valid_pubkey_b64(pk: &str) -> bool {
    if pk.is_empty() || pk.len() > MAX_PUBLIC_KEY_FIELD_LEN {
        return false;
    }
    match general_purpose::STANDARD.decode(pk) {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

pub fn validate_auth_payload(d: &Value) -> ChatifyResult<AuthInfo> {
    if !d.is_object() {
        return Err(ChatifyError::Validation("invalid auth frame".to_string()));
    }
    if d.get("t").and_then(|v| v.as_str()) != Some("auth") {
        return Err(ChatifyError::Message(
            "first frame must be auth".to_string(),
        ));
    }

    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing username".to_string()))?
        .to_string();
    if !is_valid_username(&username) {
        return Err(ChatifyError::Validation("invalid username".to_string()));
    }

    let pw = d
        .get("pw")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing password hash".to_string()))?;
    if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
        return Err(ChatifyError::Validation(
            "invalid password hash".to_string(),
        ));
    }

    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing public key".to_string()))?
        .to_string();
    if !is_valid_pubkey_b64(&pubkey) {
        return Err(ChatifyError::Message("invalid public key".to_string()));
    }

    let status = validate_status_field(d.get("status"))?;

    let otp_code = d
        .get("otp")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    if let Some(code) = otp_code.as_deref() {
        if code.len() > MAX_NONCE_LEN {
            return Err(ChatifyError::Validation("invalid otp code".to_string()));
        }
    }

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

pub fn validate_status_field(status: Option<&Value>) -> ChatifyResult<Value> {
    let Some(val) = status else {
        return Ok(serde_json::json!({"text": "Online", "emoji": ""}));
    };

    if !val.is_object() {
        return Err(ChatifyError::Validation(
            "status must be a JSON object".to_string(),
        ));
    }

    let text = val
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("Online")
        .to_string();
    if text.len() > MAX_STATUS_TEXT_LEN {
        return Err(ChatifyError::Validation("status text too long".to_string()));
    }

    let emoji = val
        .get("emoji")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if emoji.len() > MAX_STATUS_EMOJI_LEN {
        return Err(ChatifyError::Validation(
            "status emoji too long".to_string(),
        ));
    }

    Ok(serde_json::json!({"text": text, "emoji": emoji}))
}
