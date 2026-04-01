use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod events;
pub mod relay;

pub use events::{VoiceEvent, VoiceMember, VoiceMemberInfo, VoiceState};
pub use relay::VoiceRelay;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRoom {
    pub name: String,
    pub members: HashMap<String, VoiceMemberState>,
}

impl VoiceRoom {
    pub fn new(name: String) -> Self {
        Self {
            name,
            members: HashMap::new(),
        }
    }

    pub fn add_member(&mut self, username: String) {
        self.members.insert(username, VoiceMemberState::new());
    }

    pub fn remove_member(&mut self, username: &str) -> Option<VoiceMemberState> {
        self.members.remove(username)
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    pub fn get_members(&self) -> Vec<&str> {
        self.members.keys().map(|s| s.as_str()).collect()
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemberState {
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
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
