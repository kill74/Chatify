//! Shared cryptographic utilities for Chatify

use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::aead::{Aead, NewAead};
use chacha20poly1305::ChaCha20Poly1305;
use hex;
use hmac::Hmac;
use pbkdf2::pbkdf2;
use rand::{rngs::OsRng, Rng, RngCore};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

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

/// Perform X25519 Diffie-Hellman and derive a 32-byte symmetric key.
pub fn dh_key(priv_key: &[u8], pubkey_b64: &str) -> Vec<u8> {
    let priv_arr: [u8; 32] = match priv_key.try_into() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let peer_pub_raw = match general_purpose::STANDARD.decode(pubkey_b64) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let peer_pub_arr: [u8; 32] = match peer_pub_raw.as_slice().try_into() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let secret = StaticSecret::from(priv_arr);
    let peer_public = PublicKey::from(peer_pub_arr);
    let shared = secret.diffie_hellman(&peer_public);

    // Domain-separated hash to convert shared secret into a ChaCha20 key.
    let mut hasher = Sha256::new();
    hasher.update(b"chatify:dm:v1");
    hasher.update(shared.as_bytes());
    hasher.finalize().to_vec()
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
    let key_arr: &[u8; 32] = key
        .try_into()
        .map_err(|_| "Key must be 32 bytes".to_string())?;
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

/// Generate a new X25519 private key (32 bytes).
pub fn new_keypair() -> Vec<u8> {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key.to_vec()
}

/// Encode an X25519 public key as base64.
pub fn pub_b64(priv_key: &[u8]) -> String {
    let priv_arr: [u8; 32] = match priv_key.try_into() {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let secret = StaticSecret::from(priv_arr);
    let public = PublicKey::from(&secret);
    general_purpose::STANDARD.encode(public.as_bytes())
}
