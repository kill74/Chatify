//! Chatify Server Library
//!
//! This library provides the core server components for Chatify,
//! a secure WebSocket-based chat server with end-to-end encryption support.
//!
//! ## Architecture
//!
//! The server is organized into focused modules:
//!
//! - `args`: Command-line argument parsing
//! - `auth`: Authentication and credential validation
//! - `db`: SQLite-backed event store and persistence
//! - `protocol`: Protocol constants, types, and utilities
//! - `state`: Server state management
//! - `plugin_runtime`: Plugin system for extensibility
//!
//! ## Feature Flags
//!
//! - `batch-writes`: Enable batched database writes for high throughput
//! - `voice`: Enable voice chat functionality
//! - `metrics`: Enable Prometheus metrics export

pub mod args;
pub mod auth;
pub mod db;
pub mod plugin_runtime;
pub mod protocol;
pub mod state;

// Re-exports for convenience
pub use args::{Args, DbDurabilityMode};
pub use auth::{validate_auth_payload, AuthError, AuthInfo};
pub use db::{DbPool, EventStore};
pub use plugin_runtime::{
    MessageHookResult, PluginMessage, PluginMessageTarget, PluginRuntime, SlashExecutionResult,
    PLUGIN_API_VERSION,
};
pub use protocol::*;
pub use state::{BridgeInfo, Channel, ConnectionGuard, State};
