//! Shared cryptographic utilities for Chatify
//!
//! All fallible operations return [`Result`] types instead of silently
//! degrading (e.g. returning empty vectors). This ensures callers handle
//! cryptographic failures explicitly.

use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};
/// Maximum allowed plaintext length for encryption (10 MB).
const MAX_PLAINTEXT_LEN: usize = 10 * 1024 * 1024;

/// Maximum allowed ciphertext length for decryption (10 MB + overhead).
const MAX_CIPHERTEXT_LEN: usize = 10 * 1024 * 1024 + 28; // 28 bytes overhead for nonce + tag

/// Derive a channel-specific encryption key using PBKDF2.
///
/// Uses the channel name as part of the salt for domain separation.
pub fn channel_key(password: &str, channel: &str) -> Vec<u8> {
    let mut key = <[u8; 32]>::default();
    pbkdf2::<Hmac<Sha256>>(
        password.as_bytes(),
        format!("chatify:{}", channel).as_bytes(),
        120_000,
        &mut key,
    )
    .expect("PBKDF2 with 32-byte output should always succeed");
    key.to_vec()
}

fn client_password_salt() -> Vec<u8> {
    let mut salt = env!("CARGO_PKG_NAME").as_bytes().to_vec();
    salt.push(b':');
    salt.extend_from_slice("client".as_bytes());
    salt.push(b':');
    salt.extend_from_slice("v1".as_bytes());
    salt
}

/// Perform X25519 Diffie-Hellman and derive a 32-byte symmetric key.
///
/// Returns an error string on any failure (bad key length, invalid base64,
/// or decode failure) instead of returning an empty vector.
pub fn dh_key(priv_key: &[u8], pubkey_b64: &str) -> Result<Vec<u8>, String> {
    if priv_key.len() != 32 {
        return Err("private key must be exactly 32 bytes".to_string());
    }

    let priv_arr: [u8; 32] = priv_key
        .try_into()
        .map_err(|_| "private key must be exactly 32 bytes".to_string())?;

    if pubkey_b64.is_empty() || pubkey_b64.len() > 256 {
        return Err("invalid public key length".to_string());
    }

    let peer_pub_raw = general_purpose::STANDARD
        .decode(pubkey_b64)
        .map_err(|e| format!("invalid base64 public key: {}", e))?;

    if peer_pub_raw.len() != 32 {
        return Err("decoded public key must be exactly 32 bytes".to_string());
    }

    let peer_pub_arr: [u8; 32] = peer_pub_raw
        .as_slice()
        .try_into()
        .map_err(|_| "decoded public key must be exactly 32 bytes".to_string())?;

    let secret = StaticSecret::from(priv_arr);
    let peer_public = PublicKey::from(peer_pub_arr);
    let shared = secret.diffie_hellman(&peer_public);

    // Domain-separated hash to convert shared secret into a ChaCha20 key.
    let mut hasher = Sha256::new();
    hasher.update(b"chatify:dm:v1");
    hasher.update(shared.as_bytes());
    Ok(hasher.finalize().to_vec())
}

/// Encrypt data using ChaCha20Poly1305 with a random nonce.
///
/// The nonce (12 bytes) is prepended to the ciphertext.
/// Returns `Err` on encryption failure — callers must propagate this.
pub fn enc_bytes(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    // Validate inputs
    if key.len() != 32 {
        return Err("encryption key must be exactly 32 bytes".to_string());
    }
    if plaintext.is_empty() {
        return Err("plaintext cannot be empty".to_string());
    }
    if plaintext.len() > MAX_PLAINTEXT_LEN {
        return Err(format!(
            "plaintext exceeds maximum size of {} bytes",
            MAX_PLAINTEXT_LEN
        ));
    }

    use chacha20poly1305::Nonce;
    let nonce_bytes = chacha20_nonce();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| "encryption key must be exactly 32 bytes".to_string())?;

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("encryption failure: {}", e))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(combined)
}

/// Decrypt data using ChaCha20Poly1305.
pub fn dec_bytes(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    // Validate inputs
    if key.len() != 32 {
        return Err("decryption key must be exactly 32 bytes".to_string());
    }
    if ciphertext.len() < 12 {
        return Err("ciphertext too short".to_string());
    }
    if ciphertext.len() > MAX_CIPHERTEXT_LEN {
        return Err(format!(
            "ciphertext exceeds maximum size of {} bytes",
            MAX_CIPHERTEXT_LEN
        ));
    }

    use chacha20poly1305::Nonce;
    let (nonce_bytes, ciphertext_data) = ciphertext.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| "decryption key must be exactly 32 bytes".to_string())?;

    cipher
        .decrypt(nonce, ciphertext_data)
        .map_err(|e| format!("decryption failure: {}", e))
}

/// Generate a random 12-byte nonce for ChaCha20.
pub fn chacha20_nonce() -> [u8; 12] {
    let mut nonce = <[u8; 12]>::default();
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Hash a password using PBKDF2 with SHA256 and a deterministic protocol salt.
///
/// This is used CLIENT-SIDE to produce a deterministic credential that
/// is sent to the server. The server then applies its own per-user salt
/// via [`pw_hash`] before storing. Using a deterministic protocol salt here is acceptable
/// because the server-side hashing provides the real per-user protection.
///
/// Returns a lowercase hex string.
pub fn pw_hash_client(password: &str) -> Result<String, String> {
    if password.is_empty() {
        return Err("password cannot be empty".to_string());
    }
    if password.len() > 1024 {
        return Err("password exceeds maximum length".to_string());
    }

    let mut hash = <[u8; 32]>::default();
    let salt = client_password_salt();
    pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, 120_000, &mut hash)
        .expect("PBKDF2 with 32-byte output should always succeed");
    Ok(hex::encode(hash))
}

/// Build the proof sent by auth-v2 clients.
///
/// The client secret is the deterministic client password hash. It is never
/// sent for normal v2 logins; instead the client proves possession with an
/// HMAC over both nonces and the username.
pub fn auth_proof(
    client_secret: &str,
    username: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> Result<String, String> {
    if client_secret.is_empty() {
        return Err("auth secret cannot be empty".to_string());
    }
    if username.is_empty() || client_nonce.is_empty() || server_nonce.is_empty() {
        return Err("auth proof inputs cannot be empty".to_string());
    }

    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(client_secret.as_bytes())
        .map_err(|_| "invalid auth secret".to_string())?;
    mac.update(b"chatify:auth:v2:");
    mac.update(username.as_bytes());
    mac.update(b":");
    mac.update(client_nonce.as_bytes());
    mac.update(b":");
    mac.update(server_nonce.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Hash a password using PBKDF2 with SHA256 and a random per-user salt.
///
/// This is used SERVER-SIDE to store credentials. Returns `"salt_hex$hash_hex"`
/// so both salt and hash can be recovered during verification.
pub fn pw_hash(password: &str) -> String {
    let mut salt = <[u8; 16]>::default();
    OsRng.fill_bytes(&mut salt);
    pw_hash_with_salt(password, &salt)
}

/// Hash a password with a specific salt. Returns `"salt_hex$hash_hex"`.
pub fn pw_hash_with_salt(password: &str, salt: &[u8]) -> String {
    let mut hash = <[u8; 32]>::default();
    pbkdf2::<Hmac<Sha256>>(password.as_bytes(), salt, 120_000, &mut hash)
        .expect("PBKDF2 with 32-byte output should always succeed");
    format!("{}${}", hex::encode(salt), hex::encode(hash))
}

/// Verify a password against a `"salt_hex$hash_hex"` hash string.
///
/// Returns `false` if the stored hash is malformed or the comparison fails.
/// Uses constant-time comparison via `subtle` to prevent timing attacks.
pub fn pw_verify(password: &str, stored: &str) -> bool {
    let Some((salt_hex, expected_hash_hex)) = stored.split_once('$') else {
        return false;
    };
    let Ok(salt) = hex::decode(salt_hex) else {
        return false;
    };
    let mut hash = <[u8; 32]>::default();
    if pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, 120_000, &mut hash).is_err() {
        return false;
    };
    let actual_hash_hex = hex::encode(hash);

    // Constant-time comparison to prevent timing attacks.
    if actual_hash_hex.len() != expected_hash_hex.len() {
        return false;
    }
    actual_hash_hex
        .as_bytes()
        .ct_eq(expected_hash_hex.as_bytes())
        .into()
}

/// Generate a new X25519 private key (32 bytes).
pub fn new_keypair() -> Vec<u8> {
    let mut key = <[u8; 32]>::default();
    OsRng.fill_bytes(&mut key);
    key.to_vec()
}

/// Encode an X25519 public key as base64.
///
/// Returns an error string if the private key is not exactly 32 bytes.
pub fn pub_b64(priv_key: &[u8]) -> Result<String, String> {
    if priv_key.len() != 32 {
        return Err("private key must be exactly 32 bytes".to_string());
    }
    let priv_arr: [u8; 32] = priv_key
        .try_into()
        .map_err(|_| "private key must be exactly 32 bytes".to_string())?;
    let secret = StaticSecret::from(priv_arr);
    let public = PublicKey::from(&secret);
    Ok(general_purpose::STANDARD.encode(public.as_bytes()))
}

/// Constant-time comparison of two byte slices.
///
/// Returns `true` if the slices are equal, `false` otherwise.
/// This function executes in constant time regardless of where the
/// first difference occurs, preventing timing side-channel attacks.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Securely compare two strings in constant time.
///
/// Returns `true` if the strings are equal, `false` otherwise.
pub fn secure_string_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
        assert!(!constant_time_eq(b"", b"hello"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_secure_string_eq() {
        assert!(secure_string_eq("hello", "hello"));
        assert!(!secure_string_eq("hello", "world"));
        assert!(secure_string_eq("", ""));
    }

    #[test]
    fn test_pw_hash_client_validation() {
        let empty_password = String::new();
        assert!(pw_hash_client(empty_password.as_str()).is_err());
        let password = crate::fresh_nonce_hex();
        assert!(pw_hash_client(&password).is_ok());
    }

    #[test]
    fn test_enc_dec_roundtrip() {
        let key = new_keypair();
        let plaintext = b"test message";
        let encrypted = enc_bytes(&key, plaintext).unwrap();
        let decrypted = dec_bytes(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_enc_rejects_empty_plaintext() {
        let key = new_keypair();
        assert!(enc_bytes(&key, b"").is_err());
    }

    #[test]
    fn test_enc_rejects_invalid_key() {
        let short_key = vec![u8::default(); 31];
        let long_key = vec![u8::default(); 33];
        assert!(enc_bytes(&short_key, b"test").is_err());
        assert!(enc_bytes(&long_key, b"test").is_err());
    }

    #[test]
    fn test_dh_key_validation() {
        let priv_key = new_keypair();
        let pub_key = pub_b64(&priv_key).unwrap();
        assert!(dh_key(&priv_key, &pub_key).is_ok());
        assert!(dh_key(&priv_key, "invalid-base64").is_err());
        let short_private_key = vec![u8::default(); 31];
        assert!(dh_key(&short_private_key, &pub_key).is_err());
    }
}
