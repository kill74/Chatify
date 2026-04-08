//! Client event handlers.

use crate::state::{DisplayedMessage, SharedState};
use crate::voice::{decode_voice_frame, VoiceEvent};

pub async fn handle_msg_event(state: &SharedState, data: &serde_json::Value, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: ts as f64,
        channel: ch.to_string(),
        sender: u.to_string(),
        content: c.to_string(),
        encrypted: true,
        edited: false,
    });
}

pub async fn handle_err_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let msg = data.get("m").and_then(|v| v.as_str()).unwrap_or("unknown error");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: format!("Error: {}", msg),
        encrypted: false,
        edited: false,
    });
}

pub async fn handle_ok_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let mut state_lock = state.lock().await;
    state_lock.me = data.get("u").and_then(|v| v.as_str()).unwrap_or("").to_string();
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: "Connected successfully".to_string(),
        encrypted: false,
        edited: false,
    });
}

pub async fn handle_history_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let msgs = data.get("msgs").and_then(|v| v.as_array());

    if let Some(msgs) = msgs {
        let mut state_lock = state.lock().await;
        for msg in msgs {
            let content = msg.get("c").and_then(|v| v.as_str()).unwrap_or("");
            let sender = msg.get("u").and_then(|v| v.as_str()).unwrap_or("?");
            let ts = msg.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);

            state_lock.add_message(DisplayedMessage {
                id: String::new(),
                ts,
                channel: ch.to_string(),
                sender: sender.to_string(),
                content: content.to_string(),
                encrypted: true,
                edited: false,
            });
        }
    }
}

pub async fn handle_users_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let users = data.get("users").and_then(|v| v.as_array());

    if let Some(users) = users {
        let mut state_lock = state.lock().await;
        state_lock.users.clear();
        for user in users {
            let u = user.get("u").and_then(|v| v.as_str()).unwrap_or("");
            let pk = user.get("pk").and_then(|v| v.as_str()).unwrap_or("");
            state_lock.users.insert(u.to_string(), pk.to_string());
        }
    }
}

pub async fn handle_joined_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let user = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: format!("→ {} joined", user),
        encrypted: false,
        edited: false,
    });
}

pub async fn handle_vdata_event(state: &SharedState, data: &serde_json::Value) {
    let payload = data.get("a").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(frame) = decode_voice_frame(payload) {
        if let Some(session) = &state.lock().await.voice_session {
            let _ = session.event_tx.send(VoiceEvent::Playback(frame));
        }
    }
}

pub async fn handle_vusers_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let members = data.get("users").and_then(|v| v.as_array());
    if let Some(members) = members {
        let mut state_lock = state.lock().await;
        state_lock.voice_members = members
            .iter()
            .filter_map(|m| m.get("u").and_then(|v| v.as_str()).map(String::from))
            .collect();
    }
}

pub async fn handle_vstate_event(state: &SharedState, data: &serde_json::Value) {
    let m = data.get("m").and_then(|v| v.as_bool()).unwrap_or(false);
    let d = data.get("d").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut state_lock = state.lock().await;
    state_lock.voice_muted = m;
    state_lock.voice_deafened = d;
    if let Some(session) = &state_lock.voice_session {
        let _ = session.event_tx.send(VoiceEvent::MuteState(m));
    }
}

pub async fn handle_vspeaking_event(state: &SharedState, data: &serde_json::Value) {
    let speaking = data.get("s").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut state_lock = state.lock().await;
    state_lock.voice_speaking = speaking;
    if let Some(session) = &state_lock.voice_session {
        let _ = session.event_tx.send(VoiceEvent::SpeakingState(speaking));
    }
}

pub async fn handle_vjoin_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let user = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: format!("🎙 {} joined voice", user),
        encrypted: false,
        edited: false,
    });
}

pub async fn handle_vleave_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let user = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: format!("🎙 {} left voice", user),
        encrypted: false,
        edited: false,
    });
}

pub async fn dispatch_event(state: &SharedState, data: &serde_json::Map<String, serde_json::Value>) -> bool {
    let t = data.get("t").and_then(|v| v.as_str()).unwrap_or("");
    let ts = data.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);

    match t {
        "msg" => {
            handle_msg_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "err" => {
            handle_err_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "ok" => {
            handle_ok_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "history" => {
            handle_history_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "users" => {
            handle_users_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "joined" => {
            handle_joined_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "vdata" => {
            handle_vdata_event(state, &serde_json::Value::Object(data.clone())).await;
        }
        "vusers" => {
            handle_vusers_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "vstate" => {
            handle_vstate_event(state, &serde_json::Value::Object(data.clone())).await;
        }
        "vspeaking" => {
            handle_vspeaking_event(state, &serde_json::Value::Object(data.clone())).await;
        }
        "vjoin" => {
            handle_vjoin_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "vleave" => {
            handle_vleave_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        _ => {
            return false;
        }
    }
    true
}
