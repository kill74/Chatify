//! Voice communication subsystem for Chatify.
//!
//! Provides low-latency voice relay over WebSocket channels with:
//! - Per-room member tracking and state management
//! - Audio codec handling and processing
//! - Real-time voice event broadcasting
//! - Mute/deafen state synchronization
//!
//! # Architecture
//!
//! - **VoiceRoom**: Manages a voice channel's member list and state
//! - **VoiceRelay**: Broadcasts audio frames to room members
//! - **AudioProcessor**: Encodes/decodes audio (Opus codec)
//! - **VoiceEvent**: Protocol messages for join/leave/speaking/audio
//!
//! # Design Notes
//!
//! Audio frames are transmitted in chunks over the WebSocket (not HTTP) to avoid
//! connection multiplexing issues. Each frame includes room name, sender, and
//! potentially audio data. Members can mute/deafen independently of the server's
//! relay—these are client-side state that signal intent to other peers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod audio;
pub mod events;
pub mod relay;

pub use audio::AudioProcessor;
pub use events::{VoiceEvent, VoiceMember, VoiceMemberInfo, VoiceState};
pub use relay::VoiceRelay;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A named voice room with member tracking.
///
/// Each voice room maintains the set of active members and their individual
/// mute/deafen/speaking states. The server broadcasts audio frames to all
/// members of the room (unless the sender is muted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRoom {
    /// Room identifier (e.g., "general", "dev-standup").
    pub name: String,
    /// Active members: username → member state.
    pub members: HashMap<String, VoiceMemberState>,
}

impl VoiceRoom {
    /// Creates a new voice room with no members.
    pub fn new(name: String) -> Self {
        Self {
            name,
            members: HashMap::new(),
        }
    }

    /// Adds a member to the room (initializes their state).
    pub fn add_member(&mut self, username: String) {
        self.members.insert(username, VoiceMemberState::new());
    }

    /// Removes a member and returns their final state.
    pub fn remove_member(&mut self, username: &str) -> Option<VoiceMemberState> {
        self.members.remove(username)
    }

    /// Returns the number of active members in the room.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Returns a list of usernames currently in the room.
    pub fn get_members(&self) -> Vec<&str> {
        self.members.keys().map(|s| s.as_str()).collect()
    }

    /// Updates a member's voice state (mute, deafen, speaking flags).
    pub fn update_state(
        &mut self,
        username: &str,
        muted: Option<bool>,
        deafened: Option<bool>,
        speaking: Option<bool>,
    ) {
        if let Some(member) = self.members.get_mut(username) {
            if let Some(m) = muted {
                member.muted = m;
            }
            if let Some(d) = deafened {
                member.deafened = d;
            }
            if let Some(s) = speaking {
                member.speaking = s;
            }
        }
    }
}

/// Per-member voice state within a room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemberState {
    /// Whether this member's microphone is muted (client-side state).
    pub muted: bool,
    /// Whether this member's audio output is deafened (client-side state).
    pub deafened: bool,
    /// Whether the member's audio activity detector has detected speech.
    pub speaking: bool,
    /// Unix timestamp (f64) when the member joined the room.
    pub joined_at: f64,
}

impl VoiceMemberState {
    pub fn new() -> Self {
        Self {
            muted: false,
            deafened: false,
            speaking: false,
            joined_at: now(),
        }
    }
}

impl Default for VoiceMemberState {
    fn default() -> Self {
        Self::new()
    }
}
