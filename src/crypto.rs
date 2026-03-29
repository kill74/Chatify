//! Shared cryptographic utilities for Chatify
//!
//! All fallible operations return [`Result`] types instead of silently
//! degrading (e.g. returning empty vectors). This ensures callers handle
//! cryptographic failures explicitly.

use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::aead::{Aead, NewAead};
use chacha20poly1305::ChaCha20Poly1305;
use hmac::Hmac;
use pbkdf2::pbkdf2;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

fn cipher_from_key(key: &[u8]) -> Option<ChaCha20Poly1305> {
    use generic_array::GenericArray;
    let key_arr: [u8; 32] = key.try_into().ok()?;
    let key_ga = GenericArray::from(key_arr);
    Some(ChaCha20Poly1305::new(&key_ga))
}

/// Derive a channel-specific encryption key using PBKDF2.
///
/// Uses the channel name as part of the salt for domain separation.
pub fn channel_key(password: &str, channel: &str) -> Vec<u8> {
    let mut key = [0u8; 32];
    pbkdf2::<Hmac<Sha256>>(
        password.as_bytes(),
        format!("chatify:{}", channel).as_bytes(),
        120_000,
        &mut key,
    )
    .expect("PBKDF2 output size must be valid");
    key.to_vec()
}

/// Perform X25519 Diffie-Hellman and derive a 32-byte symmetric key.
///
/// Returns an error string on any failure (bad key length, invalid base64,
/// or decode failure) instead of returning an empty vector.
pub fn dh_key(priv_key: &[u8], pubkey_b64: &str) -> Result<Vec<u8>, String> {
    let priv_arr: [u8; 32] = priv_key
        .try_into()
        .map_err(|_| "private key must be exactly 32 bytes".to_string())?;

    let peer_pub_raw = general_purpose::STANDARD
        .decode(pubkey_b64)
        .map_err(|e| format!("invalid base64 public key: {}", e))?;

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
    use chacha20poly1305::Nonce;
    let nonce_bytes = chacha20_nonce();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let cipher = cipher_from_key(key).ok_or_else(|| "key must be 32 bytes".to_string())?;
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("encryption failure: {}", e))?;
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(combined)
}

/// Decrypt data using ChaCha20Poly1305.
pub fn dec_bytes(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::Nonce;
    if ciphertext.len() < 12 {
        return Err("ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext_data) = ciphertext.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = cipher_from_key(key).ok_or_else(|| "key must be 32 bytes".to_string())?;
    cipher
        .decrypt(nonce, ciphertext_data)
        .map_err(|e| format!("decryption failure: {}", e))
}

/// Generate a random 12-byte nonce for ChaCha20.
pub fn chacha20_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Backward-compatible alias for previous nonce helper name.
pub fn cha_cha20_nonce() -> [u8; 12] {
    chacha20_nonce()
}

/// Hash a password using PBKDF2 with SHA256 and a static salt.
///
/// This is used CLIENT-SIDE to produce a deterministic credential that
/// is sent to the server. The server then applies its own per-user salt
/// via [`pw_hash`] before storing. Using a static salt here is acceptable
/// because the server-side hashing provides the real per-user protection.
///
/// Returns a lowercase hex string.
pub fn pw_hash_client(password: &str) -> String {
    let mut hash = [0u8; 32];
    pbkdf2::<Hmac<Sha256>>(
        password.as_bytes(),
        b"chatify:client:v1",
        120_000,
        &mut hash,
    )
    .expect("PBKDF2 output size must be valid");
    hex::encode(hash)
}

/// Hash a password using PBKDF2 with SHA256 and a random per-user salt.
///
/// This is used SERVER-SIDE to store credentials. Returns `"salt_hex$hash_hex"`
/// so both salt and hash can be recovered during verification.
pub fn pw_hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    pw_hash_with_salt(password, &salt)
}

/// Hash a password with a specific salt. Returns `"salt_hex$hash_hex"`.
pub fn pw_hash_with_salt(password: &str, salt: &[u8]) -> String {
    let mut hash = [0u8; 32];
    pbkdf2::<Hmac<Sha256>>(password.as_bytes(), salt, 120_000, &mut hash)
        .expect("PBKDF2 output size must be valid");
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
    let mut hash = [0u8; 32];
    if pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, 120_000, &mut hash).is_err() {
        return false;
    };
    let actual_hash_hex = hex::encode(hash);

    // Constant-time comparison to prevent timing attacks.
    if actual_hash_hex.len() != expected_hash_hex.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in actual_hash_hex.bytes().zip(expected_hash_hex.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Generate a new X25519 private key (32 bytes).
pub fn new_keypair() -> Vec<u8> {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key.to_vec()
}

/// Encode an X25519 public key as base64.
///
/// Returns an error string if the private key is not exactly 32 bytes.
pub fn pub_b64(priv_key: &[u8]) -> Result<String, String> {
    let priv_arr: [u8; 32] = priv_key
        .try_into()
        .map_err(|_| "private key must be exactly 32 bytes".to_string())?;
    let secret = StaticSecret::from(priv_arr);
    let public = PublicKey::from(&secret);
    Ok(general_purpose::STANDARD.encode(public.as_bytes()))
}
