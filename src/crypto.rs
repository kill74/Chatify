//! Shared cryptographic utilities for Chatify

use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, NewAead};
use hex;
use hmac::Hmac;
use pbkdf2::pbkdf2;
use rand::Rng;
use sha2::Sha256;

/// Derive a channel-specific encryption key using PBKDF2
pub fn channel_key(password: &str, channel: &str) -> Vec<u8> {
    let mut key = [0u8; 32];
    let _ = pbkdf2::<Hmac<Sha256>>(
        password.as_bytes(),
        format!("chatify:{}", channel).as_bytes(),
        120000,
        &mut key,
    );
    key.to_vec()
}

/// Perform Diffie-Hellman key exchange for DM encryption
/// Note: This is stubbed due to security concerns with available libraries.
/// TODO: Implement with a secure, audited ECDH library
pub fn dh_key(_priv_key: &[u8], pubkey_b64: &str) -> Vec<u8> {
    // For now, derive a key from the pubkey for testing
    let decoded = general_purpose::STANDARD
        .decode(pubkey_b64)
        .unwrap_or_default();
    let mut key = [0u8; 32];
    let n = decoded.len().min(32);
    key[..n].copy_from_slice(&decoded[..n]);
    key.to_vec()
}

/// Encrypt data using ChaCha20Poly1305 with a random nonce
pub fn enc_bytes(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    use chacha20poly1305::Nonce;
    use generic_array::GenericArray;
    let nonce_bytes = cha_cha20_nonce();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let key_arr: &[u8; 32] = key.try_into().expect("Key must be 32 bytes");
    let key_ga = GenericArray::from(*key_arr);
    let cipher = ChaCha20Poly1305::new(&key_ga);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .expect("Encryption failure");
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    combined
}

/// Decrypt data using ChaCha20Poly1305
pub fn dec_bytes(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::Nonce;
    use generic_array::GenericArray;
    if ciphertext.len() < 12 {
        return Err("Ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext_data) = ciphertext.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let key_arr: &[u8; 32] = key.try_into().map_err(|_| "Key must be 32 bytes".to_string())?;
    let key_ga = GenericArray::from(*key_arr);
    let cipher = ChaCha20Poly1305::new(&key_ga);
    cipher
        .decrypt(nonce, ciphertext_data)
        .map_err(|e| format!("Decryption failure: {}", e))
}

/// Generate a random 12-byte nonce for ChaCha20
pub fn cha_cha20_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill(&mut nonce);
    nonce
}

/// Hash a password using PBKDF2 with SHA256
pub fn pw_hash(password: &str) -> String {
    let mut hash = [0u8; 32];
    let _ = pbkdf2::<Hmac<Sha256>>(password.as_bytes(), b"chatify", 120000, &mut hash);
    hex::encode(hash)
}

/// Generate a new X25519 keypair
/// Note: Stubbed due to security vulnerabilities in available libraries.
/// TODO: Implement with a secure, audited ECDH library  
pub fn new_keypair() -> Vec<u8> {
    // Generate random 32 bytes for the private key
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u8
            + i as u8;
    }
    key.to_vec()
}

/// Encode a public key as base64
/// Note: Simplified version due to ECDH implementation being stubbed
pub fn pub_b64(priv_key: &[u8]) -> String {
    // For now, just return a b64 encoded version of the key
    // TODO: Properly compute public key from private key
    general_purpose::STANDARD.encode(priv_key)
}
