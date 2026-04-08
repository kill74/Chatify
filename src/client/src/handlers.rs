//! Client event handlers.

use crate::state::{DisplayedMessage, SharedState};
use crate::voice::{decode_voice_frame, VoiceEvent};

fn extract_msg_id(data: &serde_json::Value) -> String {
    data.get("msg_id")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("id").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

fn extract_ts(data: &serde_json::Value, fallback: u64) -> f64 {
    data.get("ts")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
        .unwrap_or(fallback as f64)
}

fn short_id(id: &str) -> String {
    if id.is_empty() {
        "--------".to_string()
    } else {
        id.chars().take(8).collect()
    }
}

fn print_live_message(message: &DisplayedMessage, reaction_summary: &str) {
    if message.sender == "system" {
        println!("[system] {}", message.content);
        return;
    }

    let id = short_id(&message.id);
    if reaction_summary.is_empty() {
        println!(
            "[{}] #{} {}: {}",
            id, message.channel, message.sender, message.content
        );
    } else {
        println!(
            "[{}] #{} {}: {} {}",
            id, message.channel, message.sender, message.content, reaction_summary
        );
    }
}

pub async fn handle_msg_event(state: &SharedState, data: &serde_json::Value, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = extract_msg_id(data);
    let event_ts = extract_ts(data, ts);

    let mut state_lock = state.lock().await;
    let message = DisplayedMessage {
        id: msg_id.clone(),
        ts: event_ts,
        channel: ch.to_string(),
        sender: u.to_string(),
        content: c.to_string(),
        encrypted: true,
        edited: false,
    };
    state_lock.add_message(message.clone());
    let reaction_summary = state_lock.reaction_summary(&msg_id);
    drop(state_lock);

    print_live_message(&message, &reaction_summary);
}

pub async fn handle_err_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let msg = data
        .get("m")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
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
    drop(state_lock);
    eprintln!("[server-error] {}", msg);
}

pub async fn handle_ok_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let mut state_lock = state.lock().await;
    state_lock.me = data
        .get("u")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: "Connected successfully".to_string(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);
    println!("Connected successfully.");
}

pub async fn handle_history_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let events = data
        .get("events")
        .or_else(|| data.get("msgs"))
        .and_then(|v| v.as_array());

    if let Some(events) = events {
        let mut state_lock = state.lock().await;
        let mut reaction_events = 0usize;

        for event in events.iter().rev() {
            let t = event.get("t").and_then(|v| v.as_str()).unwrap_or("msg");
            match t {
                "msg" => {
                    let content = event.get("c").and_then(|v| v.as_str()).unwrap_or("");
                    let sender = event.get("u").and_then(|v| v.as_str()).unwrap_or("?");
                    let event_ts = extract_ts(event, 0);
                    state_lock.add_message(DisplayedMessage {
                        id: extract_msg_id(event),
                        ts: event_ts,
                        channel: ch.to_string(),
                        sender: sender.to_string(),
                        content: content.to_string(),
                        encrypted: true,
                        edited: false,
                    });
                }
                "reaction" => {
                    let msg_id = event.get("msg_id").and_then(|v| v.as_str()).unwrap_or("");
                    let emoji = event.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
                    let user = event
                        .get("user")
                        .or_else(|| event.get("u"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if msg_id.is_empty() || emoji.is_empty() {
                        continue;
                    }

                    let applied = if user.is_empty() {
                        state_lock.add_reaction(msg_id, emoji);
                        true
                    } else {
                        state_lock.add_reaction_event(msg_id, emoji, user)
                    };

                    if applied {
                        reaction_events += 1;
                    }
                }
                "sys" => {
                    let content = event.get("m").and_then(|v| v.as_str()).unwrap_or("");
                    let event_ts = extract_ts(event, 0);
                    state_lock.add_message(DisplayedMessage {
                        id: String::new(),
                        ts: event_ts,
                        channel: ch.to_string(),
                        sender: "system".to_string(),
                        content: content.to_string(),
                        encrypted: false,
                        edited: false,
                    });
                }
                _ => {}
            }
        }

        let preview: Vec<String> = state_lock
            .message_history
            .iter()
            .rev()
            .filter(|msg| msg.channel == ch && !msg.id.is_empty())
            .take(5)
            .map(|msg| {
                let summary = state_lock.reaction_summary(&msg.id);
                if summary.is_empty() {
                    format!("  [{}] {}: {}", short_id(&msg.id), msg.sender, msg.content)
                } else {
                    format!(
                        "  [{}] {}: {} {}",
                        short_id(&msg.id),
                        msg.sender,
                        msg.content,
                        summary
                    )
                }
            })
            .collect();
        drop(state_lock);

        println!(
            "Loaded {} events for #{} ({} reaction events).",
            events.len(),
            ch,
            reaction_events
        );
        for line in preview.iter().rev() {
            println!("{}", line);
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
        let count = state_lock.users.len();
        drop(state_lock);
        println!("Users online: {}", count);
    }
}

pub async fn handle_joined_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data
        .get("ch")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let hist = data.get("hist").cloned();
    let ws_tx = {
        let mut state_lock = state.lock().await;
        state_lock.ch = ch.clone();
        state_lock.add_message(DisplayedMessage {
            id: String::new(),
            ts: 0.0,
            channel: ch.clone(),
            sender: "system".to_string(),
            content: format!("Joined #{}", ch),
            encrypted: false,
            edited: false,
        });
        state_lock.ws_tx.clone()
    };

    println!("Joined #{}", ch);

    if let Some(hist_value) = hist {
        let payload = serde_json::json!({"ch": ch.clone(), "events": hist_value});
        handle_history_event(state, &payload, 0).await;
    }

    let _ = ws_tx.send(
        serde_json::json!({
            "t": "reaction_sync",
            "ch": ch,
            "limit": 500,
        })
        .to_string(),
    );
}

pub async fn handle_sys_event(state: &SharedState, data: &serde_json::Value, ts: u64) {
    let message = data
        .get("m")
        .and_then(|v| v.as_str())
        .unwrap_or("system event");
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("");
    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: extract_ts(data, ts),
        channel: ch.to_string(),
        sender: "system".to_string(),
        content: message.to_string(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);
    println!("[system] {}", message);
}

pub async fn handle_status_update_event(state: &SharedState, data: &serde_json::Value) {
    let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("?");
    let status = data
        .get("status")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let summary = status
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("Online");

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: format!("{} is now {}", user, summary),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);
    println!("[status] {} -> {}", user, summary);
}

pub async fn handle_reaction_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let msg_id = data.get("msg_id").and_then(|v| v.as_str()).unwrap_or("");
    let emoji = data.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    let user = data
        .get("user")
        .or_else(|| data.get("u"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let channel = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");

    if msg_id.is_empty() || emoji.is_empty() {
        return;
    }

    let summary = {
        let mut state_lock = state.lock().await;
        if user.is_empty() {
            state_lock.add_reaction(msg_id, emoji);
        } else {
            let _ = state_lock.add_reaction_event(msg_id, emoji, user);
        }
        state_lock.reaction_summary(msg_id)
    };

    let user_label = if user.is_empty() { "?" } else { user };

    if summary.is_empty() {
        println!(
            "[reaction] #{} {} by {} in #{}",
            short_id(msg_id),
            emoji,
            user_label,
            channel
        );
    } else {
        println!(
            "[reaction] #{} {} by {} in #{} -> {}",
            short_id(msg_id),
            emoji,
            user_label,
            channel,
            summary
        );
    }
}

pub async fn handle_reaction_sync_event(state: &SharedState, data: &serde_json::Value) {
    let channel = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let reactions = data
        .get("reactions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut applied = 0usize;
    let mut state_lock = state.lock().await;
    for item in reactions {
        let msg_id = item.get("msg_id").and_then(|v| v.as_str()).unwrap_or("");
        let emoji = item.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
        let count = item.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        if !msg_id.is_empty() && !emoji.is_empty() {
            state_lock.set_reaction_count(msg_id, emoji, count);
            applied += 1;
        }
    }
    drop(state_lock);

    println!(
        "Reaction sync complete for #{} ({} entries).",
        channel, applied
    );
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
    let members = data
        .get("users")
        .or_else(|| data.get("members"))
        .or_else(|| data.get("d").and_then(|d| d.get("members")))
        .and_then(|v| v.as_array());
    if let Some(members) = members {
        let mut state_lock = state.lock().await;
        state_lock.voice_members = members
            .iter()
            .filter_map(|m| {
                m.get("u")
                    .or_else(|| m.get("user"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| m.as_str().map(String::from))
            })
            .collect();
    }
}

pub async fn handle_vstate_event(state: &SharedState, data: &serde_json::Value) {
    let payload = data.get("d").unwrap_or(data);
    let m = payload
        .get("m")
        .or_else(|| payload.get("muted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let d = payload
        .get("d")
        .or_else(|| payload.get("deafened"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut state_lock = state.lock().await;
    state_lock.voice_muted = m;
    state_lock.voice_deafened = d;
    if let Some(session) = &state_lock.voice_session {
        let _ = session.event_tx.send(VoiceEvent::MuteState(m));
    }
}

pub async fn handle_vspeaking_event(state: &SharedState, data: &serde_json::Value) {
    let payload = data.get("d").unwrap_or(data);
    let speaking = payload
        .get("s")
        .or_else(|| payload.get("speaking"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut state_lock = state.lock().await;
    state_lock.voice_speaking = speaking;
    if let Some(session) = &state_lock.voice_session {
        let _ = session.event_tx.send(VoiceEvent::SpeakingState(speaking));
    }
}

pub async fn handle_vjoin_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let payload = data.get("d").unwrap_or(data);
    let user = payload
        .get("u")
        .or_else(|| payload.get("user"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
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
    let payload = data.get("d").unwrap_or(data);
    let user = payload
        .get("u")
        .or_else(|| payload.get("user"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
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

pub async fn dispatch_event(
    state: &SharedState,
    data: &serde_json::Map<String, serde_json::Value>,
) -> bool {
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
        "sys" => {
            handle_sys_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "joined" => {
            handle_joined_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "status_update" => {
            handle_status_update_event(state, &serde_json::Value::Object(data.clone())).await;
        }
        "reaction" => {
            handle_reaction_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "reaction_sync" => {
            handle_reaction_sync_event(state, &serde_json::Value::Object(data.clone())).await;
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
