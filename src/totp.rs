//! Two-Factor Authentication (TOTP) implementation for Chatify
//!
//! Provides TOTP-based two-factor authentication with QR code generation,
//! backup codes, and recovery mechanisms.

use base64::{engine::general_purpose, Engine as _};
use hmac::Hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

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
    /// Backup codes (stored as hashes)
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
    /// Codes are stored as hashes for security
    fn generate_backup_codes(&mut self) {
        let mut rng = rand::thread_rng();
        self.backup_codes = (0..10)
            .map(|_| {
                let mut bytes = [0u8; 8];
                rng.fill_bytes(&mut bytes);
                let code = hex::encode(bytes);
                // Store hash of the code, not the code itself
                hash_backup_code(&code)
            })
            .collect();
    }

    /// Verify a TOTP code
    /// Checks current time window and ±1 window per RFC 6238
    pub fn verify_totp(&self, code: &str) -> bool {
        if !self.enabled {
            return false;
        }

        if code.is_empty() || code.len() > 10 {
            return false;
        }

        if let Some(config) = &self.totp_config {
            // Check ±1 time window for clock skew tolerance
            let current_time = now();
            for offset in -1..=1i64 {
                let check_time = current_time + (offset * config.step as i64) as f64;
                if check_time < 0.0 {
                    continue;
                }
                let check_code =
                    generate_totp_code_at_time(&config.secret, config.step, check_time);
                if secure_string_eq(code, &check_code) {
                    return true;
                }
            }
            false
        } else {
            false
        }
    }

    /// Verify a backup code
    /// Uses constant-time comparison to prevent timing attacks
    pub fn verify_backup_code(&mut self, code: &str) -> bool {
        if code.is_empty() || code.len() > 32 {
            return false;
        }

        let code_hash = hash_backup_code(code);
        let mut found_index = None;

        // Constant-time search to prevent timing attacks
        for (i, stored_hash) in self.backup_codes.iter().enumerate() {
            if secure_string_eq(&code_hash, stored_hash) {
                found_index = Some(i);
                // Don't break - continue to maintain constant time
            }
        }

        if let Some(index) = found_index {
            // Remove the used backup code
            self.backup_codes.remove(index);
            self.last_verified = Some(now());
            true
        } else {
            false
        }
    }

    /// Get the number of remaining backup codes
    pub fn remaining_backup_codes(&self) -> usize {
        self.backup_codes.len()
    }
}

/// Hash a backup code for secure storage
fn hash_backup_code(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"chatify:backup:v1:");
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a random secret for TOTP
pub fn generate_secret() -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 20];
    rng.fill_bytes(&mut bytes);
    general_purpose::STANDARD.encode(bytes)
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

    if decoded_secret.is_empty() || decoded_secret.len() > 64 {
        return String::new();
    }

    let counter_bytes = counter.to_be_bytes();

    use hmac::Mac;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&decoded_secret).expect("HMAC can take key of any size");
    mac.update(&counter_bytes);
    let result = mac.finalize();
    let digest = result.into_bytes();

    // Dynamic truncation
    let offset = (digest[digest.len() - 1] & 0x0F) as usize;
    if offset + 4 > digest.len() {
        return String::new();
    }
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
    if username.is_empty() || issuer.is_empty() || secret.is_empty() {
        return String::new();
    }
    let label = format!("{}:{}", issuer, username);
    let params = format!(
        "secret={}&issuer={}&algorithm=SHA256&digits=6&period=30",
        secret, issuer
    );
    format!("otpauth://totp/{}?{}", label, params)
}

/// Get current Unix timestamp
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Constant-time string comparison
fn secure_string_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
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
        assert_eq!(user_2fa.remaining_backup_codes(), 10);

        user_2fa.disable();
        assert!(!user_2fa.enabled);
        assert!(user_2fa.totp_config.is_none());
        assert!(user_2fa.backup_codes.is_empty());
        assert_eq!(user_2fa.remaining_backup_codes(), 0);
    }

    #[test]
    fn test_backup_code_verification() {
        let mut user_2fa = User2FA::new("testuser".to_string());
        let secret = generate_secret();
        user_2fa.enable(secret);

        // Generate a backup code and verify it
        // Note: We need to get the raw code before it's hashed
        // For testing, we'll create a test with known codes
        let mut test_user = User2FA::new("test".to_string());
        test_user.enabled = true;
        // Manually set backup codes (hashes of known codes)
        let code1 = "abcdef1234567890";
        let code2 = "fedcba0987654321";
        test_user.backup_codes = vec![hash_backup_code(code1), hash_backup_code(code2)];

        // Test that we can verify a backup code
        assert!(test_user.verify_backup_code(code1));
        assert_eq!(test_user.remaining_backup_codes(), 1);

        // Test that the same code no longer works after use
        assert!(!test_user.verify_backup_code(code1));

        // Test that the second code still works
        assert!(test_user.verify_backup_code(code2));
        assert_eq!(test_user.remaining_backup_codes(), 0);
    }

    #[test]
    fn test_totp_verification_with_clock_skew() {
        let mut user_2fa = User2FA::new("testuser".to_string());
        let secret = generate_secret();
        user_2fa.enable(secret.clone());

        // Generate a valid TOTP code using current time
        let current_code = generate_totp_code_at_time(&secret, 30, now());

        // Should accept the current code
        assert!(user_2fa.verify_totp(&current_code));

        // Should reject empty or invalid codes
        assert!(!user_2fa.verify_totp(""));
        assert!(!user_2fa.verify_totp("invalid"));
        assert!(!user_2fa.verify_totp("12345")); // Too short
    }

    #[test]
    fn test_totp_rejects_disabled_user() {
        let user_2fa = User2FA::new("testuser".to_string());
        // 2FA is disabled, so any code should be rejected
        assert!(!user_2fa.verify_totp("123456"));
    }

    #[test]
    fn test_generate_qr_url() {
        let url = generate_qr_url("alice", "Chatify", "JBSWY3DPEHPK3PXP");
        assert!(url.starts_with("otpauth://totp/"));
        assert!(url.contains("secret=JBSWY3DPEHPK3PXP"));
        assert!(url.contains("issuer=Chatify"));

        // Test empty inputs
        assert!(generate_qr_url("", "Chatify", "secret").is_empty());
        assert!(generate_qr_url("alice", "", "secret").is_empty());
        assert!(generate_qr_url("alice", "Chatify", "").is_empty());
    }

    #[test]
    fn test_backup_code_rejects_invalid_input() {
        let mut user_2fa = User2FA::new("testuser".to_string());
        user_2fa.enabled = true;
        user_2fa.backup_codes = vec![hash_backup_code("valid")];

        // Empty code should be rejected
        assert!(!user_2fa.verify_backup_code(""));

        // Overly long code should be rejected
        assert!(!user_2fa.verify_backup_code(&"x".repeat(33)));
    }
}
