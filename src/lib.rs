//! Chatify shared crate.
//!
//! This library contains reusable modules that are consumed by the
//! server/client binaries and integration tests.
//! Current shared surface:
//! - `crypto`: key derivation and encryption helpers used by runtime components.

pub mod crypto;
