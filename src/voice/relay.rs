//! Voice relay for broadcasting audio and state changes.
//!
//! Manages voice rooms and broadcasts voice-related events to all connected clients
//! subscribed to a room. Uses broadcast channels for efficient fan-out.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

use super::VoiceRoom;

/// Manages voice rooms and broadcasts voice events.
///
/// This is a lock-free relay that maintains voice rooms and broadcasts membership
/// changes and state updates (mute, deafen, speaking) to all connected subscribers.
pub struct VoiceRelay {
    /// Rooms indexed by name, each with read-write protection.
    rooms: Arc<RwLock<HashMap<String, Arc<RwLock<VoiceRoom>>>>>,
    /// Broadcast channel for voice events.
    tx: broadcast::Sender<VoiceBroadcast>,
}

/// Voice events to be broadcast to subscribed clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum VoiceBroadcast {
    /// List of current members in a room (sent on sync or join).
    #[serde(rename = "vusers")]
    Users {
        room: String,
        members: Vec<VoiceMemberInfo>,
    },
    /// A member's state changed (mute/deafen/speaking flags).
    #[serde(rename = "vstate")]
    StateChange {
        room: String,
        user: String,
        muted: Option<bool>,
        deafened: Option<bool>,
        speaking: Option<bool>,
    },
    /// A member's speaking status changed (activity detection).
    #[serde(rename = "vspeaking")]
    Speaking {
        room: String,
        user: String,
        speaking: bool,
    },
    /// A member joined the room.
    #[serde(rename = "vjoin")]
    MemberJoined { room: String, user: String },
    /// A member left the room.
    #[serde(rename = "vleave")]
    MemberLeft { room: String, user: String },
}

/// Snapshot of a member's voice state for distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemberInfo {
    /// Member's username.
    pub user: String,
    /// Whether the member's mic is muted.
    pub muted: bool,
    /// Whether the member's audio is deafened.
    pub deafened: bool,
    /// Whether the member is currently speaking (activity detected).
    pub speaking: bool,
    /// When they joined the room (Unix timestamp).
    pub joined_at: f64,
}

impl VoiceRelay {
    /// Creates a new voice relay with an empty room registry.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    /// Creates a new subscription to broadcast voice events.
    pub fn subscribe(&self) -> broadcast::Receiver<VoiceBroadcast> {
        self.tx.subscribe()
    }

    /// Broadcasts a voice event to all subscribers.
    pub fn broadcast(&self, event: VoiceBroadcast) {
        let _ = self.tx.send(event);
    }

    /// Gets an existing room or creates it if it doesn't exist.
    pub fn get_or_create_room(&self, name: &str) -> Arc<RwLock<VoiceRoom>> {
        let mut rooms = self.rooms.write();
        rooms
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(VoiceRoom::new(name.to_string()))))
            .clone()
    }

    /// Adds a member to a room and broadcasts the join event.
    /// Returns a snapshot of all current room members.
    pub fn join_room(&self, room: &str, username: &str) -> Vec<VoiceMemberInfo> {
        let room_guard = self.get_or_create_room(room);
        {
            let mut room = room_guard.write();
            room.add_member(username.to_string());
        }

        self.broadcast(VoiceBroadcast::MemberJoined {
            room: room.to_string(),
            user: username.to_string(),
        });

        self.get_members_internal(room)
    }

    /// Removes a member from a room and broadcasts the leave event.
    /// Returns true if the member was present and removed.
    pub fn leave_room(&self, room: &str, username: &str) -> bool {
        let room_guard = self.get_or_create_room(room);
        let removed = {
            let mut room = room_guard.write();
            room.remove_member(username)
        };

        if removed.is_some() {
            self.broadcast(VoiceBroadcast::MemberLeft {
                room: room.to_string(),
                user: username.to_string(),
            });
            true
        } else {
            false
        }
    }

    /// Updates a member's voice state (mute, deafen, speaking) and broadcasts the change.
    ///
    /// Partial updates are allowed—pass `None` for fields that should not change.
    /// This allows clients to send, e.g., only a speaking flag update without affecting mute state.
    pub fn update_member_state(
        &self,
        room: &str,
        username: &str,
        muted: Option<bool>,
        deafened: Option<bool>,
        speaking: Option<bool>,
    ) {
        let room_guard = self.get_or_create_room(room);
        {
            let mut room = room_guard.write();
            room.update_state(username, muted, deafened, speaking);
        }

        self.broadcast(VoiceBroadcast::StateChange {
            room: room.to_string(),
            user: username.to_string(),
            muted,
            deafened,
            speaking,
        });
    }

    pub fn get_members(&self, room: &str) -> Vec<VoiceMemberInfo> {
        self.get_members_internal(room)
    }

    fn get_members_internal(&self, room: &str) -> Vec<VoiceMemberInfo> {
        let room_guard = self.get_or_create_room(room);
        let room = room_guard.read();

        room.members
            .iter()
            .map(|(user, state)| VoiceMemberInfo {
                user: user.clone(),
                muted: state.muted,
                deafened: state.deafened,
                speaking: state.speaking,
                joined_at: state.joined_at,
            })
            .collect()
    }

    pub fn get_room_list(&self) -> Vec<String> {
        let rooms = self.rooms.read();
        rooms.keys().cloned().collect()
    }
}

impl Default for VoiceRelay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_room_persists_members_across_calls() {
        let relay = VoiceRelay::new();

        let joined = relay.join_room("ops", "alice");
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].user, "alice");

        let current = relay.get_members("ops");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].user, "alice");
    }

    #[test]
    fn leave_room_removes_existing_member() {
        let relay = VoiceRelay::new();
        relay.join_room("ops", "alice");

        assert!(relay.leave_room("ops", "alice"));
        assert!(relay.get_members("ops").is_empty());
    }

    #[test]
    fn update_member_state_is_visible_in_member_snapshots() {
        let relay = VoiceRelay::new();
        relay.join_room("ops", "alice");

        relay.update_member_state("ops", "alice", Some(true), Some(true), Some(true));

        let members = relay.get_members("ops");
        let alice = members
            .iter()
            .find(|member| member.user == "alice")
            .expect("alice should be present after join");
        assert!(alice.muted);
        assert!(alice.deafened);
        assert!(alice.speaking);
    }
}
