//! Client state management.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::args::ClientConfig;

const MAX_MESSAGE_HISTORY: usize = 1000;
const MAX_REACTION_EVENT_DEDUP: usize = 10_000;

pub struct ClientState {
    pub ws_tx: mpsc::UnboundedSender<String>,
    pub me: String,
    pub client_id: String,
    pub pw: String,
    pub ch: String,
    pub chs: HashMap<String, bool>,
    pub users: HashMap<String, String>,
    pub trust_store: TrustStore,
    pub chan_keys: HashMap<String, Vec<u8>>,
    pub dm_keys: HashMap<String, Vec<u8>>,
    pub priv_key: Vec<u8>,
    pub running: bool,
    pub voice_active: bool,
    pub voice_session: Option<VoiceSession>,
    pub theme: (),
    pub file_transfers: HashMap<String, FileTransfer>,
    pub message_history: VecDeque<DisplayedMessage>,
    pub message_ids_seen: HashSet<String>,
    pub status: Status,
    pub reactions: HashMap<String, HashMap<String, u32>>,
    pub seen_reaction_events: HashSet<String>,
    pub reaction_event_order: VecDeque<String>,
    pub unread_counts: HashMap<String, usize>,
    pub unread_separator_scopes: HashSet<String>,
    pub activity_hint: Option<(String, u64)>,
    pub typing_presence: HashMap<String, TypingPresence>,
    pub recent_sents: VecDeque<SentMessage>,
    pub log_enabled: bool,
    pub config: clifford::config::Config,
    pub screen_share: Option<()>,
    pub screen_viewing: bool,
    pub voice_members: Vec<String>,
    pub voice_muted: bool,
    pub voice_deafened: bool,
    pub voice_speaking: bool,
    pub media_enabled: bool,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub command_history: Vec<String>,
    pub history_index: Option<usize>,
    pub scroll_offset: usize,
    pub cached_header: String,
    pub cached_subtitle: String,
    pub needs_redraw: bool,
    pub client_config: ClientConfig,
}

pub struct TrustStore {
    pub peers: HashMap<String, PeerTrust>,
    pub audit_log: Vec<AuditEntry>,
}

pub struct PeerTrust {
    pub fingerprint: String,
    pub trusted_at: u64,
    pub verified: bool,
}

pub struct AuditEntry {
    pub action: String,
    pub peer: String,
    pub timestamp: u64,
    pub details: String,
}

pub struct FileTransfer {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub progress: f64,
    pub direction: FileTransferDirection,
}

pub enum FileTransferDirection {
    Upload,
    Download,
}

#[derive(Clone)]
pub struct DisplayedMessage {
    pub id: String,
    pub ts: f64,
    pub channel: String,
    pub sender: String,
    pub content: String,
    pub encrypted: bool,
    pub edited: bool,
}

pub struct Status {
    pub text: String,
    pub emoji: String,
}

pub struct TypingPresence {
    pub user: String,
    pub timestamp: u64,
}

pub struct SentMessage {
    pub id: String,
    pub channel: String,
    pub content: String,
    pub ts: f64,
}

pub struct VoiceSession {
    pub room: String,
    pub event_tx: std_mpsc::Sender<crate::voice::VoiceEvent>,
}

impl ClientState {
    pub fn new(
        ws_tx: mpsc::UnboundedSender<String>,
        client_config: ClientConfig,
        config: clifford::config::Config,
    ) -> Self {
        Self {
            ws_tx,
            me: String::new(),
            client_id: String::new(),
            pw: String::new(),
            ch: "general".to_string(),
            chs: HashMap::new(),
            users: HashMap::new(),
            trust_store: TrustStore {
                peers: HashMap::new(),
                audit_log: Vec::new(),
            },
            chan_keys: HashMap::new(),
            dm_keys: HashMap::new(),
            priv_key: Vec::new(),
            running: true,
            voice_active: false,
            voice_session: None,
            theme: (),
            file_transfers: HashMap::new(),
            message_history: VecDeque::new(),
            message_ids_seen: HashSet::new(),
            status: Status {
                text: "Online".to_string(),
                emoji: String::new(),
            },
            reactions: HashMap::new(),
            seen_reaction_events: HashSet::new(),
            reaction_event_order: VecDeque::new(),
            unread_counts: HashMap::new(),
            unread_separator_scopes: HashSet::new(),
            activity_hint: None,
            typing_presence: HashMap::new(),
            recent_sents: VecDeque::new(),
            log_enabled: client_config.log_enabled,
            config,
            screen_share: None,
            screen_viewing: false,
            voice_members: Vec::new(),
            voice_muted: false,
            voice_deafened: false,
            voice_speaking: false,
            media_enabled: client_config.media_enabled,
            input_buffer: String::new(),
            input_cursor: 0,
            command_history: Vec::new(),
            history_index: None,
            scroll_offset: 0,
            cached_header: String::new(),
            cached_subtitle: String::new(),
            needs_redraw: true,
            client_config,
        }
    }

    pub fn add_message(&mut self, msg: DisplayedMessage) {
        if !msg.id.is_empty() && self.message_ids_seen.contains(&msg.id) {
            return;
        }

        if !msg.id.is_empty() {
            self.message_ids_seen.insert(msg.id.clone());
        }

        self.message_history.push_back(msg);
        if self.message_history.len() > MAX_MESSAGE_HISTORY {
            if let Some(removed) = self.message_history.pop_front() {
                if !removed.id.is_empty() {
                    self.message_ids_seen.remove(&removed.id);
                }
            }
        }
    }

    pub fn add_reaction(&mut self, msg_id: &str, emoji: &str) {
        if msg_id.is_empty() || emoji.is_empty() {
            return;
        }
        let entry = self.reactions.entry(msg_id.to_string()).or_default();
        *entry.entry(emoji.to_string()).or_insert(0) += 1;
    }

    pub fn add_reaction_event(&mut self, msg_id: &str, emoji: &str, user: &str) -> bool {
        if msg_id.is_empty() || emoji.is_empty() || user.is_empty() {
            return false;
        }

        let event_key = format!("{}|{}|{}", msg_id, user, emoji);
        if !self.seen_reaction_events.insert(event_key.clone()) {
            return false;
        }

        self.reaction_event_order.push_back(event_key);
        if self.reaction_event_order.len() > MAX_REACTION_EVENT_DEDUP {
            if let Some(evicted_key) = self.reaction_event_order.pop_front() {
                self.seen_reaction_events.remove(&evicted_key);
            }
        }

        self.add_reaction(msg_id, emoji);
        true
    }

    pub fn set_reaction_count(&mut self, msg_id: &str, emoji: &str, count: u32) {
        if msg_id.is_empty() || emoji.is_empty() {
            return;
        }

        if count == 0 {
            if let Some(map) = self.reactions.get_mut(msg_id) {
                map.remove(emoji);
                if map.is_empty() {
                    self.reactions.remove(msg_id);
                }
            }
            return;
        }

        let entry = self.reactions.entry(msg_id.to_string()).or_default();
        entry.insert(emoji.to_string(), count);
    }

    pub fn reaction_summary(&self, msg_id: &str) -> String {
        let Some(map) = self.reactions.get(msg_id) else {
            return String::new();
        };

        let mut items: Vec<String> = map
            .iter()
            .map(|(emoji, count)| format!("{}{}", emoji, count))
            .collect();
        items.sort();
        if items.is_empty() {
            String::new()
        } else {
            format!("[{}]", items.join(" "))
        }
    }

    pub fn resolve_recent_message_id_in_channel(
        &self,
        channel: &str,
        one_based_index: usize,
    ) -> Option<String> {
        if one_based_index == 0 {
            return None;
        }

        self.message_history
            .iter()
            .rev()
            .filter(|msg| msg.channel == channel && !msg.id.is_empty())
            .nth(one_based_index - 1)
            .map(|msg| msg.id.clone())
    }

    pub fn send_json(&self, payload: serde_json::Value) -> Result<(), String> {
        self.ws_tx
            .send(payload.to_string())
            .map_err(|_| "failed to send websocket frame".to_string())
    }

    pub fn send_join(&self, channel: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "join",
            "ch": channel,
        }))
    }

    pub fn send_history(&self, channel: &str, limit: usize) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "history",
            "ch": channel,
            "limit": limit,
        }))
    }

    pub fn send_message(&self, channel: &str, content: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "msg",
            "ch": channel,
            "c": content,
            "ts": clifford::now(),
            "n": clifford::fresh_nonce_hex(),
        }))
    }

    pub fn send_reaction(&self, channel: &str, msg_id: &str, emoji: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "reaction",
            "ch": channel,
            "msg_id": msg_id,
            "emoji": emoji,
            "ts": clifford::now(),
            "n": clifford::fresh_nonce_hex(),
        }))
    }

    pub fn send_reaction_sync(&self, channel: &str, limit: usize) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "reaction_sync",
            "ch": channel,
            "limit": limit,
        }))
    }
}

pub type SharedState = Arc<Mutex<ClientState>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ClientConfig;

    fn make_test_state() -> ClientState {
        let (tx, _rx) = mpsc::unbounded_channel();
        ClientState::new(
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
        )
    }

    fn make_message(id: &str, channel: &str, sender: &str, content: &str) -> DisplayedMessage {
        DisplayedMessage {
            id: id.to_string(),
            ts: 0.0,
            channel: channel.to_string(),
            sender: sender.to_string(),
            content: content.to_string(),
            encrypted: true,
            edited: false,
        }
    }

    #[test]
    fn resolve_recent_message_id_respects_channel_scope() {
        let mut state = make_test_state();
        state.add_message(make_message("g-1", "general", "alice", "hi"));
        state.add_message(make_message("r-1", "random", "bob", "yo"));
        state.add_message(make_message("g-2", "general", "carol", "sup"));

        assert_eq!(
            state.resolve_recent_message_id_in_channel("general", 1),
            Some("g-2".to_string())
        );
        assert_eq!(
            state.resolve_recent_message_id_in_channel("general", 2),
            Some("g-1".to_string())
        );
        assert_eq!(
            state.resolve_recent_message_id_in_channel("general", 3),
            None
        );
        assert_eq!(
            state.resolve_recent_message_id_in_channel("random", 1),
            Some("r-1".to_string())
        );
    }

    #[test]
    fn add_reaction_event_deduplicates_same_user_message_and_emoji() {
        let mut state = make_test_state();

        assert!(state.add_reaction_event("msg-1", "+1", "alice"));
        assert!(!state.add_reaction_event("msg-1", "+1", "alice"));
        assert_eq!(
            state
                .reactions
                .get("msg-1")
                .and_then(|bucket| bucket.get("+1"))
                .copied(),
            Some(1)
        );
    }

    #[test]
    fn reaction_event_dedup_cache_is_bounded_and_evicts_oldest() {
        let mut state = make_test_state();

        for i in 0..=MAX_REACTION_EVENT_DEDUP {
            let emoji = format!("e{}", i);
            assert!(state.add_reaction_event("msg-1", &emoji, "alice"));
        }

        assert_eq!(state.reaction_event_order.len(), MAX_REACTION_EVENT_DEDUP);
        assert_eq!(state.seen_reaction_events.len(), MAX_REACTION_EVENT_DEDUP);

        // Oldest key should have been evicted and therefore accepted again.
        assert!(state.add_reaction_event("msg-1", "e0", "alice"));
    }
}
