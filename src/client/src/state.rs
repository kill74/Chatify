//! Client state management.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::args::ClientConfig;

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
    pub status: Status,
    pub reactions: HashMap<String, HashMap<String, u32>>,
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
            status: Status {
                text: "Online".to_string(),
                emoji: String::new(),
            },
            reactions: HashMap::new(),
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
        self.message_history.push_back(msg);
        if self.message_history.len() > 1000 {
            self.message_history.pop_front();
        }
    }
}

pub type SharedState = Arc<Mutex<ClientState>>;
