//! Client state management for the Chatify terminal UI.
//!
//! This module maintains all ephemeral client-side state:
//! - Session info (username, auth token, client ID)
//! - Connection state (WebSocket sender, active channels, online users)
//! - Encryption state (per-channel keys, per-DM keys, keypair)
//! - UI state (message history, typing presence, unread markers)
//! - Trust management (fingerprint store, key change warnings)
//! - Voice and media state (room membership, speaker tracking, screen share)
//!
//! # Key Design Constraints
//!
//! **Session tokens are ephemeral** (AGENTS.md): They don't survive server restarts.
//! After a restart, the client must re-authenticate, and any prior session is invalidated.
//! This affects how we handle connection recovery and token validation.
//!
//! **Trust is explicit** (AGENTS.md): Keys are never assumed trusted (no TOFU).
//! The client maintains a `TrustStore` of fingerprints and warns on `KeyChangeWarning`
//! events. Users must manually verify via `/fingerprint` and `/trust` commands.
//!
//! **Search is O(n)** (AGENTS.md): Server performs linear decryption/scan. Clients should
//! not request huge result sets; use pagination and reasonable limits.
//!
//! # Caching & Deduplication
//!
//! - Message history is bounded to MAX_MESSAGE_HISTORY (1000) to avoid memory bloat
//! - Reaction events deduplicate via `seen_reaction_events` (bounded by MAX_REACTION_EVENT_DEDUP)
//! - Typing indicators have TTLs and are periodically purged

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use base64::{engine::general_purpose, Engine as _};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use crate::args::ClientConfig;
use crate::media::{PendingMediaTransfer, TimelinePayload};

const MAX_MESSAGE_HISTORY: usize = 1000;
const MAX_REACTION_EVENT_DEDUP: usize = 10_000;
const MAX_TRUST_AUDIT_ENTRIES: usize = 2_000;
const MAX_ACTIVITY_LOG: usize = 200;
const TRUST_STORE_SCHEMA_VERSION: u8 = 1;

/// Complete ephemeral state of a connected client session.
///
/// All fields except `client_config` and certain maps are reset when the connection
/// drops. The UI interacts directly with fields here during rendering and command
/// processing, so it's important that handlers keep them consistent.
pub struct ClientState {
    /// WebSocket sender for outbound messages (first-frame auth, then events).
    pub ws_tx: mpsc::UnboundedSender<String>,
    /// Authenticated username.
    pub me: String,
    /// Generated client ID (used for reconciliation and deduplication).
    pub client_id: String,
    /// Client's password hash (stored locally for re-auth after disconnects).
    pub pw: String,
    /// Currently active channel name (e.g., "general" or "dm:alice").
    pub ch: String,
    /// Known channels (name → subscribed flag).
    pub chs: HashMap<String, bool>,
    /// Online users (username → base64 public key for E2E DM encryption).
    pub users: HashMap<String, String>,
    /// Trust store: fingerprints, key change audit log.
    pub trust_store: TrustStore,
    /// Per-channel AES keys: channel_name → 32-byte key (derived from password + channel).
    pub chan_keys: HashMap<String, Vec<u8>>,
    /// Per-DM user keys: username → 32-byte key (derived from X25519 ECDH).
    pub dm_keys: HashMap<String, Vec<u8>>,
    /// Client's Ed25519 private key (32 bytes, used to sign DMs).
    pub priv_key: Vec<u8>,
    /// Whether the connection is active.
    pub running: bool,
    /// Whether a voice session is active.
    pub voice_active: bool,
    /// Active voice session details (room, codec, latency).
    pub voice_session: Option<VoiceSession>,
    /// Theme configuration (reserved for future use).
    pub theme: (),
    /// In-progress file transfers (id → metadata).
    pub file_transfers: HashMap<String, FileTransfer>,
    pub pending_media_transfers: HashMap<String, PendingMediaTransfer>,
    /// Displayed message history (newest last, up to MAX_MESSAGE_HISTORY).
    pub message_history: VecDeque<DisplayedMessage>,
    /// Message IDs we've already seen (deduplication).
    pub message_ids_seen: HashSet<String>,
    /// Current user's presence status (online, away, dnd, etc.).
    pub status: Status,
    /// Reaction tallies: message_id → { emoji → count }.
    pub reactions: HashMap<String, HashMap<String, u32>>,
    /// Reaction event IDs we've already processed.
    pub seen_reaction_events: HashSet<String>,
    /// Time-ordered reaction event IDs (for cleanup when bounded).
    pub reaction_event_order: VecDeque<String>,
    /// Unread message counts per channel.
    pub unread_counts: HashMap<String, usize>,
    /// Last message index per channel (for visual separators).
    pub unread_markers: HashMap<String, usize>,
    /// Channels that need an unread message separator.
    pub unread_separator_scopes: HashSet<String>,
    /// Recent activity hint: (hint_text, timestamp).
    pub activity_hint: Option<(String, u64)>,
    /// Typing indicators: username → TypingPresence with TTL.
    pub typing_presence: HashMap<String, TypingPresence>,
    /// Recently sent messages (for edit/delete reconciliation).
    pub recent_sents: VecDeque<SentMessage>,
    /// Whether message logging is enabled.
    pub log_enabled: bool,
    /// Persistent user configuration (connection host, UI theme, etc.).
    pub config: clifford::config::Config,
    /// Screen share session state (reserved).
    pub screen_share: Option<()>,
    /// Whether currently viewing another user's screen.
    pub screen_viewing: bool,
    /// Count of screen frames received (metrics).
    pub screen_frames_received: u64,
    /// Last received frame sequence number.
    pub screen_last_frame_seq: Option<u64>,
    /// Username of the user whose screen we're viewing.
    pub screen_last_frame_from: Option<String>,
    /// Usernames of members in the current voice room.
    pub voice_members: Vec<String>,
    /// Whether this client's microphone is muted in voice.
    pub voice_muted: bool,
    /// Whether this client is deafened (audio output disabled).
    pub voice_deafened: bool,
    /// Whether this client is currently speaking (audio detected).
    pub voice_speaking: bool,
    /// Whether media inline rendering is enabled.
    pub media_enabled: bool,
    /// Set of online users (from presence broadcasts).
    pub online_users: HashSet<String>,
    /// Peer presence status (username → Status).
    pub peer_statuses: HashMap<String, Status>,
    /// Current line of user input in the terminal.
    pub input_buffer: String,
    /// Cursor position in input_buffer (byte offset).
    pub input_cursor: usize,
    /// History of previously entered commands.
    pub command_history: Vec<String>,
    /// Draft messages per channel (channel → draft text).
    pub drafts: HashMap<String, String>,
    /// Current position in command_history (for up/down navigation).
    pub history_index: Option<usize>,
    /// Scroll offset in message history view.
    pub scroll_offset: usize,
    /// Cached channel header (e.g., "# general" or "dm: alice").
    pub cached_header: String,
    /// Cached subtitle (e.g., "12 online").
    pub cached_subtitle: String,
    /// Whether the UI needs a full redraw.
    pub needs_redraw: bool,
    /// Client-specific configuration (cli flags, defaults).
    pub client_config: ClientConfig,
    /// Activity log for debugging and audit (recent actions).
    pub activity_log: VecDeque<ActivityEntry>,
}

/// Explicit key trust management.
///
/// Implements the trust model described in AGENTS.md: keys must be explicitly verified
/// by the user, not implicitly trusted on first use (no TOFU). This struct stores
/// fingerprint mappings and a full audit log of trust changes.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TrustStore {
    /// Map of peer username → their trusted key fingerprint and metadata.
    pub peers: HashMap<String, PeerTrust>,
    /// Full audit log of all trust-related actions (key changes, verifications, etc.).
    pub audit_log: Vec<AuditEntry>,
}

/// Trust metadata for a single peer.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerTrust {
    /// SHA256 fingerprint of the peer's Ed25519 public key (hex-encoded).
    pub fingerprint: String,
    /// Unix timestamp when the fingerprint was first trusted by the user.
    pub trusted_at: u64,
    /// Whether the user has manually verified the fingerprint (via visual inspection, etc.).
    pub verified: bool,
}

/// Audit log entry for trust changes.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    /// Action type: "trust_new", "trust_change", "verify_fingerprint", etc.
    pub action: String,
    /// Affected peer username.
    pub peer: String,
    /// Unix timestamp of the action.
    pub timestamp: u64,
    /// Details (e.g., "changed from abc123... to def456...").
    pub details: String,
}

/// Warning emitted when a peer's key changes without prior verification.
pub struct KeyChangeWarning {
    /// Username of the peer whose key changed.
    pub user: String,
    pub trusted_fingerprint: String,
    pub observed_fingerprint: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TrustStoreFile {
    schema_version: u8,
    peers: HashMap<String, PeerTrust>,
    audit_log: Vec<AuditEntry>,
}

#[derive(serde::Serialize)]
struct TrustAuditExportProfile {
    user: String,
    host: String,
    port: u16,
    tls: bool,
}

#[derive(serde::Serialize)]
struct TrustAuditExportFile {
    schema_version: u8,
    profile: TrustAuditExportProfile,
    entries: Vec<AuditEntry>,
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

pub struct OutgoingMediaMeta<'a> {
    pub channel: &'a str,
    pub file_id: &'a str,
    pub filename: &'a str,
    pub media_kind: &'a str,
    pub size: u64,
    pub mime: Option<&'a str>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone)]
pub struct DisplayedMessage {
    pub id: String,
    pub ts: f64,
    pub channel: String,
    pub sender: String,
    pub content: String,
    pub payload: Option<TimelinePayload>,
    pub encrypted: bool,
    pub edited: bool,
}

#[derive(Clone, Default)]
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

#[derive(Clone)]
pub struct ActivityEntry {
    pub ts: u64,
    pub text: String,
    pub is_error: bool,
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
            pending_media_transfers: HashMap::new(),
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
            unread_markers: HashMap::new(),
            unread_separator_scopes: HashSet::new(),
            activity_hint: None,
            typing_presence: HashMap::new(),
            recent_sents: VecDeque::new(),
            log_enabled: client_config.log_enabled,
            config,
            screen_share: None,
            screen_viewing: false,
            screen_frames_received: 0,
            screen_last_frame_seq: None,
            screen_last_frame_from: None,
            voice_members: Vec::new(),
            voice_muted: false,
            voice_deafened: false,
            voice_speaking: false,
            media_enabled: client_config.media_enabled,
            online_users: HashSet::new(),
            peer_statuses: HashMap::new(),
            input_buffer: String::new(),
            input_cursor: 0,
            command_history: Vec::new(),
            drafts: HashMap::new(),
            history_index: None,
            scroll_offset: 0,
            cached_header: String::new(),
            cached_subtitle: String::new(),
            needs_redraw: true,
            client_config,
            activity_log: VecDeque::new(),
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

    pub fn upsert_message(&mut self, msg: DisplayedMessage) {
        if msg.id.is_empty() {
            self.add_message(msg);
            return;
        }

        if let Some(existing) = self
            .message_history
            .iter_mut()
            .find(|entry| entry.id == msg.id)
        {
            *existing = msg;
            return;
        }

        self.add_message(msg);
    }

    pub fn update_message<F>(&mut self, id: &str, mut update: F) -> bool
    where
        F: FnMut(&mut DisplayedMessage),
    {
        if id.is_empty() {
            return false;
        }

        if let Some(message) = self.message_history.iter_mut().find(|entry| entry.id == id) {
            update(message);
            return true;
        }

        false
    }

    pub fn add_activity(&mut self, text: impl Into<String>, is_error: bool) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        self.activity_log.push_back(ActivityEntry {
            ts: clifford::now() as u64,
            text,
            is_error,
        });
        if self.activity_log.len() > MAX_ACTIVITY_LOG {
            let overflow = self.activity_log.len() - MAX_ACTIVITY_LOG;
            self.activity_log.drain(0..overflow);
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

    pub fn clear_unread(&mut self, scope: &str) {
        self.unread_counts.remove(scope);
        self.unread_separator_scopes.remove(scope);
    }

    pub fn mark_scope_viewed(&mut self, scope: &str) {
        let unread = self.unread_counts.remove(scope).unwrap_or(0);
        if unread > 0 {
            self.unread_markers.insert(scope.to_string(), unread);
        }
        self.unread_separator_scopes.remove(scope);
    }

    pub fn dismiss_unread_marker(&mut self, scope: &str) {
        self.unread_markers.remove(scope);
    }

    pub fn note_incoming_message(&mut self, scope: &str, from_self: bool) {
        if scope.is_empty() {
            return;
        }

        if from_self || scope == self.ch {
            self.clear_unread(scope);
            self.dismiss_unread_marker(scope);
            return;
        }

        *self.unread_counts.entry(scope.to_string()).or_insert(0) += 1;
        self.unread_separator_scopes.insert(scope.to_string());
    }

    pub fn switch_scope(&mut self, scope: String) {
        if scope.trim().is_empty() {
            return;
        }

        if self.ch == scope {
            self.mark_scope_viewed(&scope);
            let _ = self.load_draft();
            return;
        }

        self.save_draft();
        self.ch = scope;
        let current_scope = self.ch.clone();
        self.mark_scope_viewed(&current_scope);
        self.input_buffer.clear();
        self.input_cursor = 0;
        self.history_index = None;
        self.scroll_offset = 0;
        let _ = self.load_draft();
    }

    pub fn set_online_users<I>(&mut self, users: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.online_users = users.into_iter().collect();
    }

    pub fn set_peer_status(&mut self, user: &str, text: &str, emoji: &str) {
        let user = user.trim();
        if user.is_empty() {
            return;
        }

        self.peer_statuses.insert(
            user.to_string(),
            Status {
                text: if text.trim().is_empty() {
                    "Online".to_string()
                } else {
                    text.trim().to_string()
                },
                emoji: emoji.trim().to_string(),
            },
        );
    }

    fn trust_storage_root() -> PathBuf {
        if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let trimmed = appdata.trim();
                if !trimmed.is_empty() {
                    return PathBuf::from(trimmed).join("Chatify");
                }
            }
        }

        if let Ok(home) = std::env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join(".chatify");
            }
        }

        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".chatify")
    }

    fn sanitize_storage_component(raw: &str) -> String {
        let mut sanitized = String::with_capacity(raw.len());
        for ch in raw.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                sanitized.push(ch);
            } else {
                sanitized.push('_');
            }
        }

        if sanitized.is_empty() {
            "default".to_string()
        } else {
            sanitized
        }
    }

    fn storage_profile_components(&self) -> (String, String, u16, &'static str) {
        let user = if self.me.is_empty() {
            "anonymous".to_string()
        } else {
            Self::sanitize_storage_component(&self.me)
        };

        let host = if self.client_config.host.is_empty() {
            "localhost".to_string()
        } else {
            Self::sanitize_storage_component(&self.client_config.host)
        };

        let mode = if self.client_config.tls {
            "tls"
        } else {
            "plain"
        };

        (user, host, self.client_config.port, mode)
    }

    pub fn trust_store_path(&self) -> PathBuf {
        let (user, host, port, mode) = self.storage_profile_components();

        let file_name = format!("trust-{}-{}-{}-{}.json", user, host, port, mode);

        Self::trust_storage_root().join("trust").join(file_name)
    }

    pub fn trust_audit_export_path(&self) -> PathBuf {
        let (user, host, port, mode) = self.storage_profile_components();

        let file_name = format!("trust-audit-{}-{}-{}-{}.json", user, host, port, mode);
        Self::trust_storage_root().join("trust").join(file_name)
    }

    pub fn media_storage_root(&self) -> PathBuf {
        let (user, host, port, mode) = self.storage_profile_components();
        Self::trust_storage_root()
            .join("media")
            .join(format!("{}-{}-{}-{}", user, host, port, mode))
    }

    pub fn media_download_path(&self, sender: &str, file_id: &str, filename: &str) -> PathBuf {
        let sender = Self::sanitize_storage_component(sender);
        let file_id = Self::sanitize_storage_component(file_id);
        let filename = Self::sanitize_storage_component(filename);
        self.media_storage_root()
            .join(format!("{}-{}-{}", sender, file_id, filename))
    }

    fn clamp_audit_log(entries: &mut Vec<AuditEntry>) {
        if entries.len() > MAX_TRUST_AUDIT_ENTRIES {
            let overflow = entries.len() - MAX_TRUST_AUDIT_ENTRIES;
            entries.drain(0..overflow);
        }
    }

    fn sorted_trust_audit_entries(&self) -> Vec<AuditEntry> {
        let mut entries = self.trust_store.audit_log.clone();
        entries.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then_with(|| a.action.cmp(&b.action))
                .then_with(|| a.peer.cmp(&b.peer))
                .then_with(|| a.details.cmp(&b.details))
        });
        entries
    }

    fn write_atomically(path: &Path, serialized: &[u8], context: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create {} dir '{}': {}",
                    context,
                    parent.display(),
                    e
                )
            })?;
        }

        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, serialized).map_err(|e| {
            format!(
                "failed to write {} temp file '{}': {}",
                context,
                tmp_path.display(),
                e
            )
        })?;

        if path.exists() {
            fs::remove_file(path).map_err(|e| {
                format!(
                    "failed to replace existing {} '{}': {}",
                    context,
                    path.display(),
                    e
                )
            })?;
        }

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "failed to finalize {} '{}' from '{}': {}",
                context,
                path.display(),
                tmp_path.display(),
                e
            ));
        }

        Ok(())
    }

    pub fn save_media_bytes(
        &self,
        sender: &str,
        file_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, String> {
        let destination = self.media_download_path(sender, file_id, filename);
        Self::write_atomically(&destination, bytes, "media file")?;
        Ok(destination)
    }

    pub fn save_trust_store_to_path(&self, path: &Path) -> Result<(), String> {
        let payload = TrustStoreFile {
            schema_version: TRUST_STORE_SCHEMA_VERSION,
            peers: self.trust_store.peers.clone(),
            audit_log: self.trust_store.audit_log.clone(),
        };

        let serialized = serde_json::to_vec_pretty(&payload)
            .map_err(|e| format!("failed to serialize trust store: {}", e))?;

        Self::write_atomically(path, &serialized, "trust store")
    }

    pub fn export_trust_audit_to_path(&self, path: &Path) -> Result<usize, String> {
        let entries = self.sorted_trust_audit_entries();
        let payload = TrustAuditExportFile {
            schema_version: TRUST_STORE_SCHEMA_VERSION,
            profile: TrustAuditExportProfile {
                user: if self.me.is_empty() {
                    "anonymous".to_string()
                } else {
                    self.me.clone()
                },
                host: if self.client_config.host.is_empty() {
                    "localhost".to_string()
                } else {
                    self.client_config.host.clone()
                },
                port: self.client_config.port,
                tls: self.client_config.tls,
            },
            entries,
        };

        let serialized = serde_json::to_vec_pretty(&payload)
            .map_err(|e| format!("failed to serialize trust audit export: {}", e))?;

        Self::write_atomically(path, &serialized, "trust audit export")?;
        Ok(payload.entries.len())
    }

    pub fn save_trust_store(&self) -> Result<(), String> {
        self.save_trust_store_to_path(&self.trust_store_path())
    }

    pub fn load_trust_store_from_path(&mut self, path: &Path) -> Result<bool, String> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(format!(
                    "failed to read trust store '{}': {}",
                    path.display(),
                    err
                ));
            }
        };

        let parsed = match serde_json::from_str::<TrustStoreFile>(&raw) {
            Ok(file) => file,
            Err(file_err) => {
                let legacy = serde_json::from_str::<TrustStore>(&raw).map_err(|legacy_err| {
                    format!(
                        "failed to parse trust store '{}': {}; legacy parse also failed: {}",
                        path.display(),
                        file_err,
                        legacy_err
                    )
                })?;

                TrustStoreFile {
                    schema_version: 0,
                    peers: legacy.peers,
                    audit_log: legacy.audit_log,
                }
            }
        };

        if parsed.schema_version > TRUST_STORE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported trust store schema version {} (max {})",
                parsed.schema_version, TRUST_STORE_SCHEMA_VERSION
            ));
        }
        let mut audit_log = parsed.audit_log;
        Self::clamp_audit_log(&mut audit_log);

        self.trust_store = TrustStore {
            peers: parsed.peers,
            audit_log,
        };

        Ok(true)
    }

    pub fn load_trust_store(&mut self) -> Result<bool, String> {
        self.load_trust_store_from_path(&self.trust_store_path())
    }

    pub fn normalize_fingerprint(raw: &str) -> Option<String> {
        let mut normalized = String::with_capacity(64);

        for ch in raw.chars() {
            if ch.is_ascii_hexdigit() {
                normalized.push(ch.to_ascii_lowercase());
            } else if ch == ':' || ch == '-' || ch.is_ascii_whitespace() {
                continue;
            } else {
                return None;
            }
        }

        if normalized.len() == 64 {
            Some(normalized)
        } else {
            None
        }
    }

    pub fn format_fingerprint_for_display(fingerprint: &str) -> String {
        let mut grouped = String::with_capacity(fingerprint.len() + (fingerprint.len() / 8));
        for (idx, ch) in fingerprint.chars().enumerate() {
            if idx > 0 && idx % 8 == 0 {
                grouped.push(':');
            }
            grouped.push(ch);
        }
        grouped
    }

    pub fn fingerprint_for_pubkey(pubkey_b64: &str) -> Option<String> {
        let decoded = general_purpose::STANDARD.decode(pubkey_b64).ok()?;
        if decoded.len() != 32 {
            return None;
        }

        let digest = Sha256::digest(decoded);
        Some(hex::encode(digest))
    }

    pub fn append_trust_audit(&mut self, action: &str, peer: &str, details: &str) {
        self.trust_store.audit_log.push(AuditEntry {
            action: action.to_string(),
            peer: peer.to_string(),
            timestamp: clifford::now() as u64,
            details: details.to_string(),
        });

        Self::clamp_audit_log(&mut self.trust_store.audit_log);
    }

    pub fn observe_user_key(&mut self, user: &str, pubkey_b64: &str) -> Option<KeyChangeWarning> {
        if user.trim().is_empty() || pubkey_b64.trim().is_empty() {
            return None;
        }

        self.users.insert(user.to_string(), pubkey_b64.to_string());

        let observed_fingerprint = Self::fingerprint_for_pubkey(pubkey_b64)?;
        let mut warning = None;
        let mut audit = None;

        if let Some(peer) = self.trust_store.peers.get_mut(user) {
            if peer.fingerprint == observed_fingerprint {
                if !peer.verified {
                    peer.verified = true;
                    audit = Some((
                        "trust_revalidated".to_string(),
                        format!("fingerprint={}", observed_fingerprint),
                    ));
                }
            } else if peer.verified {
                let trusted_fingerprint = peer.fingerprint.clone();
                peer.verified = false;
                warning = Some(KeyChangeWarning {
                    user: user.to_string(),
                    trusted_fingerprint: trusted_fingerprint.clone(),
                    observed_fingerprint: observed_fingerprint.clone(),
                });
                audit = Some((
                    "key_change_warning".to_string(),
                    format!(
                        "trusted={} observed={}",
                        trusted_fingerprint, observed_fingerprint
                    ),
                ));
            }
        }

        if let Some((action, details)) = audit {
            self.append_trust_audit(&action, user, &details);
        }

        warning
    }

    pub fn trust_peer(&mut self, user: &str, supplied_fingerprint: &str) -> Result<String, String> {
        let Some(pubkey_b64) = self.users.get(user) else {
            return Err(format!(
                "user '{}' is not in the current key directory; run /users first",
                user
            ));
        };

        let Some(observed_fingerprint) = Self::fingerprint_for_pubkey(pubkey_b64) else {
            return Err(format!("user '{}' has an invalid public key", user));
        };

        let Some(normalized_supplied) = Self::normalize_fingerprint(supplied_fingerprint) else {
            return Err(
                "fingerprint must be 64 hex chars (separators ':' or '-' are allowed)".to_string(),
            );
        };

        if !clifford::crypto::secure_string_eq(&observed_fingerprint, &normalized_supplied) {
            self.append_trust_audit(
                "trust_failed",
                user,
                &format!(
                    "provided={} observed={}",
                    normalized_supplied, observed_fingerprint
                ),
            );
            return Err(format!("fingerprint mismatch for '{}'", user));
        }

        self.trust_store.peers.insert(
            user.to_string(),
            PeerTrust {
                fingerprint: observed_fingerprint.clone(),
                trusted_at: clifford::now() as u64,
                verified: true,
            },
        );
        self.append_trust_audit(
            "trust_set",
            user,
            &format!("fingerprint={}", observed_fingerprint),
        );

        Ok(observed_fingerprint)
    }

    pub fn ensure_peer_trusted_for_dm(&mut self, user: &str) -> Result<String, String> {
        let user = user.trim();
        if user.is_empty() {
            return Err("user cannot be empty".to_string());
        }

        let Some(pubkey_b64) = self.users.get(user).cloned() else {
            self.append_trust_audit(
                "dm_blocked_unknown_user",
                user,
                "missing from current key directory",
            );
            return Err(format!(
                "user '{}' is not in key directory; run /users first",
                user
            ));
        };

        let Some(observed_fingerprint) = Self::fingerprint_for_pubkey(&pubkey_b64) else {
            self.append_trust_audit(
                "dm_blocked_invalid_key",
                user,
                "public key could not be fingerprinted",
            );
            return Err(format!("user '{}' has an invalid public key", user));
        };

        let trusted_state = self
            .trust_store
            .peers
            .get(user)
            .map(|peer| (peer.fingerprint.clone(), peer.verified));

        match trusted_state {
            Some((trusted_fingerprint, true))
                if clifford::crypto::secure_string_eq(
                    &trusted_fingerprint,
                    &observed_fingerprint,
                ) =>
            {
                Ok(observed_fingerprint)
            }
            Some((trusted_fingerprint, false))
                if clifford::crypto::secure_string_eq(
                    &trusted_fingerprint,
                    &observed_fingerprint,
                ) =>
            {
                self.append_trust_audit(
                    "dm_blocked_unverified",
                    user,
                    &format!("fingerprint={}", observed_fingerprint),
                );
                Err(format!(
                    "peer '{}' is known but currently unverified; run /trust {} <fingerprint>",
                    user, user
                ))
            }
            Some((trusted_fingerprint, _)) => {
                if let Some(peer) = self.trust_store.peers.get_mut(user) {
                    peer.verified = false;
                }
                self.append_trust_audit(
                    "dm_blocked_key_changed",
                    user,
                    &format!(
                        "trusted={} observed={}",
                        trusted_fingerprint, observed_fingerprint
                    ),
                );
                Err(format!(
                    "trusted key for '{}' changed; re-verify with /fingerprint {} and /trust {} <fingerprint>",
                    user, user, user
                ))
            }
            None => {
                self.append_trust_audit(
                    "dm_blocked_untrusted",
                    user,
                    &format!("observed={}", observed_fingerprint),
                );
                Err(format!(
                    "peer '{}' is not trusted yet; run /fingerprint {} and /trust {} <fingerprint>",
                    user, user, user
                ))
            }
        }
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

    pub fn send_leave(&self, channel: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "leave",
            "ch": channel,
        }))
    }

    pub fn send_voice_join(&self, room: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "vjoin",
            "r": room,
        }))
    }

    pub fn send_voice_leave(&self, room: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "vleave",
            "r": room,
        }))
    }

    pub fn send_voice_state(
        &self,
        muted: Option<bool>,
        deafened: Option<bool>,
    ) -> Result<(), String> {
        let mut payload = serde_json::json!({ "t": "vstate" });
        if let Some(muted) = muted {
            payload["muted"] = serde_json::json!(muted);
        }
        if let Some(deafened) = deafened {
            payload["deafened"] = serde_json::json!(deafened);
        }
        self.send_json(payload)
    }

    pub fn send_voice_speaking(&self, speaking: bool) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "vspeaking",
            "speaking": speaking,
        }))
    }

    pub fn send_screen_start(&self, room: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "ss_start",
            "r": room,
        }))
    }

    pub fn send_screen_stop(&self, room: &str) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "ss_stop",
            "r": room,
        }))
    }

    pub fn send_history(&self, channel: &str, limit: usize) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "history",
            "ch": channel,
            "limit": limit,
        }))
    }

    pub fn send_search(&self, scope: &str, query: &str, limit: usize) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "search",
            "ch": scope,
            "q": query,
            "limit": limit,
        }))
    }

    pub fn send_replay(&self, scope: &str, from_ts: f64, limit: usize) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "replay",
            "ch": scope,
            "from_ts": from_ts,
            "limit": limit,
        }))
    }

    pub fn send_plugin_list(&self) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "plugin",
            "sub": "list",
        }))
    }

    pub fn send_plugin_install(&self, plugin: &str) -> Result<(), String> {
        let plugin = plugin.trim();
        if plugin.is_empty() {
            return Err("plugin install target cannot be empty".to_string());
        }

        self.send_json(serde_json::json!({
            "t": "plugin",
            "sub": "install",
            "plugin": plugin,
        }))
    }

    pub fn send_plugin_disable(&self, plugin: &str) -> Result<(), String> {
        let plugin = plugin.trim();
        if plugin.is_empty() {
            return Err("plugin disable target cannot be empty".to_string());
        }

        self.send_json(serde_json::json!({
            "t": "plugin",
            "sub": "disable",
            "plugin": plugin,
        }))
    }

    #[cfg(feature = "bridge-client")]
    pub fn send_bridge_status(&self) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "bridge_status",
        }))
    }

    pub fn send_typing(&self, scope: &str, typing: bool) -> Result<(), String> {
        let scope = scope.trim();
        if let Some(target) = scope.strip_prefix("dm:") {
            let to = target.trim().to_ascii_lowercase();
            if to.is_empty() {
                return Err("typing dm target cannot be empty".to_string());
            }

            return self.send_json(serde_json::json!({
                "t": "typing",
                "to": to,
                "typing": typing,
            }));
        }

        if scope.is_empty() {
            return Err("typing channel cannot be empty".to_string());
        }

        self.send_json(serde_json::json!({
            "t": "typing",
            "ch": scope,
            "typing": typing,
        }))
    }

    pub fn send_metrics(&self) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "metrics",
        }))
    }

    pub fn send_db_profile(&self) -> Result<(), String> {
        self.send_json(serde_json::json!({
            "t": "db_profile",
        }))
    }

    pub fn send_dm(&self, to_user: &str, content: &str) -> Result<(), String> {
        let to = to_user.trim().to_ascii_lowercase();
        let body = content.trim();
        if to.is_empty() {
            return Err("dm target cannot be empty".to_string());
        }
        if body.is_empty() {
            return Err("dm content cannot be empty".to_string());
        }

        self.send_json(serde_json::json!({
            "t": "dm",
            "to": to,
            "c": body,
            "ts": clifford::now(),
            "n": clifford::fresh_nonce_hex(),
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

    pub fn send_file_meta(&self, meta: OutgoingMediaMeta<'_>) -> Result<(), String> {
        let mut payload = serde_json::json!({
            "t": "file_meta",
            "ch": meta.channel,
            "file_id": meta.file_id,
            "filename": meta.filename,
            "media_kind": meta.media_kind,
            "size": meta.size,
            "ts": clifford::now(),
            "n": clifford::fresh_nonce_hex(),
        });
        if let Some(mime) = meta.mime {
            payload["mime"] = serde_json::Value::String(mime.to_string());
        }
        if let Some(duration_ms) = meta.duration_ms {
            payload["duration_ms"] = serde_json::Value::from(duration_ms);
        }
        self.send_json(payload)
    }

    pub fn send_file_chunk(
        &self,
        channel: &str,
        file_id: &str,
        index: u64,
        data: &[u8],
    ) -> Result<(), String> {
        let chunk_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.send_json(serde_json::json!({
            "t": "file_chunk",
            "ch": channel,
            "file_id": file_id,
            "index": index,
            "data": chunk_b64,
            "ts": clifford::now(),
            "n": clifford::fresh_nonce_hex(),
        }))
    }

    pub fn save_draft(&mut self) {
        if self.input_buffer.trim().is_empty() {
            self.drafts.remove(&self.ch);
        } else {
            self.drafts
                .insert(self.ch.clone(), self.input_buffer.clone());
        }
    }

    pub fn load_draft(&mut self) -> bool {
        if let Some(content) = self.drafts.get(&self.ch).cloned() {
            self.input_buffer = content;
            self.input_cursor = self.input_buffer.chars().count();
            return true;
        }

        self.input_buffer.clear();
        self.input_cursor = 0;
        false
    }
}

pub type SharedState = Arc<Mutex<ClientState>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ClientConfig;
    use std::path::PathBuf;

    fn unique_test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "chatify-trust-{}-{}-{}.json",
            name,
            std::process::id(),
            clifford::fresh_nonce_hex()
        ))
    }

    fn make_test_state() -> ClientState {
        let (tx, _rx) = mpsc::unbounded_channel();
        ClientState::new(
            tx,
            ClientConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                tls: false,
                auto_reconnect: true,
                log_enabled: false,
                markdown_enabled: true,
                media_enabled: true,
                animations_enabled: true,
            },
            clifford::config::Config::default(),
        )
    }

    fn make_test_state_with_receiver() -> (ClientState, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = ClientState::new(
            tx,
            ClientConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                tls: false,
                auto_reconnect: true,
                log_enabled: false,
                markdown_enabled: true,
                media_enabled: true,
                animations_enabled: true,
            },
            clifford::config::Config::default(),
        );

        (state, rx)
    }

    fn make_message(id: &str, channel: &str, sender: &str, content: &str) -> DisplayedMessage {
        DisplayedMessage {
            id: id.to_string(),
            ts: 0.0,
            channel: channel.to_string(),
            sender: sender.to_string(),
            content: content.to_string(),
            payload: None,
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

    #[test]
    fn send_search_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_search("dm:alice", "deploy failed", 25)
            .expect("send_search should serialize");

        let frame = rx.try_recv().expect("search frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("search"));
        assert_eq!(parsed.get("ch").and_then(|v| v.as_str()), Some("dm:alice"));
        assert_eq!(
            parsed.get("q").and_then(|v| v.as_str()),
            Some("deploy failed")
        );
        assert_eq!(parsed.get("limit").and_then(|v| v.as_u64()), Some(25));
    }

    #[test]
    fn send_voice_join_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_voice_join("room-a")
            .expect("send_voice_join should serialize");

        let frame = rx.try_recv().expect("voice join frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("vjoin"));
        assert_eq!(parsed.get("r").and_then(|v| v.as_str()), Some("room-a"));
    }

    #[test]
    fn send_voice_state_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_voice_state(Some(true), Some(false))
            .expect("send_voice_state should serialize");

        let frame = rx.try_recv().expect("voice state frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("vstate"));
        assert_eq!(parsed.get("muted").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            parsed.get("deafened").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn send_screen_start_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_screen_start("general")
            .expect("send_screen_start should serialize");

        let frame = rx.try_recv().expect("screen start frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("ss_start"));
        assert_eq!(parsed.get("r").and_then(|v| v.as_str()), Some("general"));
    }

    #[test]
    fn send_screen_stop_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_screen_stop("general")
            .expect("send_screen_stop should serialize");

        let frame = rx.try_recv().expect("screen stop frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("ss_stop"));
        assert_eq!(parsed.get("r").and_then(|v| v.as_str()), Some("general"));
    }

    #[test]
    fn send_replay_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_replay("general", 1_711_234_567.125, 100)
            .expect("send_replay should serialize");

        let frame = rx.try_recv().expect("replay frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("replay"));
        assert_eq!(parsed.get("ch").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(parsed.get("limit").and_then(|v| v.as_u64()), Some(100));

        let from_ts = parsed
            .get("from_ts")
            .and_then(|v| v.as_f64())
            .expect("from_ts should be f64");
        assert!((from_ts - 1_711_234_567.125).abs() < f64::EPSILON);
    }

    #[test]
    fn send_file_meta_serializes_media_metadata_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_file_meta(OutgoingMediaMeta {
                channel: "general",
                file_id: "audio-1",
                filename: "voice-note.ogg",
                media_kind: "audio",
                size: 1_024,
                mime: Some("audio/ogg"),
                duration_ms: Some(4_250),
            })
            .expect("send_file_meta should serialize");

        let frame = rx.try_recv().expect("file metadata frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("file_meta"));
        assert_eq!(parsed.get("ch").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(
            parsed.get("file_id").and_then(|v| v.as_str()),
            Some("audio-1")
        );
        assert_eq!(
            parsed.get("filename").and_then(|v| v.as_str()),
            Some("voice-note.ogg")
        );
        assert_eq!(
            parsed.get("media_kind").and_then(|v| v.as_str()),
            Some("audio")
        );
        assert_eq!(parsed.get("size").and_then(|v| v.as_u64()), Some(1_024));
        assert_eq!(
            parsed.get("mime").and_then(|v| v.as_str()),
            Some("audio/ogg")
        );
        assert_eq!(
            parsed.get("duration_ms").and_then(|v| v.as_u64()),
            Some(4_250)
        );
    }

    #[test]
    fn send_file_chunk_serializes_chunk_index_and_payload() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_file_chunk("general", "image-1", 3, b"hello")
            .expect("send_file_chunk should serialize");

        let frame = rx.try_recv().expect("file chunk frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("file_chunk"));
        assert_eq!(parsed.get("ch").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(
            parsed.get("file_id").and_then(|v| v.as_str()),
            Some("image-1")
        );
        assert_eq!(parsed.get("index").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            parsed.get("data").and_then(|v| v.as_str()),
            Some("aGVsbG8=")
        );
    }

    #[test]
    fn send_plugin_list_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_plugin_list()
            .expect("send_plugin_list should serialize");

        let frame = rx.try_recv().expect("plugin list frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("plugin"));
        assert_eq!(parsed.get("sub").and_then(|v| v.as_str()), Some("list"));
        assert!(parsed.get("plugin").is_none());
    }

    #[test]
    fn send_plugin_install_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_plugin_install("builtin:poll")
            .expect("send_plugin_install should serialize");

        let frame = rx
            .try_recv()
            .expect("plugin install frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("plugin"));
        assert_eq!(parsed.get("sub").and_then(|v| v.as_str()), Some("install"));
        assert_eq!(
            parsed.get("plugin").and_then(|v| v.as_str()),
            Some("builtin:poll")
        );
    }

    #[test]
    fn send_plugin_disable_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_plugin_disable("poll")
            .expect("send_plugin_disable should serialize");

        let frame = rx
            .try_recv()
            .expect("plugin disable frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("plugin"));
        assert_eq!(parsed.get("sub").and_then(|v| v.as_str()), Some("disable"));
        assert_eq!(parsed.get("plugin").and_then(|v| v.as_str()), Some("poll"));
    }

    #[cfg(feature = "bridge-client")]
    #[test]
    fn send_bridge_status_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_bridge_status()
            .expect("send_bridge_status should serialize");

        let frame = rx.try_recv().expect("bridge status frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(
            parsed.get("t").and_then(|v| v.as_str()),
            Some("bridge_status")
        );
    }

    #[test]
    fn send_typing_channel_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_typing("general", true)
            .expect("send_typing should serialize channel scope");

        let frame = rx.try_recv().expect("typing frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("typing"));
        assert_eq!(parsed.get("ch").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(parsed.get("typing").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed.get("to").is_none());
    }

    #[test]
    fn send_typing_dm_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_typing("dm:Alice", false)
            .expect("send_typing should serialize dm scope");

        let frame = rx.try_recv().expect("typing frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("typing"));
        assert_eq!(parsed.get("to").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(parsed.get("typing").and_then(|v| v.as_bool()), Some(false));
        assert!(parsed.get("ch").is_none());
    }

    #[test]
    fn send_metrics_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state.send_metrics().expect("send_metrics should serialize");

        let frame = rx.try_recv().expect("metrics frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("metrics"));
    }

    #[test]
    fn send_db_profile_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_db_profile()
            .expect("send_db_profile should serialize");

        let frame = rx.try_recv().expect("db_profile frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("db_profile"));
    }

    #[test]
    fn send_dm_serializes_protocol_frame() {
        let (state, mut rx) = make_test_state_with_receiver();
        state
            .send_dm("Alice", "hello dm")
            .expect("send_dm should serialize");

        let frame = rx.try_recv().expect("dm frame should be queued");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("valid json frame");

        assert_eq!(parsed.get("t").and_then(|v| v.as_str()), Some("dm"));
        assert_eq!(parsed.get("to").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(parsed.get("c").and_then(|v| v.as_str()), Some("hello dm"));
        assert!(parsed.get("p").is_none());
    }

    #[test]
    fn switch_scope_persists_per_scope_drafts() {
        let mut state = make_test_state();
        state.ch = "general".to_string();
        state.input_buffer = "hello general".to_string();
        state.input_cursor = state.input_buffer.chars().count();

        state.switch_scope("dm:alice".to_string());
        assert_eq!(state.ch, "dm:alice");
        assert!(state.input_buffer.is_empty());

        state.input_buffer = "hello alice".to_string();
        state.input_cursor = state.input_buffer.chars().count();

        state.switch_scope("general".to_string());
        assert_eq!(state.input_buffer, "hello general");

        state.switch_scope("dm:alice".to_string());
        assert_eq!(state.input_buffer, "hello alice");
    }

    #[test]
    fn note_incoming_message_tracks_unread_for_inactive_scope() {
        let mut state = make_test_state();
        state.ch = "general".to_string();

        state.note_incoming_message("random", false);
        assert_eq!(state.unread_counts.get("random"), Some(&1));

        state.note_incoming_message("general", false);
        assert!(!state.unread_counts.contains_key("general"));

        state.note_incoming_message("random", true);
        assert!(!state.unread_counts.contains_key("random"));
    }

    #[test]
    fn mark_scope_viewed_converts_unread_count_into_timeline_marker() {
        let mut state = make_test_state();
        state.unread_counts.insert("general".to_string(), 3);
        state.unread_separator_scopes.insert("general".to_string());

        state.mark_scope_viewed("general");

        assert!(!state.unread_counts.contains_key("general"));
        assert_eq!(state.unread_markers.get("general"), Some(&3));
        assert!(!state.unread_separator_scopes.contains("general"));
    }

    #[test]
    fn normalize_fingerprint_accepts_grouped_variants() {
        let canonical = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let grouped = "01234567:89abcdef-01234567:89abcdef 01234567:89abcdef-01234567:89abcdef";

        assert_eq!(
            ClientState::normalize_fingerprint(canonical),
            Some(canonical.to_string())
        );
        assert_eq!(
            ClientState::normalize_fingerprint(grouped),
            Some(canonical.to_string())
        );
        assert_eq!(ClientState::normalize_fingerprint("zz"), None);
    }

    #[test]
    fn trust_peer_validates_fingerprint_and_records_audit() {
        let mut state = make_test_state();
        let priv_key = clifford::crypto::new_keypair();
        let pub_key = clifford::crypto::pub_b64(&priv_key).expect("pubkey should encode");
        state.users.insert("alice".to_string(), pub_key);

        let fingerprint = ClientState::fingerprint_for_pubkey(
            state
                .users
                .get("alice")
                .expect("alice key should be present"),
        )
        .expect("fingerprint should be derivable");

        let trusted = state
            .trust_peer("alice", &fingerprint)
            .expect("trust should succeed with matching fingerprint");
        assert_eq!(trusted, fingerprint);
        assert!(
            state
                .trust_store
                .peers
                .get("alice")
                .expect("alice trust should exist")
                .verified
        );

        let mismatch = "00".repeat(32);
        let err = state
            .trust_peer("alice", &mismatch)
            .expect_err("trust should fail with mismatched fingerprint");
        assert!(err.contains("fingerprint mismatch"));
        assert_eq!(
            state
                .trust_store
                .audit_log
                .last()
                .map(|entry| entry.action.as_str()),
            Some("trust_failed")
        );
    }

    #[test]
    fn observe_user_key_marks_verified_peer_unverified_on_change() {
        let mut state = make_test_state();

        let old_key = clifford::crypto::pub_b64(&clifford::crypto::new_keypair())
            .expect("old key should encode");
        let mut new_key = clifford::crypto::pub_b64(&clifford::crypto::new_keypair())
            .expect("new key should encode");
        if old_key == new_key {
            new_key = clifford::crypto::pub_b64(&clifford::crypto::new_keypair())
                .expect("replacement key should encode");
        }

        let trusted_fingerprint =
            ClientState::fingerprint_for_pubkey(&old_key).expect("old fingerprint should derive");
        state.trust_store.peers.insert(
            "alice".to_string(),
            PeerTrust {
                fingerprint: trusted_fingerprint.clone(),
                trusted_at: 1,
                verified: true,
            },
        );

        let warning = state
            .observe_user_key("alice", &new_key)
            .expect("key change warning should be emitted");

        assert_eq!(warning.user, "alice");
        assert_eq!(warning.trusted_fingerprint, trusted_fingerprint);
        assert!(
            !state
                .trust_store
                .peers
                .get("alice")
                .expect("alice trust should exist")
                .verified
        );
        assert_eq!(
            state
                .trust_store
                .audit_log
                .last()
                .map(|entry| entry.action.as_str()),
            Some("key_change_warning")
        );
    }

    #[test]
    fn load_trust_store_returns_false_for_missing_file() {
        let mut state = make_test_state();
        let path = unique_test_path("missing");
        if path.exists() {
            let _ = fs::remove_file(&path);
        }

        let loaded = state
            .load_trust_store_from_path(&path)
            .expect("missing file should not error");

        assert!(!loaded);
    }

    #[test]
    fn save_and_load_trust_store_roundtrip() {
        let mut state = make_test_state();
        state.me = "alice".to_string();
        state.trust_store.peers.insert(
            "bob".to_string(),
            PeerTrust {
                fingerprint: "ab".repeat(32),
                trusted_at: 123,
                verified: true,
            },
        );
        state.trust_store.audit_log.push(AuditEntry {
            action: "trust_set".to_string(),
            peer: "bob".to_string(),
            timestamp: 123,
            details: "fingerprint=test".to_string(),
        });

        let path = unique_test_path("roundtrip");
        state
            .save_trust_store_to_path(&path)
            .expect("save should succeed");

        let mut loaded = make_test_state();
        let found = loaded
            .load_trust_store_from_path(&path)
            .expect("load should succeed");

        assert!(found);
        assert_eq!(loaded.trust_store.peers.len(), 1);
        assert_eq!(loaded.trust_store.audit_log.len(), 1);
        assert_eq!(
            loaded
                .trust_store
                .peers
                .get("bob")
                .expect("peer should exist")
                .fingerprint,
            "ab".repeat(32)
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn ensure_peer_trusted_for_dm_blocks_untrusted_peer() {
        let mut state = make_test_state();
        let priv_key = clifford::crypto::new_keypair();
        let pub_key = clifford::crypto::pub_b64(&priv_key).expect("pubkey should encode");
        state.users.insert("bob".to_string(), pub_key);

        let err = state
            .ensure_peer_trusted_for_dm("bob")
            .expect_err("untrusted peer should be blocked");

        assert!(err.contains("not trusted"));
        assert_eq!(
            state
                .trust_store
                .audit_log
                .last()
                .map(|entry| entry.action.as_str()),
            Some("dm_blocked_untrusted")
        );
    }

    #[test]
    fn export_trust_audit_is_deterministic_and_sorted() {
        let mut state = make_test_state();
        state.me = "alice".to_string();
        state.trust_store.audit_log.push(AuditEntry {
            action: "z_action".to_string(),
            peer: "bob".to_string(),
            timestamp: 200,
            details: "late".to_string(),
        });
        state.trust_store.audit_log.push(AuditEntry {
            action: "a_action".to_string(),
            peer: "bob".to_string(),
            timestamp: 100,
            details: "early".to_string(),
        });

        let path = unique_test_path("audit-export");
        let count = state
            .export_trust_audit_to_path(&path)
            .expect("export should succeed");
        assert_eq!(count, 2);

        let raw = fs::read_to_string(&path).expect("export file should be readable");
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("export should parse as json");

        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_u64()),
            Some(TRUST_STORE_SCHEMA_VERSION as u64)
        );
        let entries = parsed
            .get("entries")
            .and_then(|v| v.as_array())
            .expect("entries should be present");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].get("timestamp").and_then(|v| v.as_u64()),
            Some(100)
        );
        assert_eq!(
            entries[1].get("timestamp").and_then(|v| v.as_u64()),
            Some(200)
        );

        let _ = fs::remove_file(path);
    }
}
