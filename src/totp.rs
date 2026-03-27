//! Two-Factor Authentication (TOTP) implementation for Chatify
//!
//! Provides TOTP-based two-factor authentication with QR code generation,
//! backup codes, and recovery mechanisms.

use base64::{engine::general_purpose, Engine as _};
use hmac::Hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

/// TOTP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpConfig {
    /// Secret key for TOTP generation
    pub secret: String,
    /// Number of digits in the TOTP code
    pub digits: u32,
    /// Time step in seconds
    pub step: u64,
    /// Algorithm used (SHA1, SHA256, SHA512)
    pub algorithm: String,
}

/// User's 2FA status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User2FA {
    /// Username
    pub username: String,
    /// Whether 2FA is enabled
    pub enabled: bool,
    /// TOTP configuration
    pub totp_config: Option<TotpConfig>,
    /// Backup codes
    pub backup_codes: Vec<String>,
    /// Timestamp when 2FA was enabled
    pub enabled_at: Option<f64>,
    /// Last successful verification timestamp
    pub last_verified: Option<f64>,
}

impl User2FA {
    /// Create a new User2FA instance
    pub fn new(username: String) -> Self {
        Self {
            username,
            enabled: false,
            totp_config: None,
            backup_codes: Vec::new(),
            enabled_at: None,
            last_verified: None,
        }
    }

    /// Enable 2FA for the user
    pub fn enable(&mut self, secret: String) {
        self.totp_config = Some(TotpConfig {
            secret,
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });
        self.enabled = true;
        self.enabled_at = Some(now());
        self.generate_backup_codes();
    }

    /// Disable 2FA for the user
    pub fn disable(&mut self) {
        self.enabled = false;
        self.totp_config = None;
        self.backup_codes.clear();
        self.enabled_at = None;
        self.last_verified = None;
    }

    /// Generate backup codes for the user
    fn generate_backup_codes(&mut self) {
        let mut rng = rand::thread_rng();
        self.backup_codes = (0..10)
            .map(|_| {
                let mut bytes = [0u8; 8];
                rng.fill_bytes(&mut bytes);
                hex::encode(&bytes)
            })
            .collect();
    }

    /// Verify a TOTP code
    pub fn verify_totp(&self, code: &str) -> bool {
        if !self.enabled {
            return false;
        }

        if let Some(config) = &self.totp_config {
            let current_code = generate_totp_code(&config.secret, config.step);
            let previous_code = generate_totp_code_at_time(&config.secret, config.step, now() - config.step as f64);

            code == current_code || code == previous_code
        } else {
            false
        }
    }

    /// Verify a backup code
    pub fn verify_backup_code(&mut self, code: &str) -> bool {
        if let Some(pos) = self.backup_codes.iter().position(|c| c == code) {
            // Remove the used backup code
            self.backup_codes.remove(pos);
            self.last_verified = Some(now());
            true
        } else {
            false
        }
    }
}

/// Generate a random secret for TOTP
pub fn generate_secret() -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 20];
    rng.fill_bytes(&mut bytes);
    general_purpose::STANDARD.encode(&bytes)
}

/// Generate a TOTP code based on the current time
fn generate_totp_code(secret: &str, step: u64) -> String {
    generate_totp_code_at_time(secret, step, now())
}

/// Generate a TOTP code at a specific time
fn generate_totp_code_at_time(secret: &str, step: u64, time: f64) -> String {
    let counter = (time as u64) / step;
    generate_hotp_code(secret, counter)
}

/// Generate an HOTP code
fn generate_hotp_code(secret: &str, counter: u64) -> String {
    let decoded_secret = match general_purpose::STANDARD.decode(secret) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let counter_bytes = counter.to_be_bytes();

    use hmac::Mac;
    let mut mac = Hmac::<Sha256>::new_from_slice(&decoded_secret)
        .expect("HMAC can take key of any size");
    mac.update(&counter_bytes);
    let result = mac.finalize();
    let digest = result.into_bytes();

    // Dynamic truncation
    let offset = (digest[19] & 0x0F) as usize;
    let binary = ((digest[offset] as u32 & 0x7F) << 24)
        | ((digest[offset + 1] as u32 & 0xFF) << 16)
        | ((digest[offset + 2] as u32 & 0xFF) << 8)
        | (digest[offset + 3] as u32 & 0xFF);

    // Generate the OTP digits
    let otp = binary % 1_000_000;
    format!("{:06}", otp)
}

/// Generate a QR code URL for TOTP setup
pub fn generate_qr_url(username: &str, issuer: &str, secret: &str) -> String {
    let label = format!("{}:{}", issuer, username);
    let params = format!("secret={}&issuer={}&algorithm=SHA256&digits=6&period=30", secret, issuer);
    format!("otpauth://totp/{}?{}", label, params)
}

/// Get current Unix timestamp
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret() {
        let secret = generate_secret();
        assert!(!secret.is_empty());
        // Decode to verify it's valid base32-ish
        assert!(general_purpose::STANDARD.decode(&secret).is_ok());
    }

    #[test]
    fn test_user_2fa_lifecycle() {
        let mut user_2fa = User2FA::new("testuser".to_string());
        assert!(!user_2fa.enabled);

        let secret = generate_secret();
        user_2fa.enable(secret.clone());
        assert!(user_2fa.enabled);
        assert!(user_2fa.totp_config.is_some());
        assert!(!user_2fa.backup_codes.is_empty());
        assert_eq!(user_2fa.backup_codes.len(), 10);

        user_2fa.disable();
        assert!(!user_2fa.enabled);
        assert!(user_2fa.totp_config.is_none());
        assert!(user_2fa.backup_codes.is_empty());
    }

    #[test]
    fn test_backup_code_verification() {
        let mut user_2fa = User2FA::new("testuser".to_string());
        let secret = generate_secret();
        user_2fa.enable(secret);

        // Test that we can verify a backup code
        let code = user_2fa.backup_codes[0].clone();
        assert!(user_2fa.verify_backup_code(&code));

        // Test that the same code no longer works after use
        assert!(!user_2fa.verify_backup_code(&code));
    }
}