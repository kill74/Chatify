//! Chatify shared crate.
//!
//! This library contains reusable modules that are consumed by the
//! server/client binaries and integration tests.
//! Current shared surface:
//! - `crypto`: key derivation and encryption helpers used by runtime components.
//! - `error`: shared error/result types used by binaries.
//! - `totp`: two-factor authentication implementation.

pub mod crypto;
pub mod error;
pub mod totp;
