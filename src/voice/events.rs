use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum VoiceEvent {
    #[serde(rename = "vjoin")]
    Join { room: String },
    #[serde(rename = "vleave")]
    Leave { room: String },
    #[serde(rename = "vdata")]
    Data {
        room: String,
        data: String,
        from: String,
    },
    #[serde(rename = "vstate")]
    StateChange {
        room: String,
        muted: Option<bool>,
        deafened: Option<bool>,
    },
    #[serde(rename = "vspeaking")]
    Speaking { room: String, speaking: bool },
    #[serde(rename = "vusers")]
    Users { room: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VoiceState {
    #[default]
    Idle,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMember {
    pub user: String,
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
    pub joined_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemberInfo {
    pub user: String,
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
    pub joined_at: f64,
}
