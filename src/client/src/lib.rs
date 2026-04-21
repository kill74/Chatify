//! Client library - modular structure for Chatify client.

pub mod args;
pub mod handlers;
pub mod state;
pub mod ui;
pub mod voice;

// Re-exports
pub use crate::args::Args;
pub use crate::state::ClientState;
pub use crate::voice::{VoiceEvent, VoiceFrame, VoiceSession};
