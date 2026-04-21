//! Client event handlers.

use crate::state::{ClientState, DisplayedMessage, KeyChangeWarning, SharedState, TypingPresence};
use crate::voice::{decode_voice_frame, VoiceEvent, VoicePlaybackPacket};
use clifford::notifications::NotificationService;

macro_rules! println {
    ($($arg:tt)*) => {{
        crate::ui::emit_output_line(format!($($arg)*), false);
    }};
}

macro_rules! eprintln {
    ($($arg:tt)*) => {{
        crate::ui::emit_output_line(format!($($arg)*), true);
    }};
}

const TYPING_TTL_SECS: u64 = 30;

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

fn format_scope_label(scope: &str) -> String {
    if scope.starts_with("dm:") {
        scope.to_string()
    } else {
        format!("#{}", scope)
    }
}

fn trust_warning_summary(warning: &KeyChangeWarning) -> String {
    format!(
        "key change detected for {}: trusted {} but observed {}. Re-verify with /fingerprint {} and /trust {} <fingerprint>",
        warning.user,
        ClientState::format_fingerprint_for_display(&warning.trusted_fingerprint),
        ClientState::format_fingerprint_for_display(&warning.observed_fingerprint),
        warning.user,
        warning.user
    )
}

fn plugin_commands_summary(value: Option<&serde_json::Value>) -> String {
    let Some(commands) = value.and_then(|v| v.as_array()) else {
        return "none".to_string();
    };

    let rendered: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            command
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| format!("/{}", name))
        })
        .collect();

    if rendered.is_empty() {
        "none".to_string()
    } else {
        rendered.join(", ")
    }
}

fn print_live_message(message: &DisplayedMessage, reaction_summary: &str) {
    if crate::ui::is_tui_active() {
        return;
    }

    if message.sender == "system" {
        println!("[system] {}", message.content);
        return;
    }

    let id = short_id(&message.id);
    let scope = format_scope_label(&message.channel);
    if reaction_summary.is_empty() {
        println!("[{}] {} {}: {}", id, scope, message.sender, message.content);
    } else {
        println!(
            "[{}] {} {}: {} {}",
            id, scope, message.sender, message.content, reaction_summary
        );
    }
}

fn is_mention_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn mention_target(username: &str) -> String {
    username.trim().trim_start_matches('@').to_ascii_lowercase()
}

pub fn format_content_for_mentions(content: &str, username: &str) -> (String, bool) {
    let target = mention_target(username);
    if target.is_empty() {
        return (content.to_string(), false);
    }

    let mut out = String::with_capacity(content.len() + 8);
    let mut cursor = 0usize;
    let mut mentioned = false;

    while cursor < content.len() {
        let Some(ch) = content[cursor..].chars().next() else {
            break;
        };

        if ch != '@' {
            out.push(ch);
            cursor += ch.len_utf8();
            continue;
        }

        let previous = content[..cursor].chars().next_back();
        if previous.map(is_mention_char).unwrap_or(false) {
            out.push(ch);
            cursor += ch.len_utf8();
            continue;
        }

        let mut end = cursor + ch.len_utf8();
        let mut token_len = 0usize;
        let mut token_lower = String::new();
        for next in content[end..].chars() {
            if !is_mention_char(next) {
                break;
            }

            end += next.len_utf8();
            token_len += 1;
            if token_len <= 32 {
                token_lower.push(next.to_ascii_lowercase());
            }
        }

        if token_len == 0 {
            out.push(ch);
            cursor += ch.len_utf8();
            continue;
        }

        let mention_text = &content[cursor..end];
        if token_len <= 32 && token_lower == target {
            out.push('[');
            out.push_str(mention_text);
            out.push(']');
            mentioned = true;
        } else {
            out.push_str(mention_text);
        }
        cursor = end;
    }

    (out, mentioned)
}

fn notify_message(
    notifications: &clifford::config::NotificationConfig,
    title: &str,
    message: &str,
) {
    let body = message.trim();
    if body.is_empty() {
        return;
    }

    NotificationService::send(notifications, title, body, false);
}

fn value_as_u64(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
                .or_else(|| {
                    v.as_f64().and_then(|n| {
                        if n.is_finite() && n >= 0.0 {
                            Some(n as u64)
                        } else {
                            None
                        }
                    })
                })
        })
        .unwrap_or(0)
}

fn value_as_f64(value: Option<&serde_json::Value>) -> f64 {
    value
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

fn print_db_pool_summary(data: &serde_json::Value) {
    let active = value_as_u64(data.get("db_pool_active"));
    let idle = value_as_u64(data.get("db_pool_idle"));
    let total = value_as_u64(data.get("db_pool_total"));
    let waiters = value_as_u64(data.get("db_pool_waiters"));

    println!(
        "DB pool: active={} idle={} total={} waiters={}",
        active, idle, total, waiters
    );
}

fn print_db_latency_budget(data: &serde_json::Value) {
    let budget = data
        .get("db_latency_budget_ms")
        .unwrap_or(&serde_json::Value::Null);
    let warning = value_as_f64(budget.get("warning_p95"));
    let critical = value_as_f64(budget.get("critical_p95"));
    let min_samples = value_as_u64(budget.get("min_samples"));

    if warning > 0.0 || critical > 0.0 || min_samples > 0 {
        println!(
            "DB latency budget p95(ms): warning={:.1} critical={:.1} min_samples={}",
            warning, critical, min_samples
        );
    }
}

fn print_db_top_ops(data: &serde_json::Value) {
    let Some(top_ops) = data.get("db_top_ops").and_then(|v| v.as_array()) else {
        println!("DB top ops: no samples yet.");
        return;
    };

    if top_ops.is_empty() {
        println!("DB top ops: no samples yet.");
        return;
    }

    println!("DB top ops (by p95):");
    for (idx, op) in top_ops.iter().enumerate() {
        let name = op
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let p50 = value_as_f64(op.get("p50_ms"));
        let p95 = value_as_f64(op.get("p95_ms"));
        let p99 = value_as_f64(op.get("p99_ms"));
        let avg = value_as_f64(op.get("avg_ms"));
        let samples = value_as_u64(op.get("samples"));
        let errors = value_as_u64(op.get("errors"));
        let error_rate = value_as_f64(op.get("error_rate"));

        println!(
            "  {}. {} p50={:.2}ms p95={:.2}ms p99={:.2}ms avg={:.2}ms samples={} errors={} err_rate={:.4}",
            idx + 1,
            name,
            p50,
            p95,
            p99,
            avg,
            samples,
            errors,
            error_rate
        );
    }
}

fn print_db_alerts(data: &serde_json::Value) {
    let Some(alerts) = data.get("db_alerts").and_then(|v| v.as_array()) else {
        return;
    };

    if alerts.is_empty() {
        println!("DB alerts: none");
        return;
    }

    println!("DB alerts:");
    for alert in alerts {
        let operation = alert
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let severity = alert
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("warning")
            .to_ascii_uppercase();
        let p95 = value_as_f64(alert.get("p95_ms"));
        let samples = value_as_u64(alert.get("samples"));
        println!(
            "  [{}] {} p95={:.2}ms samples={}",
            severity, operation, p95, samples
        );
    }
}

pub async fn handle_metrics_event(_state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let messages_sent = value_as_u64(data.get("messages_sent"));
    let messages_received = value_as_u64(data.get("messages_received"));
    let bytes_sent = value_as_u64(data.get("bytes_sent"));
    let bytes_received = value_as_u64(data.get("bytes_received"));
    let errors = value_as_u64(data.get("errors"));
    let connections_accepted = value_as_u64(data.get("connections_accepted"));
    let connections_closed = value_as_u64(data.get("connections_closed"));
    let active_connections = value_as_u64(data.get("active_connections"));
    let cache_hits = value_as_u64(data.get("cache_hits"));
    let cache_misses = value_as_u64(data.get("cache_misses"));
    let cache_hit_rate = value_as_f64(data.get("cache_hit_rate"));

    println!("Metrics snapshot:");
    println!(
        "Traffic: messages sent={} received={} bytes sent={} received={}",
        messages_sent, messages_received, bytes_sent, bytes_received
    );
    println!(
        "Connections: accepted={} closed={} active={}",
        connections_accepted, connections_closed, active_connections
    );
    println!(
        "Cache: hits={} misses={} hit_rate={:.2}%",
        cache_hits,
        cache_misses,
        cache_hit_rate * 100.0
    );
    println!("Errors: {}", errors);

    print_db_pool_summary(data);
    print_db_latency_budget(data);
    print_db_top_ops(data);
    print_db_alerts(data);
}

pub async fn handle_db_profile_event(_state: &SharedState, data: &serde_json::Value, _ts: u64) {
    println!("DB profile snapshot:");
    print_db_pool_summary(data);
    print_db_latency_budget(data);
    print_db_top_ops(data);
    print_db_alerts(data);
}

pub async fn handle_msg_event(state: &SharedState, data: &serde_json::Value, ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let u = data.get("u").and_then(|v| v.as_str()).unwrap_or("?");
    let c = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let pk = data.get("pk").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = extract_msg_id(data);
    let event_ts = extract_ts(data, ts);

    let mut state_lock = state.lock().await;
    let trust_audit_len_before = state_lock.trust_store.audit_log.len();
    let current_user = state_lock.me.clone();
    let from_self = !current_user.is_empty() && u.eq_ignore_ascii_case(&current_user);
    let notification_config = state_lock.config.notifications.clone();
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
    state_lock.note_incoming_message(ch, from_self);
    let trust_warning = state_lock.observe_user_key(u, pk);
    if let Some(warning) = trust_warning.as_ref() {
        state_lock.add_message(DisplayedMessage {
            id: String::new(),
            ts: event_ts,
            channel: String::new(),
            sender: "system".to_string(),
            content: trust_warning_summary(warning),
            encrypted: false,
            edited: false,
        });
    }
    if state_lock.trust_store.audit_log.len() != trust_audit_len_before {
        if let Err(err) = state_lock.save_trust_store() {
            eprintln!("failed to persist trust store: {}", err);
        }
    }
    let reaction_summary = state_lock.reaction_summary(&msg_id);
    drop(state_lock);

    let (rendered_content, mentioned_me) =
        format_content_for_mentions(&message.content, &current_user);
    let mut rendered_message = message.clone();
    rendered_message.content = rendered_content;
    print_live_message(&rendered_message, &reaction_summary);

    if !from_self {
        if mentioned_me && notification_config.on_mention {
            notify_message(
                &notification_config,
                &format!("Mention from {}", message.sender),
                &message.content,
            );
        } else if notification_config.on_all_messages {
            notify_message(
                &notification_config,
                &format!("Message from {}", message.sender),
                &message.content,
            );
        }
    }

    if let Some(warning) = trust_warning {
        eprintln!("[trust-warning] {}", trust_warning_summary(&warning));
    }
}

pub async fn handle_dm_event(state: &SharedState, data: &serde_json::Value, ts: u64) {
    let from = data.get("from").and_then(|v| v.as_str()).unwrap_or("?");
    let to = data.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let content = data.get("c").and_then(|v| v.as_str()).unwrap_or("");
    let pk = data.get("pk").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = extract_msg_id(data);
    let event_ts = extract_ts(data, ts);

    let mut state_lock = state.lock().await;
    let trust_audit_len_before = state_lock.trust_store.audit_log.len();
    let notification_config = state_lock.config.notifications.clone();

    let my_user = state_lock.me.clone();
    let from_is_me = !my_user.is_empty() && from.eq_ignore_ascii_case(&my_user);
    let peer = if from_is_me { to } else { from };
    let scope = format!("dm:{}", peer.to_ascii_lowercase());

    let message = DisplayedMessage {
        id: msg_id.clone(),
        ts: event_ts,
        channel: scope,
        sender: from.to_string(),
        content: content.to_string(),
        encrypted: true,
        edited: false,
    };
    state_lock.add_message(message.clone());
    state_lock.note_incoming_message(&message.channel, from_is_me);

    let trust_warning = if from_is_me {
        None
    } else {
        state_lock.observe_user_key(from, pk)
    };

    if let Some(warning) = trust_warning.as_ref() {
        state_lock.add_message(DisplayedMessage {
            id: String::new(),
            ts: event_ts,
            channel: String::new(),
            sender: "system".to_string(),
            content: trust_warning_summary(warning),
            encrypted: false,
            edited: false,
        });
    }

    if state_lock.trust_store.audit_log.len() != trust_audit_len_before {
        if let Err(err) = state_lock.save_trust_store() {
            eprintln!("failed to persist trust store: {}", err);
        }
    }

    let reaction_summary = state_lock.reaction_summary(&msg_id);
    drop(state_lock);

    let (rendered_content, mentioned_me) = format_content_for_mentions(&message.content, &my_user);
    let mut rendered_message = message.clone();
    rendered_message.content = rendered_content;
    print_live_message(&rendered_message, &reaction_summary);

    if !from_is_me {
        if notification_config.on_dm {
            notify_message(
                &notification_config,
                &format!("DM from {}", from),
                &message.content,
            );
        } else if mentioned_me && notification_config.on_mention {
            notify_message(
                &notification_config,
                &format!("Mention from {}", from),
                &message.content,
            );
        } else if notification_config.on_all_messages {
            notify_message(
                &notification_config,
                &format!("Message from {}", from),
                &message.content,
            );
        }
    }

    if let Some(warning) = trust_warning {
        eprintln!("[trust-warning] {}", trust_warning_summary(&warning));
    }
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
    let voice_capability = data
        .get("media")
        .and_then(|m| m.get("voice"))
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let screen_capability = data
        .get("media")
        .and_then(|m| m.get("screen_share"))
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let voice_codecs = data
        .get("media")
        .and_then(|m| m.get("voice"))
        .and_then(|v| v.get("codecs"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();

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
    if let Err(err) = state_lock.send_json(serde_json::json!({"t": "users"})) {
        eprintln!("failed to request users directory: {}", err);
    }
    drop(state_lock);
    println!("Connected successfully.");
    println!(
        "Media capabilities: voice={} codecs=[{}] screen_share={}",
        if voice_capability { "on" } else { "off" },
        if voice_codecs.is_empty() {
            "none"
        } else {
            &voice_codecs
        },
        if screen_capability { "on" } else { "off" }
    );
}

async fn ingest_timeline_events(
    state: &SharedState,
    scope: &str,
    events: &[serde_json::Value],
) -> (usize, Vec<String>) {
    let mut state_lock = state.lock().await;
    let mut reaction_events = 0usize;
    let current_user = state_lock.me.clone();

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
                    channel: scope.to_string(),
                    sender: sender.to_string(),
                    content: content.to_string(),
                    encrypted: true,
                    edited: false,
                });
            }
            "dm" => {
                let content = event.get("c").and_then(|v| v.as_str()).unwrap_or("");
                let sender = event
                    .get("from")
                    .or_else(|| event.get("u"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let event_ts = extract_ts(event, 0);
                state_lock.add_message(DisplayedMessage {
                    id: extract_msg_id(event),
                    ts: event_ts,
                    channel: scope.to_string(),
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
                    channel: scope.to_string(),
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
        .filter(|msg| msg.channel == scope && !msg.id.is_empty())
        .take(5)
        .map(|msg| {
            let (display_content, _) = format_content_for_mentions(&msg.content, &current_user);
            let summary = state_lock.reaction_summary(&msg.id);
            if summary.is_empty() {
                format!(
                    "  [{}] {}: {}",
                    short_id(&msg.id),
                    msg.sender,
                    display_content
                )
            } else {
                format!(
                    "  [{}] {}: {} {}",
                    short_id(&msg.id),
                    msg.sender,
                    display_content,
                    summary
                )
            }
        })
        .collect();

    (reaction_events, preview)
}

pub async fn handle_history_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let events = data
        .get("events")
        .or_else(|| data.get("msgs"))
        .and_then(|v| v.as_array());

    if let Some(events) = events {
        let (reaction_events, preview) = ingest_timeline_events(state, ch, events).await;

        println!(
            "Loaded {} events for {} ({} reaction events).",
            events.len(),
            format_scope_label(ch),
            reaction_events
        );
        if !crate::ui::is_tui_active() {
            for line in preview.iter().rev() {
                println!("{}", line);
            }
        }
    }
}

pub async fn handle_search_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let query = data.get("q").and_then(|v| v.as_str()).unwrap_or("");
    let events = data.get("events").and_then(|v| v.as_array());

    if let Some(events) = events {
        let (reaction_events, preview) = ingest_timeline_events(state, ch, events).await;
        println!(
            "Search '{}' returned {} events in {} ({} reaction events).",
            query,
            events.len(),
            format_scope_label(ch),
            reaction_events
        );
        if !crate::ui::is_tui_active() {
            for line in preview.iter().rev() {
                println!("{}", line);
            }
        }
    }
}

pub async fn handle_replay_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data.get("ch").and_then(|v| v.as_str()).unwrap_or("general");
    let from_ts = data
        .get("from_ts")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
        .unwrap_or(0.0);
    let events = data.get("events").and_then(|v| v.as_array());

    if let Some(events) = events {
        let (reaction_events, preview) = ingest_timeline_events(state, ch, events).await;
        println!(
            "Replay from ts={} returned {} events for {} ({} reaction events).",
            from_ts,
            events.len(),
            format_scope_label(ch),
            reaction_events
        );
        if !crate::ui::is_tui_active() {
            for line in preview.iter().rev() {
                println!("{}", line);
            }
        }
    }
}

pub async fn handle_plugins_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let plugins = data
        .get("plugins")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let summary = format!("Plugin inventory refreshed ({} installed).", plugins.len());

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: summary.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", summary);
    if plugins.is_empty() {
        return;
    }

    println!("Installed plugins:");
    for plugin in plugins {
        let id = plugin
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let enabled = plugin
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let api_version = plugin
            .get("api_version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let message_hook = plugin
            .get("message_hook")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let source = plugin
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let commands = plugin_commands_summary(plugin.get("commands"));

        println!(
            "  {} [{}] api=v{} hook={} commands={} source={}",
            id,
            if enabled { "enabled" } else { "disabled" },
            api_version,
            if message_hook { "on" } else { "off" },
            commands,
            source
        );
    }
}

pub async fn handle_plugin_installed_event(
    state: &SharedState,
    data: &serde_json::Value,
    _ts: u64,
) {
    let plugin = data
        .get("plugin")
        .or_else(|| data.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let api_version = data
        .get("api_version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let message_hook = data
        .get("message_hook")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let commands = plugin_commands_summary(data.get("commands"));
    let summary = format!(
        "Plugin installed: {} (api=v{}, commands={}, message_hook={}).",
        plugin,
        api_version,
        commands,
        if message_hook { "on" } else { "off" }
    );

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: summary.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", summary);
}

pub async fn handle_plugin_disabled_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let plugin = data
        .get("plugin")
        .or_else(|| data.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let summary = format!("Plugin disabled: {}.", plugin);

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: summary.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", summary);
}

pub async fn handle_bridge_status_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let bridges = data
        .get("bridges")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let count = data
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(bridges.len() as u64);
    let summary = format!("Bridge status: {} connected instance(s).", count);

    let mut state_lock = state.lock().await;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: summary.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", summary);
    if bridges.is_empty() {
        return;
    }

    println!("Connected bridges:");
    for bridge in bridges {
        let username = bridge
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let bridge_type = bridge
            .get("bridge_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let instance_id = bridge
            .get("instance_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let route_count = bridge
            .get("route_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let uptime_secs = bridge
            .get("uptime_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        println!(
            "  {} type={} instance={} routes={} uptime={}s",
            username, bridge_type, instance_id, route_count, uptime_secs
        );
    }
}

pub async fn handle_users_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let users = data.get("users").and_then(|v| v.as_array());

    if let Some(users) = users {
        let mut state_lock = state.lock().await;
        let trust_audit_len_before = state_lock.trust_store.audit_log.len();
        state_lock.users.clear();
        let mut online_users = Vec::new();
        let mut warnings = Vec::new();
        for user in users {
            let u = user.get("u").and_then(|v| v.as_str()).unwrap_or("");
            let pk = user.get("pk").and_then(|v| v.as_str()).unwrap_or("");
            let status = user.get("status").unwrap_or(&serde_json::Value::Null);
            let status_text = status
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("Online");
            let status_emoji = status.get("emoji").and_then(|v| v.as_str()).unwrap_or("");

            if !u.trim().is_empty() {
                online_users.push(u.to_string());
                state_lock.set_peer_status(u, status_text, status_emoji);
            }

            if let Some(warning) = state_lock.observe_user_key(u, pk) {
                let summary = trust_warning_summary(&warning);
                state_lock.add_message(DisplayedMessage {
                    id: String::new(),
                    ts: 0.0,
                    channel: String::new(),
                    sender: "system".to_string(),
                    content: summary.clone(),
                    encrypted: false,
                    edited: false,
                });
                warnings.push(summary);
            }
        }
        state_lock.set_online_users(online_users);
        if state_lock.trust_store.audit_log.len() != trust_audit_len_before {
            if let Err(err) = state_lock.save_trust_store() {
                eprintln!("failed to persist trust store: {}", err);
            }
        }
        let count = state_lock.users.len();
        drop(state_lock);
        println!("Users online: {}", count);
        for warning in warnings {
            eprintln!("[trust-warning] {}", warning);
        }
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
        state_lock.chs.insert(ch.clone(), true);
        state_lock.switch_scope(ch.clone());
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

pub async fn handle_left_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let ch = data
        .get("ch")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let already_left = data
        .get("already_left")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut state_lock = state.lock().await;
    if state_lock.ch == ch {
        state_lock.switch_scope("general".to_string());
    }
    state_lock.chs.remove(&ch);

    let summary = if already_left {
        format!("Left #{} (already inactive)", ch)
    } else {
        format!("Left #{}", ch)
    };

    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: ch.clone(),
        sender: "system".to_string(),
        content: summary.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", summary);
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
    let emoji = status.get("emoji").and_then(|v| v.as_str()).unwrap_or("");

    let mut state_lock = state.lock().await;
    if !state_lock.me.is_empty() && user.eq_ignore_ascii_case(&state_lock.me) {
        state_lock.status.text = summary.to_string();
        state_lock.status.emoji = emoji.to_string();
    } else {
        state_lock.set_peer_status(user, summary, emoji);
        if !user.trim().is_empty() {
            state_lock.online_users.insert(user.to_string());
        }
    }
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

pub async fn handle_typing_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let user = data
        .get("u")
        .or_else(|| data.get("from"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if user.is_empty() {
        return;
    }

    let typing = data.get("typing").and_then(|v| v.as_bool()).unwrap_or(true);
    let now_ts = clifford::now() as u64;
    let event_ts = extract_ts(data, now_ts) as u64;

    let mut state_lock = state.lock().await;
    if !state_lock.me.is_empty() && user.eq_ignore_ascii_case(&state_lock.me) {
        return;
    }

    let fallback_scope = state_lock.ch.clone();
    let raw_scope = data
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .or_else(|| {
            data.get("ch")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
        })
        .or_else(|| {
            data.get("to")
                .and_then(|v| v.as_str())
                .map(|peer| format!("dm:{}", peer.to_ascii_lowercase()))
        })
        .unwrap_or_else(|| fallback_scope.clone());

    let scope = if raw_scope.starts_with("dm:") {
        let peer = raw_scope
            .trim_start_matches("dm:")
            .trim()
            .to_ascii_lowercase();
        if peer.is_empty() {
            fallback_scope
        } else {
            format!("dm:{}", peer)
        }
    } else {
        clifford::normalize_channel(raw_scope.trim_start_matches('#')).unwrap_or(fallback_scope)
    };

    let key = format!("{}|{}", scope, user.to_ascii_lowercase());
    if typing {
        state_lock.typing_presence.insert(
            key,
            TypingPresence {
                user: user.to_string(),
                timestamp: event_ts,
            },
        );
    } else {
        state_lock.typing_presence.remove(&key);
    }

    let cutoff = now_ts.saturating_sub(TYPING_TTL_SECS);
    state_lock
        .typing_presence
        .retain(|_, presence| presence.timestamp >= cutoff);

    let scope_prefix = format!("{}|", scope);
    let mut active_users: Vec<String> = state_lock
        .typing_presence
        .iter()
        .filter_map(|(k, presence)| {
            if k.starts_with(&scope_prefix) && presence.timestamp >= cutoff {
                Some(presence.user.clone())
            } else {
                None
            }
        })
        .collect();
    active_users.sort_by_key(|name| name.to_ascii_lowercase());
    active_users.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    drop(state_lock);

    if crate::ui::is_tui_active() || !typing || active_users.is_empty() {
        return;
    }

    let scope_label = format_scope_label(&scope);
    if active_users.len() == 1 {
        println!("[typing] {} is typing in {}", active_users[0], scope_label);
    } else {
        println!(
            "[typing] {} are typing in {}",
            active_users.join(", "),
            scope_label
        );
    }
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
    let source = data
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("remote")
        .to_string();
    let seq = data.get("seq").and_then(|v| v.as_u64());
    let capture_ts_ms = data.get("capture_ts_ms").and_then(|v| v.as_u64());

    if let Some(frame) = decode_voice_frame(payload) {
        if let Some(session) = &state.lock().await.voice_session {
            let _ = session
                .event_tx
                .send(VoiceEvent::PlaybackPacket(VoicePlaybackPacket {
                    source,
                    seq,
                    capture_ts_ms,
                    frame,
                }));
        }
    }
}

pub async fn handle_ss_meta_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let room = data
        .get("room")
        .or_else(|| data.get("r"))
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let from = data
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("remote");
    let codec = data
        .get("codec")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let width = data.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
    let height = data.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let fps = data.get("fps").and_then(|v| v.as_u64()).unwrap_or(0);

    let content = if width > 0 && height > 0 && fps > 0 {
        format!(
            "🖥 screen stream from {} in #{} ({}, {}x{} @ {}fps)",
            from, room, codec, width, height, fps
        )
    } else {
        format!("🖥 screen stream from {} in #{} ({})", from, room, codec)
    };

    let mut state_lock = state.lock().await;
    state_lock.screen_share = Some(());
    state_lock.screen_viewing = true;
    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: content.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", content);
}

pub async fn handle_ss_frame_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let payload = data
        .get("a")
        .or_else(|| data.get("data"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload.is_empty() {
        return;
    }

    let room = data
        .get("room")
        .or_else(|| data.get("r"))
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let from = data
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("remote")
        .to_string();
    let seq = data.get("seq").and_then(|v| v.as_u64());
    let keyframe = data
        .get("keyframe")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut state_lock = state.lock().await;
    state_lock.screen_share = Some(());
    state_lock.screen_viewing = true;
    state_lock.screen_frames_received = state_lock.screen_frames_received.saturating_add(1);
    if let Some(seq) = seq {
        state_lock.screen_last_frame_seq = Some(seq);
    }
    state_lock.screen_last_frame_from = Some(from.clone());
    let frame_count = state_lock.screen_frames_received;
    drop(state_lock);

    if keyframe || frame_count == 1 {
        if let Some(seq) = seq {
            println!(
                "🖥 receiving screen frames from {} in #{} (frame={} seq={}{})",
                from,
                room,
                frame_count,
                seq,
                if keyframe { ", keyframe" } else { "" }
            );
        } else {
            println!(
                "🖥 receiving screen frames from {} in #{} (frame={}{})",
                from,
                room,
                frame_count,
                if keyframe { ", keyframe" } else { "" }
            );
        }
    }
}

pub async fn handle_ss_state_event(state: &SharedState, data: &serde_json::Value, _ts: u64) {
    let room = data
        .get("room")
        .or_else(|| data.get("r"))
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let enabled = data
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status = data
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or(if enabled { "active" } else { "inactive" });
    let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("");

    let mut state_lock = state.lock().await;
    state_lock.screen_share = if enabled { Some(()) } else { None };
    state_lock.screen_viewing = enabled;
    if !enabled {
        state_lock.screen_frames_received = 0;
        state_lock.screen_last_frame_seq = None;
        state_lock.screen_last_frame_from = None;
    }

    let content = if enabled {
        format!("🖥 screen share active in #{} ({})", room, status)
    } else if reason.is_empty() {
        format!("🖥 screen share unavailable in #{} ({})", room, status)
    } else {
        format!(
            "🖥 screen share unavailable in #{} ({}, reason={})",
            room, status, reason
        )
    };

    state_lock.add_message(DisplayedMessage {
        id: String::new(),
        ts: 0.0,
        channel: String::new(),
        sender: "system".to_string(),
        content: content.clone(),
        encrypted: false,
        edited: false,
    });
    drop(state_lock);

    println!("{}", content);
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
    if !state_lock
        .voice_members
        .iter()
        .any(|member| member.eq_ignore_ascii_case(user))
    {
        state_lock.voice_members.push(user.to_string());
    }
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
    state_lock
        .voice_members
        .retain(|member| !member.eq_ignore_ascii_case(user));
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
        "dm" => {
            handle_dm_event(state, &serde_json::Value::Object(data.clone()), ts).await;
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
        "search" => {
            handle_search_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "replay" => {
            handle_replay_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "plugins" => {
            handle_plugins_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "plugin_installed" => {
            handle_plugin_installed_event(state, &serde_json::Value::Object(data.clone()), ts)
                .await;
        }
        "plugin_disabled" => {
            handle_plugin_disabled_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "bridge_status" => {
            handle_bridge_status_event(state, &serde_json::Value::Object(data.clone()), ts).await;
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
        "left" => {
            handle_left_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "status_update" => {
            handle_status_update_event(state, &serde_json::Value::Object(data.clone())).await;
        }
        "typing" => {
            handle_typing_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "metrics" => {
            handle_metrics_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "db_profile" => {
            handle_db_profile_event(state, &serde_json::Value::Object(data.clone()), ts).await;
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
        "ss_meta" => {
            handle_ss_meta_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "ss_frame" => {
            handle_ss_frame_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        "ss_state" => {
            handle_ss_state_event(state, &serde_json::Value::Object(data.clone()), ts).await;
        }
        _ => {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{dispatch_event, format_content_for_mentions};
    use crate::{args::ClientConfig, state::ClientState};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    fn make_test_state() -> crate::state::SharedState {
        let (tx, _rx) = mpsc::unbounded_channel();
        Arc::new(Mutex::new(ClientState::new(
            tx,
            ClientConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                tls: false,
                log_enabled: false,
                markdown_enabled: true,
                media_enabled: true,
                animations_enabled: true,
            },
            clifford::config::Config::default(),
        )))
    }

    #[test]
    fn mention_highlights_current_user() {
        let (rendered, mentioned) = format_content_for_mentions("hey @Alice and @bob", "alice");
        assert!(mentioned);
        assert_eq!(rendered, "hey [@Alice] and @bob");
    }

    #[test]
    fn mention_ignores_embedded_at_signs() {
        let (rendered, mentioned) =
            format_content_for_mentions("mail alice@example.com then @carol", "alice");
        assert!(!mentioned);
        assert_eq!(rendered, "mail alice@example.com then @carol");
    }

    #[tokio::test]
    async fn dispatch_event_handles_plugin_inventory_payload() {
        let state = make_test_state();
        let payload = serde_json::json!({
            "t": "plugins",
            "plugins": [
                {
                    "id": "poll",
                    "enabled": true,
                    "api_version": "1",
                    "message_hook": false,
                    "commands": [{"name": "poll", "description": "Create a poll"}],
                    "source": "builtin:poll"
                }
            ]
        });

        let handled = dispatch_event(
            &state,
            payload.as_object().expect("payload should be object"),
        )
        .await;

        assert!(handled);
        let state_lock = state.lock().await;
        let latest = state_lock
            .message_history
            .back()
            .expect("plugin inventory summary should be recorded");
        assert_eq!(latest.sender, "system");
        assert_eq!(latest.content, "Plugin inventory refreshed (1 installed).");
    }

    #[tokio::test]
    async fn dispatch_event_handles_plugin_installed_payload() {
        let state = make_test_state();
        let payload = serde_json::json!({
            "t": "plugin_installed",
            "plugin": "poll",
            "api_version": "1",
            "message_hook": false,
            "commands": [{"name": "poll", "description": "Create a poll"}]
        });

        let handled = dispatch_event(
            &state,
            payload.as_object().expect("payload should be object"),
        )
        .await;

        assert!(handled);
        let state_lock = state.lock().await;
        let latest = state_lock
            .message_history
            .back()
            .expect("plugin installed summary should be recorded");
        assert_eq!(
            latest.content,
            "Plugin installed: poll (api=v1, commands=/poll, message_hook=off)."
        );
    }

    #[tokio::test]
    async fn dispatch_event_handles_bridge_status_payload() {
        let state = make_test_state();
        let payload = serde_json::json!({
            "t": "bridge_status",
            "count": 1,
            "bridges": [
                {
                    "username": "discordbot",
                    "bridge_type": "discord",
                    "instance_id": "abc123",
                    "route_count": 2,
                    "uptime_secs": 45
                }
            ]
        });

        let handled = dispatch_event(
            &state,
            payload.as_object().expect("payload should be object"),
        )
        .await;

        assert!(handled);
        let state_lock = state.lock().await;
        let latest = state_lock
            .message_history
            .back()
            .expect("bridge status summary should be recorded");
        assert_eq!(latest.content, "Bridge status: 1 connected instance(s).");
    }
}
