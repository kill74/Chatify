//! Terminal UI runtime and shared output sink for the Chatify client.

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use chrono::{Local, TimeZone};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use image::ImageReader;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    StatefulImage,
};

use crate::handlers;
use crate::media::{
    render_message_lines, MediaKind, RgbColor, StyledFragment, StyledLine, TimelinePayload,
};
use crate::state::{ActivityEntry, ClientState, ReplyPreview, SharedState};
use chatify::error::{ChatifyError, ChatifyResult};

#[derive(Clone, Debug)]
pub struct OutputLine {
    pub text: String,
    pub is_error: bool,
}

static OUTPUT_SINK: OnceLock<Mutex<Option<Sender<OutputLine>>>> = OnceLock::new();

fn output_sink() -> &'static Mutex<Option<Sender<OutputLine>>> {
    OUTPUT_SINK.get_or_init(|| Mutex::new(None))
}

pub fn emit_output_line(text: String, is_error: bool) {
    let text = text.trim_end_matches(['\r', '\n']).to_string();
    let Ok(guard) = output_sink().lock() else {
        if is_error {
            std::eprintln!("{}", text);
        } else {
            std::println!("{}", text);
        }
        return;
    };

    let sink = guard.as_ref().cloned();
    drop(guard);

    if let Some(tx) = sink {
        let _ = tx.send(OutputLine { text, is_error });
        return;
    }

    if is_error {
        std::eprintln!("{}", text);
    } else {
        std::println!("{}", text);
    }
}

pub fn is_tui_active() -> bool {
    output_sink()
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

struct OutputSinkGuard;

impl OutputSinkGuard {
    fn install(tx: Sender<OutputLine>) -> Self {
        if let Ok(mut guard) = output_sink().lock() {
            *guard = Some(tx);
        }
        Self
    }
}

impl Drop for OutputSinkGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = output_sink().lock() {
            *guard = None;
        }
    }
}

struct InputThread {
    alive: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    rx: Receiver<Event>,
}

impl InputThread {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Event>();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let handle = thread::spawn(move || {
            while worker_alive.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(60)) {
                    Ok(true) => {
                        if let Ok(event) = event::read() {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(false) => {}
                    Err(_) => {}
                }
            }
        });

        Self {
            alive,
            handle: Some(handle),
            rx,
        }
    }

    fn try_recv(&self) -> Result<Event, TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn enter() -> ChatifyResult<Self> {
        enable_raw_mode().map_err(ChatifyError::from)?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).map_err(ChatifyError::from)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).map_err(ChatifyError::from)?;
        terminal.clear().map_err(ChatifyError::from)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

enum UiAction {
    None,
    Execute(String),
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiLayoutMode {
    Full,
    Narrow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RightPanelMode {
    Media,
    Suggestions,
    Now,
}

#[derive(Clone, Default)]
struct PaletteState {
    open: bool,
    query: String,
    selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteActionKind {
    Prefill {
        value: &'static str,
        cursor_back: usize,
    },
    Execute(&'static str),
    ToggleVoice,
    ToggleMute,
    ToggleDeafen,
}

#[derive(Clone, Copy, Debug)]
struct PaletteAction {
    label: &'static str,
    detail: &'static str,
    keywords: &'static [&'static str],
    kind: PaletteActionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PaletteResolvedAction {
    Prefill { value: String, cursor_back: usize },
    Execute(String),
    Disabled(String),
}

const PALETTE_ACTIONS: &[PaletteAction] = &[
    PaletteAction {
        label: "Join a room",
        detail: "Open /join and type a channel name",
        keywords: &["channel", "switch", "room"],
        kind: PaletteActionKind::Prefill {
            value: "/join ",
            cursor_back: 0,
        },
    },
    PaletteAction {
        label: "Start a DM",
        detail: "Open /dm and type a person plus message",
        keywords: &["direct", "message", "private", "user"],
        kind: PaletteActionKind::Prefill {
            value: "/dm ",
            cursor_back: 0,
        },
    },
    PaletteAction {
        label: "Search this chat",
        detail: "Open /search for the active room or DM",
        keywords: &["find", "history"],
        kind: PaletteActionKind::Prefill {
            value: "/search ",
            cursor_back: 0,
        },
    },
    PaletteAction {
        label: "React to latest message",
        detail: "Open quick reaction for recent message #1",
        keywords: &["emoji"],
        kind: PaletteActionKind::Prefill {
            value: "/react #1 ",
            cursor_back: 0,
        },
    },
    PaletteAction {
        label: "Reply to latest message",
        detail: "Open quick reply for recent message #1",
        keywords: &["quote", "respond"],
        kind: PaletteActionKind::Prefill {
            value: "/reply #1 ",
            cursor_back: 0,
        },
    },
    PaletteAction {
        label: "Attach image",
        detail: "Open image upload with a quoted path",
        keywords: &["upload", "media", "photo", "picture"],
        kind: PaletteActionKind::Prefill {
            value: "/image \"\"",
            cursor_back: 1,
        },
    },
    PaletteAction {
        label: "Attach file",
        detail: "Open file upload with a quoted path",
        keywords: &["upload", "media", "document"],
        kind: PaletteActionKind::Prefill {
            value: "/file \"\"",
            cursor_back: 1,
        },
    },
    PaletteAction {
        label: "Attach video",
        detail: "Open video upload with a quoted path",
        keywords: &["upload", "media", "movie"],
        kind: PaletteActionKind::Prefill {
            value: "/video \"\"",
            cursor_back: 1,
        },
    },
    PaletteAction {
        label: "Attach audio",
        detail: "Open audio-note upload with a quoted path",
        keywords: &["upload", "media", "voice", "sound"],
        kind: PaletteActionKind::Prefill {
            value: "/audio \"\"",
            cursor_back: 1,
        },
    },
    PaletteAction {
        label: "Show people",
        detail: "Refresh users and key directory",
        keywords: &["users", "online", "presence"],
        kind: PaletteActionKind::Execute("/users"),
    },
    PaletteAction {
        label: "Toggle voice",
        detail: "Join or leave voice in the current room",
        keywords: &["call", "talk", "vc"],
        kind: PaletteActionKind::ToggleVoice,
    },
    PaletteAction {
        label: "Mute or unmute mic",
        detail: "Toggle microphone while voice is active",
        keywords: &["voice", "microphone", "mic"],
        kind: PaletteActionKind::ToggleMute,
    },
    PaletteAction {
        label: "Deafen or undeafen",
        detail: "Toggle listening while voice is active",
        keywords: &["voice", "audio", "listen"],
        kind: PaletteActionKind::ToggleDeafen,
    },
    PaletteAction {
        label: "Show help",
        detail: "List available commands",
        keywords: &["commands", "shortcuts"],
        kind: PaletteActionKind::Execute("/help"),
    },
];

#[derive(Clone)]
struct ScopeRow {
    label: String,
    unread: usize,
    is_current: bool,
    has_voice: bool,
    online: Option<bool>,
    status_text: Option<String>,
}

#[derive(Clone)]
struct TimelineEntry {
    when: String,
    sender: String,
    body_lines: Vec<StyledLine>,
    reply: Option<ReplyPreview>,
    reaction_summary: String,
    show_sender: bool,
    is_system: bool,
    is_self: bool,
    unread_divider_before: bool,
}

#[derive(Clone)]
struct VoiceSnapshot {
    active: bool,
    room: Option<String>,
    muted: bool,
    deafened: bool,
    speaking: bool,
    members: Vec<String>,
}

#[derive(Clone)]
struct ComposerSuggestion {
    value: String,
    detail: String,
}

#[derive(Clone)]
struct PresenceRow {
    user: String,
    online: bool,
    status_text: String,
}

#[derive(Clone)]
struct MediaPreviewCandidate {
    key: String,
    title: String,
    summary: String,
    path: String,
}

struct MediaPreviewRuntime {
    picker: Option<Picker>,
    protocol_label: Option<String>,
    unsupported_reason: Option<String>,
    active_key: Option<String>,
    image: Option<StatefulProtocol>,
    last_error: Option<String>,
}

#[derive(Clone)]
struct UiSnapshot {
    subtitle: String,
    current_scope: String,
    scopes: Vec<ScopeRow>,
    timeline: Vec<TimelineEntry>,
    typing_users: Vec<String>,
    activity: Vec<ActivityEntry>,
    online_people: Vec<PresenceRow>,
    composer_suggestions: Vec<ComposerSuggestion>,
    unread_marker_count: usize,
    input_buffer: String,
    input_cursor: usize,
    scroll_offset: usize,
    media_enabled: bool,
    media_preview: Option<MediaPreviewCandidate>,
    known_users: usize,
    total_unread: usize,
    dm_trust_label: Option<String>,
    voice: VoiceSnapshot,
}

impl MediaPreviewRuntime {
    fn from_terminal() -> Self {
        match Picker::from_query_stdio() {
            Ok(picker) => match picker.protocol_type() {
                ProtocolType::Halfblocks => Self {
                    picker: None,
                    protocol_label: None,
                    unsupported_reason: Some(
                        "Terminal bitmap image preview is unavailable here.".to_string(),
                    ),
                    active_key: None,
                    image: None,
                    last_error: None,
                },
                protocol_type => Self {
                    picker: Some(picker),
                    protocol_label: Some(protocol_type_label(protocol_type).to_string()),
                    unsupported_reason: None,
                    active_key: None,
                    image: None,
                    last_error: None,
                },
            },
            Err(err) => Self {
                picker: None,
                protocol_label: None,
                unsupported_reason: Some(format!("Image preview detection failed: {}", err)),
                active_key: None,
                image: None,
                last_error: None,
            },
        }
    }

    fn sync(&mut self, candidate: Option<&MediaPreviewCandidate>) {
        let Some(candidate) = candidate else {
            self.active_key = None;
            self.image = None;
            self.last_error = None;
            return;
        };

        if self.active_key.as_deref() == Some(candidate.key.as_str()) {
            return;
        }

        self.active_key = Some(candidate.key.clone());
        self.image = None;
        self.last_error = None;

        let Some(picker) = &self.picker else {
            return;
        };

        match ImageReader::open(&candidate.path) {
            Ok(reader) => match reader.decode() {
                Ok(image) => {
                    self.image = Some(picker.new_resize_protocol(image));
                }
                Err(err) => {
                    self.last_error = Some(format!("Failed to decode image: {}", err));
                }
            },
            Err(err) => {
                self.last_error = Some(format!("Failed to open image: {}", err));
            }
        }
    }

    fn record_render_result(&mut self) {
        let Some(image) = self.image.as_mut() else {
            return;
        };

        if let Some(result) = image.last_encoding_result() {
            match result {
                Ok(()) => {
                    self.last_error = None;
                }
                Err(err) => {
                    self.last_error = Some(format!("Failed to render image: {}", err));
                }
            }
        }
    }
}

impl UiSnapshot {
    fn from_state(state: &ClientState) -> Self {
        let current_scope = state.ch.clone();
        let subtitle = format!(
            "{} @ {}://{}:{}",
            if state.me.is_empty() {
                "anonymous"
            } else {
                &state.me
            },
            if state.client_config.tls { "wss" } else { "ws" },
            state.client_config.host,
            state.client_config.port
        );

        let scopes = collect_scopes(state)
            .into_iter()
            .map(|scope| {
                let (online, status_text) = if let Some(peer) = scope.strip_prefix("dm:") {
                    peer_presence_for(state, peer)
                } else {
                    (None, None)
                };

                ScopeRow {
                    label: format_scope_label(&scope),
                    unread: *state.unread_counts.get(&scope).unwrap_or(&0),
                    is_current: scope == current_scope,
                    has_voice: state
                        .voice_session
                        .as_ref()
                        .map(|session| session.room == scope)
                        .unwrap_or(false),
                    online,
                    status_text,
                }
            })
            .collect();

        let visible_messages: Vec<&crate::state::DisplayedMessage> = state
            .message_history
            .iter()
            .filter(|message| {
                message.channel == current_scope
                    || (message.channel.is_empty() && message.sender == "system")
            })
            .collect();
        let unread_marker_count = *state.unread_markers.get(&current_scope).unwrap_or(&0);
        let unreadable_messages = visible_messages
            .iter()
            .filter(|message| !message.id.is_empty())
            .count();
        let unread_divider_offset = if unread_marker_count == 0 || unreadable_messages == 0 {
            None
        } else {
            Some(unreadable_messages.saturating_sub(unread_marker_count.min(unreadable_messages)))
        };

        let mut timeline = Vec::new();
        let mut previous_sender = String::new();
        let mut previous_scope = String::new();
        let mut seen_unreadable = 0usize;
        for message in &visible_messages {
            let reaction_summary = if message.id.is_empty() {
                String::new()
            } else {
                state.reaction_summary(&message.id)
            };
            let (content, _) = handlers::format_content_for_mentions(&message.content, &state.me);
            let body_lines =
                render_message_lines(&content, message.payload.as_ref(), state.media_enabled);
            let when = format_timestamp(message.ts);
            let is_system = message.sender == "system";
            let is_self = !state.me.is_empty() && message.sender.eq_ignore_ascii_case(&state.me);
            let show_sender =
                is_system || previous_sender != message.sender || previous_scope != message.channel;
            let unread_divider_before = !message.id.is_empty()
                && unread_divider_offset
                    .map(|offset| seen_unreadable == offset)
                    .unwrap_or(false);

            timeline.push(TimelineEntry {
                when,
                sender: message.sender.clone(),
                body_lines,
                reply: message.reply.clone(),
                reaction_summary,
                show_sender,
                is_system,
                is_self,
                unread_divider_before,
            });

            if !message.id.is_empty() {
                seen_unreadable += 1;
            }
            previous_sender = message.sender.clone();
            previous_scope = message.channel.clone();
        }

        let cutoff = (chatify::now() as u64).saturating_sub(30);
        let scope_prefix = format!("{}|", current_scope);
        let mut typing_users: Vec<String> = state
            .typing_presence
            .iter()
            .filter_map(|(key, presence)| {
                if key.starts_with(&scope_prefix) && presence.timestamp >= cutoff {
                    Some(presence.user.clone())
                } else {
                    None
                }
            })
            .collect();
        typing_users.sort_by_key(|name| name.to_ascii_lowercase());
        typing_users.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let dm_trust_label = current_scope.strip_prefix("dm:").map(|peer| {
            if let Some(peer_trust) = state.trust_store.peers.get(peer) {
                if peer_trust.verified {
                    "Trusted fingerprint".to_string()
                } else {
                    "Fingerprint recorded but unverified".to_string()
                }
            } else {
                "Unverified peer".to_string()
            }
        });

        let mut voice_members = state.voice_members.clone();
        voice_members.sort_by_key(|user| user.to_ascii_lowercase());
        voice_members.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let mut online_people: Vec<PresenceRow> = state
            .users
            .keys()
            .map(|user| {
                let (online, status_text) = peer_presence_for(state, user);
                PresenceRow {
                    user: user.clone(),
                    online: online.unwrap_or(false),
                    status_text: status_text.unwrap_or_else(|| "Offline".to_string()),
                }
            })
            .collect();
        online_people.sort_by_key(|row| (!row.online, row.user.to_ascii_lowercase()));

        let media_preview = visible_messages.iter().rev().find_map(|message| {
            let Some(TimelinePayload::Media(media)) = message.payload.as_ref() else {
                return None;
            };
            if media.media_kind != MediaKind::Image {
                return None;
            }

            Some(MediaPreviewCandidate {
                key: message.id.clone(),
                title: media.filename.clone(),
                summary: media.summary_line(),
                path: media.local_path.as_ref()?.clone(),
            })
        });

        let composer_suggestions = mention_query(&state.input_buffer, state.input_cursor)
            .map(|query| mention_suggestions(state, &query.query))
            .unwrap_or_default();
        let total_unread = state.unread_counts.values().sum();

        Self {
            subtitle,
            current_scope,
            scopes,
            timeline,
            typing_users,
            activity: state.activity_log.iter().cloned().collect(),
            online_people,
            composer_suggestions,
            unread_marker_count,
            input_buffer: state.input_buffer.clone(),
            input_cursor: state.input_cursor,
            scroll_offset: state.scroll_offset,
            media_enabled: state.media_enabled,
            media_preview,
            known_users: state.users.len(),
            total_unread,
            dm_trust_label,
            voice: VoiceSnapshot {
                active: state.voice_active,
                room: state
                    .voice_session
                    .as_ref()
                    .map(|session| session.room.clone()),
                muted: state.voice_muted,
                deafened: state.voice_deafened,
                speaking: state.voice_speaking,
                members: voice_members,
            },
        }
    }
}

pub async fn run_tui_loop<F, Fut>(state: SharedState, mut submit: F) -> ChatifyResult<()>
where
    F: FnMut(SharedState, String) -> Fut,
    Fut: Future<Output = bool>,
{
    let (output_tx, output_rx) = mpsc::channel::<OutputLine>();
    let _output_guard = OutputSinkGuard::install(output_tx);
    let mut terminal = TerminalSession::enter()?;
    let mut media_preview = MediaPreviewRuntime::from_terminal();
    let mut palette = PaletteState::default();
    let input_thread = InputThread::start();

    {
        let mut state_lock = state.lock().await;
        state_lock.add_activity(
            "Chat UI ready. Press Ctrl+K for actions, Enter to send, Tab to complete mentions.",
            false,
        );
        let current_scope = state_lock.ch.clone();
        state_lock.clear_unread(&current_scope);
    }

    loop {
        let pending_output = drain_output_lines(&output_rx);
        if !pending_output.is_empty() {
            let mut state_lock = state.lock().await;
            for line in pending_output {
                state_lock.add_activity(line.text, line.is_error);
            }
        }

        let snapshot = {
            let state_lock = state.lock().await;
            UiSnapshot::from_state(&state_lock)
        };
        media_preview.sync(snapshot.media_preview.as_ref());

        terminal
            .terminal
            .draw(|frame| render(frame, &snapshot, &mut media_preview, &palette))
            .map_err(ChatifyError::from)?;
        media_preview.record_render_result();

        let mut actions = Vec::new();
        while let Ok(event) = input_thread.try_recv() {
            let action = handle_event(&state, event, &mut palette).await;
            if !matches!(action, UiAction::None) {
                actions.push(action);
            }
        }

        for action in actions {
            match action {
                UiAction::None => {}
                UiAction::Quit => return Ok(()),
                UiAction::Execute(command) => {
                    if !submit(state.clone(), command).await {
                        return Ok(());
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(33)).await;
    }
}

fn drain_output_lines(output_rx: &Receiver<OutputLine>) -> Vec<OutputLine> {
    let mut lines = Vec::new();
    while let Ok(line) = output_rx.try_recv() {
        lines.push(line);
    }
    lines
}

async fn handle_event(state: &SharedState, event: Event, palette: &mut PaletteState) -> UiAction {
    let Event::Key(key) = event else {
        return UiAction::None;
    };

    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return UiAction::None;
    }

    if matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        return UiAction::Quit;
    }

    if matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        open_palette(palette);
        return UiAction::None;
    }

    if palette.open {
        return handle_palette_event(state, key.code, key.modifiers, palette).await;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Tab, _) => {
            let mut state_lock = state.lock().await;
            apply_mention_completion(&mut state_lock);
            UiAction::None
        }
        (KeyCode::Esc, _) => {
            let mut state_lock = state.lock().await;
            state_lock.input_buffer.clear();
            state_lock.input_cursor = 0;
            state_lock.history_index = None;
            state_lock.save_draft();
            UiAction::None
        }
        (KeyCode::Enter, _) => submit_current_input(state).await,
        (KeyCode::Char('p'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            let mut state_lock = state.lock().await;
            navigate_history(&mut state_lock, true);
            UiAction::None
        }
        (KeyCode::Char('n'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            let mut state_lock = state.lock().await;
            navigate_history(&mut state_lock, false);
            UiAction::None
        }
        (KeyCode::PageUp, _) => {
            let mut state_lock = state.lock().await;
            state_lock.scroll_offset = state_lock.scroll_offset.saturating_add(6);
            UiAction::None
        }
        (KeyCode::PageDown, _) => {
            let mut state_lock = state.lock().await;
            state_lock.scroll_offset = state_lock.scroll_offset.saturating_sub(6);
            UiAction::None
        }
        (KeyCode::Up, modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            switch_scope(state, false).await;
            UiAction::None
        }
        (KeyCode::Down, modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            switch_scope(state, true).await;
            UiAction::None
        }
        (KeyCode::Char('r'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let mut state_lock = state.lock().await;
            if state_lock.input_buffer.trim().is_empty() {
                prefill_input(&mut state_lock, "/react #1 ");
            } else {
                state_lock.add_activity(
                    "Clear the composer before using Alt+R for a quick reaction.",
                    false,
                );
            }
            UiAction::None
        }
        (KeyCode::Char('i'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let mut state_lock = state.lock().await;
            if state_lock.input_buffer.trim().is_empty() {
                prefill_input(&mut state_lock, "/image \"\"");
                state_lock.input_cursor = state_lock.input_cursor.saturating_sub(1);
            } else {
                state_lock.add_activity(
                    "Clear the composer before using Alt+I for an image attachment.",
                    false,
                );
            }
            UiAction::None
        }
        (KeyCode::Char('v'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let command = {
                let state_lock = state.lock().await;
                if state_lock.voice_active {
                    "/voice off".to_string()
                } else {
                    "/voice on".to_string()
                }
            };
            UiAction::Execute(command)
        }
        (KeyCode::Char('m'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let command = {
                let state_lock = state.lock().await;
                if !state_lock.voice_active {
                    String::new()
                } else if state_lock.voice_muted {
                    "/voice unmute".to_string()
                } else {
                    "/voice mute".to_string()
                }
            };
            if command.is_empty() {
                UiAction::None
            } else {
                UiAction::Execute(command)
            }
        }
        (KeyCode::Char('d'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let command = {
                let state_lock = state.lock().await;
                if !state_lock.voice_active {
                    String::new()
                } else if state_lock.voice_deafened {
                    "/voice undeafen".to_string()
                } else {
                    "/voice deafen".to_string()
                }
            };
            if command.is_empty() {
                UiAction::None
            } else {
                UiAction::Execute(command)
            }
        }
        (KeyCode::Backspace, _) => {
            let mut state_lock = state.lock().await;
            delete_backward(&mut state_lock);
            UiAction::None
        }
        (KeyCode::Delete, _) => {
            let mut state_lock = state.lock().await;
            delete_forward(&mut state_lock);
            UiAction::None
        }
        (KeyCode::Left, _) => {
            let mut state_lock = state.lock().await;
            state_lock.input_cursor = state_lock.input_cursor.saturating_sub(1);
            UiAction::None
        }
        (KeyCode::Right, _) => {
            let mut state_lock = state.lock().await;
            let max_cursor = state_lock.input_buffer.chars().count();
            state_lock.input_cursor = (state_lock.input_cursor + 1).min(max_cursor);
            UiAction::None
        }
        (KeyCode::Home, _) => {
            let mut state_lock = state.lock().await;
            state_lock.input_cursor = 0;
            UiAction::None
        }
        (KeyCode::End, _) => {
            let mut state_lock = state.lock().await;
            state_lock.input_cursor = state_lock.input_buffer.chars().count();
            UiAction::None
        }
        (KeyCode::Char(ch), modifiers)
            if !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
        {
            let mut state_lock = state.lock().await;
            insert_char(&mut state_lock, ch);
            UiAction::None
        }
        _ => UiAction::None,
    }
}

fn open_palette(palette: &mut PaletteState) {
    palette.open = true;
    palette.query.clear();
    palette.selected = 0;
}

fn close_palette(palette: &mut PaletteState) {
    palette.open = false;
    palette.query.clear();
    palette.selected = 0;
}

async fn handle_palette_event(
    state: &SharedState,
    code: KeyCode,
    modifiers: KeyModifiers,
    palette: &mut PaletteState,
) -> UiAction {
    match code {
        KeyCode::Esc => {
            close_palette(palette);
            UiAction::None
        }
        KeyCode::Enter => activate_palette_selection(state, palette).await,
        KeyCode::Up => {
            move_palette_selection(palette, false);
            UiAction::None
        }
        KeyCode::Down => {
            move_palette_selection(palette, true);
            UiAction::None
        }
        KeyCode::Char('p') | KeyCode::Char('P') if modifiers.contains(KeyModifiers::CONTROL) => {
            move_palette_selection(palette, false);
            UiAction::None
        }
        KeyCode::Char('n') | KeyCode::Char('N') if modifiers.contains(KeyModifiers::CONTROL) => {
            move_palette_selection(palette, true);
            UiAction::None
        }
        KeyCode::Backspace => {
            palette.query.pop();
            clamp_palette_selection(palette);
            UiAction::None
        }
        KeyCode::Delete => {
            palette.query.clear();
            palette.selected = 0;
            UiAction::None
        }
        KeyCode::Char(ch) if !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => {
            palette.query.push(ch);
            palette.selected = 0;
            UiAction::None
        }
        _ => UiAction::None,
    }
}

async fn activate_palette_selection(state: &SharedState, palette: &mut PaletteState) -> UiAction {
    let Some(action) = selected_palette_action(&palette.query, palette.selected) else {
        close_palette(palette);
        return UiAction::None;
    };
    close_palette(palette);

    let resolved = {
        let state_lock = state.lock().await;
        resolve_palette_action(action, &state_lock)
    };

    match resolved {
        PaletteResolvedAction::Execute(command) => UiAction::Execute(command),
        PaletteResolvedAction::Prefill { value, cursor_back } => {
            let mut state_lock = state.lock().await;
            if !state_lock.input_buffer.trim().is_empty() {
                state_lock.add_activity(
                    "Composer already has text. Send it or press Esc before using that action.",
                    false,
                );
                return UiAction::None;
            }
            prefill_input(&mut state_lock, &value);
            state_lock.input_cursor = state_lock.input_cursor.saturating_sub(cursor_back);
            state_lock.add_activity(action.detail, false);
            UiAction::None
        }
        PaletteResolvedAction::Disabled(message) => {
            let mut state_lock = state.lock().await;
            state_lock.add_activity(message, false);
            UiAction::None
        }
    }
}

fn move_palette_selection(palette: &mut PaletteState, forward: bool) {
    let count = filtered_palette_actions(&palette.query).len();
    if count == 0 {
        palette.selected = 0;
        return;
    }

    palette.selected = if forward {
        (palette.selected + 1) % count
    } else if palette.selected == 0 {
        count - 1
    } else {
        palette.selected - 1
    };
}

fn clamp_palette_selection(palette: &mut PaletteState) {
    let count = filtered_palette_actions(&palette.query).len();
    if count == 0 {
        palette.selected = 0;
    } else {
        palette.selected = palette.selected.min(count - 1);
    }
}

fn selected_palette_action(query: &str, selected: usize) -> Option<&'static PaletteAction> {
    filtered_palette_actions(query).get(selected).copied()
}

fn filtered_palette_actions(query: &str) -> Vec<&'static PaletteAction> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.to_ascii_lowercase())
        .collect();

    PALETTE_ACTIONS
        .iter()
        .filter(|action| palette_action_matches(action, &terms))
        .collect()
}

fn palette_action_matches(action: &PaletteAction, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }

    let label = action.label.to_ascii_lowercase();
    let detail = action.detail.to_ascii_lowercase();
    terms.iter().all(|term| {
        label.contains(term)
            || detail.contains(term)
            || action
                .keywords
                .iter()
                .any(|keyword| keyword.to_ascii_lowercase().contains(term))
    })
}

fn resolve_palette_action(action: &PaletteAction, state: &ClientState) -> PaletteResolvedAction {
    match action.kind {
        PaletteActionKind::Prefill { value, cursor_back } => PaletteResolvedAction::Prefill {
            value: value.to_string(),
            cursor_back,
        },
        PaletteActionKind::Execute(command) => PaletteResolvedAction::Execute(command.to_string()),
        PaletteActionKind::ToggleVoice => {
            if state.voice_active {
                PaletteResolvedAction::Execute("/voice off".to_string())
            } else {
                PaletteResolvedAction::Execute("/voice on".to_string())
            }
        }
        PaletteActionKind::ToggleMute => {
            if !state.voice_active {
                PaletteResolvedAction::Disabled(
                    "Start voice first with Ctrl+K -> Toggle voice.".to_string(),
                )
            } else if state.voice_muted {
                PaletteResolvedAction::Execute("/voice unmute".to_string())
            } else {
                PaletteResolvedAction::Execute("/voice mute".to_string())
            }
        }
        PaletteActionKind::ToggleDeafen => {
            if !state.voice_active {
                PaletteResolvedAction::Disabled(
                    "Start voice first with Ctrl+K -> Toggle voice.".to_string(),
                )
            } else if state.voice_deafened {
                PaletteResolvedAction::Execute("/voice undeafen".to_string())
            } else {
                PaletteResolvedAction::Execute("/voice deafen".to_string())
            }
        }
    }
}

async fn submit_current_input(state: &SharedState) -> UiAction {
    let command = {
        let mut state_lock = state.lock().await;
        let command = state_lock.input_buffer.trim().to_string();
        if command.is_empty() {
            return UiAction::None;
        }

        if state_lock
            .command_history
            .last()
            .map(|previous| previous != &command)
            .unwrap_or(true)
        {
            state_lock.command_history.push(command.clone());
        }
        state_lock.history_index = None;
        state_lock.input_buffer.clear();
        state_lock.input_cursor = 0;
        state_lock.save_draft();
        command
    };

    UiAction::Execute(command)
}

async fn switch_scope(state: &SharedState, forward: bool) {
    let (target_scope, needs_join) = {
        let mut state_lock = state.lock().await;
        let scopes = collect_scopes(&state_lock);
        if scopes.is_empty() {
            return;
        }

        let current_index = scopes
            .iter()
            .position(|scope| scope == &state_lock.ch)
            .unwrap_or(0);
        let next_index = if forward {
            (current_index + 1) % scopes.len()
        } else if current_index == 0 {
            scopes.len() - 1
        } else {
            current_index - 1
        };
        let target_scope = scopes[next_index].clone();
        let needs_join = !target_scope.starts_with("dm:");
        state_lock.switch_scope(target_scope.clone());
        (target_scope, needs_join)
    };

    if needs_join {
        let state_lock = state.lock().await;
        if let Err(err) = state_lock.send_join(&target_scope) {
            drop(state_lock);
            let mut state_lock = state.lock().await;
            state_lock.add_activity(format!("failed to join {}: {}", target_scope, err), true);
        }
    }
}

fn navigate_history(state: &mut ClientState, older: bool) {
    if state.command_history.is_empty() {
        return;
    }

    let next_index = match (state.history_index, older) {
        (None, true) => Some(state.command_history.len() - 1),
        (Some(index), true) => Some(index.saturating_sub(1)),
        (Some(index), false) if index + 1 < state.command_history.len() => Some(index + 1),
        (Some(_), false) => None,
        (None, false) => None,
    };

    state.history_index = next_index;
    if let Some(index) = state.history_index {
        state.input_buffer = state.command_history[index].clone();
        state.input_cursor = state.input_buffer.chars().count();
    } else {
        let _ = state.load_draft();
    }
}

fn insert_char(state: &mut ClientState, ch: char) {
    state.history_index = None;
    let byte_index = char_to_byte_index(&state.input_buffer, state.input_cursor);
    state.input_buffer.insert(byte_index, ch);
    state.input_cursor += 1;
    state.save_draft();
}

fn delete_backward(state: &mut ClientState) {
    if state.input_cursor == 0 {
        return;
    }

    state.history_index = None;
    let end = char_to_byte_index(&state.input_buffer, state.input_cursor);
    let start = char_to_byte_index(&state.input_buffer, state.input_cursor - 1);
    state.input_buffer.replace_range(start..end, "");
    state.input_cursor -= 1;
    state.save_draft();
}

fn delete_forward(state: &mut ClientState) {
    let total = state.input_buffer.chars().count();
    if state.input_cursor >= total {
        return;
    }

    state.history_index = None;
    let start = char_to_byte_index(&state.input_buffer, state.input_cursor);
    let end = char_to_byte_index(&state.input_buffer, state.input_cursor + 1);
    state.input_buffer.replace_range(start..end, "");
    state.save_draft();
}

fn prefill_input(state: &mut ClientState, value: &str) {
    state.history_index = None;
    state.input_buffer = value.to_string();
    state.input_cursor = state.input_buffer.chars().count();
    state.save_draft();
}

fn apply_mention_completion(state: &mut ClientState) -> bool {
    let Some(query) = mention_query(&state.input_buffer, state.input_cursor) else {
        return false;
    };
    let Some(suggestion) = mention_suggestions(state, &query.query).into_iter().next() else {
        return false;
    };

    let mention = format!("@{} ", suggestion.value);
    replace_char_range(
        &mut state.input_buffer,
        query.at_char_index,
        query.cursor_char_index,
        &mention,
    );
    state.input_cursor = query.at_char_index + mention.chars().count();
    state.history_index = None;
    state.save_draft();
    true
}

fn char_to_byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or_else(|| value.len())
}

fn replace_char_range(value: &mut String, start_char: usize, end_char: usize, replacement: &str) {
    let start = char_to_byte_index(value, start_char);
    let end = char_to_byte_index(value, end_char);
    value.replace_range(start..end, replacement);
}

#[derive(Clone)]
struct MentionQuery {
    at_char_index: usize,
    query: String,
    cursor_char_index: usize,
}

fn mention_query(buffer: &str, cursor: usize) -> Option<MentionQuery> {
    let chars: Vec<char> = buffer.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_mention_char(chars[start - 1]) {
        start -= 1;
    }
    if start == 0 || chars[start - 1] != '@' {
        return None;
    }
    if start > 1 && is_mention_char(chars[start - 2]) {
        return None;
    }

    let query: String = chars[start..cursor].iter().collect();
    Some(MentionQuery {
        at_char_index: start - 1,
        query,
        cursor_char_index: cursor,
    })
}

fn mention_suggestions(state: &ClientState, query: &str) -> Vec<ComposerSuggestion> {
    let needle = query.trim().to_ascii_lowercase();
    let mut suggestions: Vec<ComposerSuggestion> = state
        .users
        .keys()
        .map(|user| {
            let (online, status_text) = peer_presence_for(state, user);
            ComposerSuggestion {
                value: user.clone(),
                detail: format!(
                    "{} {}",
                    if online.unwrap_or(false) {
                        "online"
                    } else {
                        "offline"
                    },
                    status_text.unwrap_or_else(|| "available".to_string())
                ),
            }
        })
        .filter(|suggestion| {
            if needle.is_empty() {
                true
            } else {
                suggestion.value.to_ascii_lowercase().starts_with(&needle)
                    || suggestion.detail.to_ascii_lowercase().contains(&needle)
            }
        })
        .collect();
    suggestions.sort_by_key(|suggestion| {
        let online = suggestion.detail.starts_with("online");
        (!online, suggestion.value.to_ascii_lowercase())
    });
    suggestions.truncate(6);
    suggestions
}

fn peer_presence_for(state: &ClientState, user: &str) -> (Option<bool>, Option<String>) {
    let online = state
        .online_users
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(user))
        .map(|_| true)
        .unwrap_or(false);

    let status = state
        .peer_statuses
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(user))
        .map(|(_, status)| {
            if status.emoji.trim().is_empty() {
                status.text.clone()
            } else {
                format!("{} {}", status.emoji.trim(), status.text)
            }
        })
        .or_else(|| {
            if online {
                Some("Online".to_string())
            } else {
                None
            }
        });

    (Some(online), status)
}

fn is_mention_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn collect_scopes(state: &ClientState) -> Vec<String> {
    let mut scopes = HashSet::new();
    scopes.insert("general".to_string());
    scopes.insert(state.ch.clone());
    for scope in state.unread_counts.keys() {
        scopes.insert(scope.clone());
    }
    for scope in state.drafts.keys() {
        scopes.insert(scope.clone());
    }
    for message in state.message_history.iter() {
        if !message.channel.is_empty() {
            scopes.insert(message.channel.clone());
        }
    }
    if let Some(session) = &state.voice_session {
        scopes.insert(session.room.clone());
    }

    let mut scopes: Vec<String> = scopes.into_iter().collect();
    scopes.sort_by_key(|scope| {
        if let Some(peer) = scope.strip_prefix("dm:") {
            (1u8, peer.to_ascii_lowercase())
        } else {
            (0u8, scope.to_ascii_lowercase())
        }
    });
    scopes
}

fn format_scope_label(scope: &str) -> String {
    if let Some(peer) = scope.strip_prefix("dm:") {
        format!("@{}", peer)
    } else {
        format!("#{}", scope)
    }
}

fn format_timestamp(ts: f64) -> String {
    if !ts.is_finite() || ts <= 0.0 {
        return String::new();
    }

    Local
        .timestamp_opt(ts.floor() as i64, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn layout_mode(width: u16) -> UiLayoutMode {
    if width < 108 {
        UiLayoutMode::Narrow
    } else {
        UiLayoutMode::Full
    }
}

fn right_panel_mode(snapshot: &UiSnapshot) -> RightPanelMode {
    if snapshot.media_preview.is_some() {
        RightPanelMode::Media
    } else if !snapshot.composer_suggestions.is_empty() {
        RightPanelMode::Suggestions
    } else {
        RightPanelMode::Now
    }
}

fn composer_hint(snapshot: &UiSnapshot) -> &'static str {
    if snapshot.current_scope.starts_with("dm:")
        && snapshot
            .dm_trust_label
            .as_deref()
            .map(|label| label != "Trusted fingerprint")
            .unwrap_or(false)
    {
        "Verify fingerprint before sending private messages"
    } else if !snapshot.composer_suggestions.is_empty() {
        "Tab completes mention"
    } else if snapshot.input_buffer.trim().starts_with("/image")
        || snapshot.input_buffer.trim().starts_with("/file")
        || snapshot.input_buffer.trim().starts_with("/video")
        || snapshot.input_buffer.trim().starts_with("/audio")
    {
        "Add a file path inside quotes"
    } else if snapshot.input_buffer.trim().starts_with("/join") {
        "Type a room name, then Enter"
    } else if snapshot.input_buffer.trim().starts_with("/dm") {
        "Type a username and message"
    } else if snapshot.input_buffer.trim().starts_with("/search") {
        "Type search words, then Enter"
    } else if snapshot.input_buffer.trim().is_empty() {
        "Type a message or press Ctrl+K for actions"
    } else {
        "Enter sends"
    }
}

fn panel_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            title.into(),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ))
}

fn muted_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn accent_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn success_style() -> Style {
    Style::default().fg(Color::Green)
}

fn warning_style() -> Style {
    Style::default().fg(Color::Yellow)
}

fn render(
    frame: &mut ratatui::Frame<'_>,
    snapshot: &UiSnapshot,
    media_preview: &mut MediaPreviewRuntime,
    palette: &PaletteState,
) {
    let area = frame.area();
    let mode = layout_mode(area.width);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, root[0], snapshot);

    match mode {
        UiLayoutMode::Full => {
            let panel_width = match right_panel_mode(snapshot) {
                RightPanelMode::Media => 36,
                RightPanelMode::Suggestions | RightPanelMode::Now => 30,
            };
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(22),
                    Constraint::Min(42),
                    Constraint::Length(panel_width),
                ])
                .split(root[1]);

            render_sidebar(frame, body[0], snapshot);
            render_timeline(frame, body[1], snapshot);
            render_right_panel(frame, body[2], snapshot, media_preview);
        }
        UiLayoutMode::Narrow => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(18), Constraint::Min(30)])
                .split(root[1]);

            render_sidebar(frame, body[0], snapshot);
            render_timeline(frame, body[1], snapshot);
        }
    }
    render_composer(frame, root[2], snapshot);
    render_footer(frame, root[3], snapshot);
    if palette.open {
        render_palette(frame, area, snapshot, palette);
    }
}

fn render_palette(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    palette: &PaletteState,
) {
    let palette_area = centered_rect(area, 72, 17);
    frame.render_widget(Clear, palette_area);

    let actions = filtered_palette_actions(&palette.query);
    let selected = if actions.is_empty() {
        0
    } else {
        palette.selected.min(actions.len() - 1)
    };

    let visible_capacity = palette_area.height.saturating_sub(7) as usize;
    let start = selected.saturating_sub(visible_capacity.saturating_sub(1));
    let end = (start + visible_capacity).min(actions.len());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("What do you want to do?", muted_style()),
            Span::raw("  "),
            Span::styled("Enter run | Esc close | Up/Down choose", muted_style()),
        ]),
        Line::from(vec![
            Span::styled("> ", accent_style()),
            Span::styled(
                if palette.query.is_empty() {
                    "type: dm, room, image, voice..."
                } else {
                    &palette.query
                },
                Style::default().fg(if palette.query.is_empty() {
                    Color::DarkGray
                } else {
                    Color::White
                }),
            ),
        ]),
        Line::default(),
    ];

    if actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No actions match. Try: room, dm, image, react, voice, help.",
            warning_style(),
        )));
    } else {
        for (index, action) in actions[start..end].iter().enumerate() {
            let absolute_index = start + index;
            let is_selected = absolute_index == selected;
            let prefix = if is_selected { "> " } else { "  " };
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, accent_style()),
                Span::styled(palette_action_label(action, snapshot), style),
            ]));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(action.detail, muted_style()),
            ]));
        }
    }

    let palette_widget = Paragraph::new(Text::from(lines))
        .block(panel_block("Actions"))
        .wrap(Wrap { trim: true });
    frame.render_widget(palette_widget, palette_area);

    let query_width = palette_area.width.saturating_sub(5) as usize;
    let cursor_column = if palette.query.is_empty() {
        0
    } else {
        palette.query.chars().count().min(query_width)
    };
    frame.set_cursor_position((
        palette_area.x + 3 + cursor_column as u16,
        palette_area.y + 2,
    ));
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width_limit = area.width.saturating_sub(4).max(1);
    let height_limit = area.height.saturating_sub(2).max(1);
    let width = max_width.min(width_limit).max(1);
    let height = max_height.min(height_limit).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn palette_action_label(action: &PaletteAction, snapshot: &UiSnapshot) -> String {
    match action.kind {
        PaletteActionKind::ToggleVoice => {
            if snapshot.voice.active {
                "Leave voice".to_string()
            } else {
                "Join voice".to_string()
            }
        }
        PaletteActionKind::ToggleMute => {
            if snapshot.voice.muted {
                "Unmute mic".to_string()
            } else {
                "Mute mic".to_string()
            }
        }
        PaletteActionKind::ToggleDeafen => {
            if snapshot.voice.deafened {
                "Undeafen".to_string()
            } else {
                "Deafen".to_string()
            }
        }
        _ => action.label.to_string(),
    }
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let title = Line::from(vec![
        Span::styled(
            "Chatify",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format_scope_label(&snapshot.current_scope),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut subtitle_spans = vec![
        Span::styled(&snapshot.subtitle, Style::default().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(
            if snapshot.voice.active {
                "Voice on"
            } else {
                "Voice off"
            },
            if snapshot.voice.active {
                success_style()
            } else {
                muted_style()
            },
        ),
        Span::raw("  "),
        Span::styled(format!("People {}", snapshot.known_users), warning_style()),
    ];
    if snapshot.total_unread > 0 {
        subtitle_spans.push(Span::raw("  "));
        subtitle_spans.push(Span::styled(
            format!("Unread {}", snapshot.total_unread),
            warning_style().add_modifier(Modifier::BOLD),
        ));
    }
    let subtitle = Line::from(subtitle_spans);

    let header = Paragraph::new(Text::from(vec![title, subtitle]))
        .block(panel_block("Session"))
        .wrap(Wrap { trim: true });
    frame.render_widget(header, area);
}

fn render_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Chats",
        warning_style().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    for scope in &snapshot.scopes {
        let mut spans = Vec::new();
        spans.push(Span::styled(
            if scope.is_current { "> " } else { "  " },
            Style::default().fg(if scope.is_current {
                Color::Cyan
            } else {
                Color::DarkGray
            }),
        ));
        spans.push(Span::styled(
            &scope.label,
            Style::default()
                .fg(if scope.is_current {
                    Color::White
                } else {
                    Color::Gray
                })
                .add_modifier(if scope.is_current {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        if let Some(online) = scope.online {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                if online { "o" } else { "-" },
                if online {
                    success_style()
                } else {
                    muted_style()
                },
            ));
        }
        if scope.has_voice {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("voice", success_style()));
        }
        if scope.unread > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("[{}]", scope.unread),
                warning_style().add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
        if let Some(status_text) = &scope.status_text {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(status_text, muted_style()),
            ]));
        }
    }

    let sidebar = Paragraph::new(Text::from(lines))
        .block(panel_block("Chats"))
        .wrap(Wrap { trim: true });
    frame.render_widget(sidebar, area);
}

fn reply_context_line(reply: &ReplyPreview) -> String {
    let target = reply
        .sender
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", reply.msg_id.chars().take(8).collect::<String>()));
    let preview = reply
        .preview
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("message unavailable");

    format!("reply to {}: {}", target, preview)
}

fn render_timeline(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    if snapshot.timeline.is_empty() {
        let empty = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                "No messages in this conversation yet.",
                Style::default().fg(Color::Gray),
            )),
            Line::default(),
            Line::from("Type below and press Enter to send."),
            Line::from("Press Ctrl+K for actions: join room, start DM, attach file, search."),
            Line::from("Use Alt+Up/Down when you want to switch rooms quickly."),
        ]))
        .block(panel_block(format!(
            "Chat {}",
            format_scope_label(&snapshot.current_scope)
        )))
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let mut lines = Vec::new();
    for (item_index, item) in snapshot.timeline.iter().enumerate() {
        if item.unread_divider_before {
            lines.push(Line::from(vec![
                Span::styled(
                    "New messages ",
                    warning_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{}]", snapshot.unread_marker_count),
                    warning_style(),
                ),
            ]));
        }

        if item.show_sender {
            if item_index > 0 {
                lines.push(Line::default());
            }
            if item.is_system {
                lines.push(Line::from(vec![Span::styled(
                    if item.when.is_empty() {
                        "system".to_string()
                    } else {
                        format!("{} system", item.when)
                    },
                    warning_style().add_modifier(Modifier::BOLD),
                )]));
            } else {
                let accent = if item.is_self {
                    Color::Green
                } else {
                    Color::Cyan
                };
                let author = if item.when.is_empty() {
                    item.sender.clone()
                } else {
                    format!("{} {}", item.when, item.sender)
                };
                lines.push(Line::from(vec![Span::styled(
                    author,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                )]));
            }
        }

        let content_style = if item.is_system {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(Color::White)
        };
        if let Some(reply) = item.reply.as_ref() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    reply_context_line(reply),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        let mut body_lines = item.body_lines.clone();
        if body_lines.is_empty() {
            body_lines.push(vec![StyledFragment {
                text: String::new(),
                fg: None,
                bg: None,
                bold: false,
                dim: false,
            }]);
        }

        for (index, body_line) in body_lines.iter().enumerate() {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(styled_line_to_spans(body_line, content_style));
            if index == 0 && !item.reaction_summary.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(item.reaction_summary.clone(), warning_style()));
            }
            lines.push(Line::from(spans));
        }
    }

    if !snapshot.typing_users.is_empty() {
        let label = if snapshot.typing_users.len() == 1 {
            format!("{} is typing...", snapshot.typing_users[0])
        } else {
            format!("{} are typing...", snapshot.typing_users.join(", "))
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();
    let scroll_top = total_lines.saturating_sub(inner_height + snapshot.scroll_offset);

    let timeline = Paragraph::new(Text::from(lines))
        .block(panel_block(format!(
            "Chat {}",
            format_scope_label(&snapshot.current_scope)
        )))
        .wrap(Wrap { trim: false })
        .scroll((scroll_top as u16, 0));
    frame.render_widget(timeline, area);
}

fn styled_line_to_spans(line: &StyledLine, base_style: Style) -> Vec<Span<'static>> {
    line.iter()
        .map(|fragment| styled_fragment_to_span(fragment, base_style))
        .collect()
}

fn styled_fragment_to_span(fragment: &StyledFragment, base_style: Style) -> Span<'static> {
    let mut style = base_style;
    if let Some(color) = fragment.fg {
        style = style.fg(rgb_to_color(color));
    }
    if let Some(color) = fragment.bg {
        style = style.bg(rgb_to_color(color));
    }
    if fragment.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if fragment.dim {
        style = style.add_modifier(Modifier::DIM);
    }

    Span::styled(fragment.text.clone(), style)
}

fn rgb_to_color(color: RgbColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

fn render_right_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    media_preview: &mut MediaPreviewRuntime,
) {
    match right_panel_mode(snapshot) {
        RightPanelMode::Media => {
            if let Some(candidate) = &snapshot.media_preview {
                render_media_preview_panel(frame, area, snapshot, candidate, media_preview);
            }
        }
        RightPanelMode::Suggestions => render_suggestions_panel(frame, area, snapshot),
        RightPanelMode::Now => render_now_panel(frame, area, snapshot),
    }
}

fn render_suggestions_panel(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let suggestion_lines: Vec<Line<'_>> = snapshot
        .composer_suggestions
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .map(|suggestion| {
            Line::from(vec![
                Span::styled(
                    suggestion.value.clone(),
                    accent_style().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(suggestion.detail.clone(), muted_style()),
            ])
        })
        .collect();

    let suggestions = Paragraph::new(Text::from(suggestion_lines))
        .block(panel_block("Suggestions"))
        .wrap(Wrap { trim: true });
    frame.render_widget(suggestions, area);
}

fn render_now_panel(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Voice: ", muted_style()),
        Span::styled(
            if snapshot.voice.active { "on" } else { "off" },
            if snapshot.voice.active {
                success_style()
            } else {
                muted_style()
            },
        ),
    ]));
    if snapshot.voice.active {
        if let Some(room) = &snapshot.voice.room {
            lines.push(Line::from(vec![
                Span::styled("Room: ", muted_style()),
                Span::raw(format_scope_label(room)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Mic: ", muted_style()),
            Span::raw(if snapshot.voice.muted {
                "muted"
            } else {
                "live"
            }),
            Span::raw("  "),
            Span::styled("Sound: ", muted_style()),
            Span::raw(if snapshot.voice.deafened { "off" } else { "on" }),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Speaking: ", muted_style()),
            Span::raw(on_off(snapshot.voice.speaking)),
        ]));
        if !snapshot.voice.members.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("In call: ", muted_style()),
                Span::raw(snapshot.voice.members.join(", ")),
            ]));
        }
    }
    lines.push(Line::from(vec![
        Span::styled("Media: ", muted_style()),
        Span::raw(on_off(snapshot.media_enabled)),
    ]));
    if let Some(label) = &snapshot.dm_trust_label {
        lines.push(Line::from(vec![
            Span::styled("Trust: ", muted_style()),
            Span::styled(label.clone(), warning_style()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("People {}", snapshot.known_users),
        warning_style().add_modifier(Modifier::BOLD),
    )));

    let people_take = area.height.saturating_sub(10).clamp(2, 6) as usize;
    for person in snapshot.online_people.iter().take(people_take) {
        lines.push(Line::from(vec![
            Span::styled(
                if person.online { "o " } else { "- " },
                if person.online {
                    success_style()
                } else {
                    muted_style()
                },
            ),
            Span::styled(
                person.user.clone(),
                Style::default().fg(if person.online {
                    Color::White
                } else {
                    Color::Gray
                }),
            ),
            Span::raw(" "),
            Span::styled(person.status_text.clone(), muted_style()),
        ]));
    }

    let recent_activity: Vec<Line<'_>> = snapshot
        .activity
        .iter()
        .rev()
        .take(3)
        .map(render_activity_line)
        .collect();
    if !recent_activity.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Recent",
            muted_style().add_modifier(Modifier::BOLD),
        )));
        lines.extend(recent_activity);
    }

    let now = Paragraph::new(Text::from(lines))
        .block(panel_block("Now"))
        .wrap(Wrap { trim: true });
    frame.render_widget(now, area);
}

fn render_media_preview_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &UiSnapshot,
    candidate: &MediaPreviewCandidate,
    media_preview: &mut MediaPreviewRuntime,
) {
    let protocol_suffix = media_preview
        .protocol_label
        .as_deref()
        .map(|label| format!(" [{}]", label))
        .unwrap_or_default();
    let block = panel_block(format!("Image Preview{}", protocol_suffix));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 4 {
        return;
    }

    if !snapshot.media_enabled {
        let disabled = Paragraph::new(Text::from(vec![
            Line::from(candidate.summary.clone()),
            Line::default(),
            Line::from(Span::styled(
                "Media rendering is disabled.",
                Style::default().fg(Color::DarkGray),
            )),
        ]))
        .wrap(Wrap { trim: true });
        frame.render_widget(disabled, inner);
        return;
    }

    if let Some(reason) = media_preview.unsupported_reason.as_deref() {
        let fallback = Paragraph::new(Text::from(vec![
            Line::from(candidate.summary.clone()),
            Line::default(),
            Line::from(Span::styled(reason, Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled(
                format!("saved: {}", candidate.path),
                Style::default().fg(Color::DarkGray),
            )),
        ]))
        .wrap(Wrap { trim: true });
        frame.render_widget(fallback, inner);
        return;
    }

    if let Some(error) = media_preview.last_error.as_deref() {
        let fallback = Paragraph::new(Text::from(vec![
            Line::from(candidate.summary.clone()),
            Line::default(),
            Line::from(Span::styled(error, Style::default().fg(Color::Red))),
            Line::from(Span::styled(
                format!("saved: {}", candidate.path),
                Style::default().fg(Color::DarkGray),
            )),
        ]))
        .wrap(Wrap { trim: true });
        frame.render_widget(fallback, inner);
        return;
    }

    if let Some(image) = media_preview.image.as_mut() {
        frame.render_stateful_widget(StatefulImage::default(), inner, image);
        return;
    }

    let loading = Paragraph::new(Text::from(vec![
        Line::from(candidate.title.clone()),
        Line::default(),
        Line::from(Span::styled(
            "Loading image preview...",
            Style::default().fg(Color::DarkGray),
        )),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(loading, inner);
}

fn render_activity_line(entry: &ActivityEntry) -> Line<'static> {
    let when = if entry.ts > 0 {
        Local
            .timestamp_opt(entry.ts as i64, 0)
            .single()
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    Line::from(vec![
        Span::styled(
            if when.is_empty() {
                String::new()
            } else {
                format!("{} ", when)
            },
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            entry.text.clone(),
            Style::default().fg(if entry.is_error {
                Color::Red
            } else {
                Color::Gray
            }),
        ),
    ])
}

fn render_composer(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let mut title = if snapshot.current_scope.starts_with("dm:") {
        format!("Message {}", format_scope_label(&snapshot.current_scope))
    } else {
        format!("Compose {}", format_scope_label(&snapshot.current_scope))
    };
    title.push_str(" - ");
    title.push_str(composer_hint(snapshot));

    let visible_width = area.width.saturating_sub(4) as usize;
    let (visible_input, cursor_column) =
        visible_input_fragment(&snapshot.input_buffer, snapshot.input_cursor, visible_width);
    let composer = Paragraph::new(visible_input)
        .block(panel_block(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(composer, area);

    let cursor_x = area.x + 1 + cursor_column as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, _snapshot: &UiSnapshot) {
    let footer = Paragraph::new(
        "Enter send | Ctrl+K actions | Tab complete | PageUp/PageDown scroll | Ctrl+C quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn visible_input_fragment(buffer: &str, cursor: usize, width: usize) -> (String, usize) {
    let chars: Vec<char> = buffer.chars().collect();
    if width == 0 {
        return (String::new(), 0);
    }

    let cursor = cursor.min(chars.len());
    let start = cursor.saturating_sub(width.saturating_sub(1));
    let end = (start + width).min(chars.len());
    let visible: String = chars[start..end].iter().collect();
    let cursor_column = cursor.saturating_sub(start).min(width.saturating_sub(1));
    (visible, cursor_column)
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn protocol_type_label(protocol_type: ProtocolType) -> &'static str {
    match protocol_type {
        ProtocolType::Halfblocks => "Halfblocks",
        ProtocolType::Sixel => "Sixel",
        ProtocolType::Kitty => "Kitty",
        ProtocolType::Iterm2 => "iTerm2",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_mention_completion, composer_hint, filtered_palette_actions, layout_mode,
        mention_query, mention_suggestions, move_palette_selection, resolve_palette_action,
        right_panel_mode, PaletteActionKind, PaletteResolvedAction, PaletteState, RightPanelMode,
        UiLayoutMode, UiSnapshot,
    };
    use crate::args::ClientConfig;
    use crate::{
        media::{MediaKind, MediaRenderStatus, TimelineMedia, TimelinePayload},
        state::{ClientState, DisplayedMessage, PeerTrust, ReplyPreview},
    };
    use tokio::sync::mpsc;

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
            chatify::config::Config::default(),
        )
    }

    #[test]
    fn mention_query_detects_active_token() {
        let query = mention_query("hello @ali", "hello @ali".chars().count())
            .expect("mention query should be detected");
        assert_eq!(query.query, "ali");
        assert_eq!(query.at_char_index, 6);
    }

    #[test]
    fn mention_completion_replaces_partial_token() {
        let mut state = make_test_state();
        state.online_users.insert("alice".to_string());
        state.set_peer_status("alice", "Pairing", "");
        state
            .users
            .insert("alice".to_string(), "pubkey-placeholder".to_string());
        state.input_buffer = "hey @ali".to_string();
        state.input_cursor = state.input_buffer.chars().count();

        assert!(apply_mention_completion(&mut state));
        assert_eq!(state.input_buffer, "hey @alice ");
    }

    #[test]
    fn mention_suggestions_prioritize_online_people() {
        let mut state = make_test_state();
        state.online_users.insert("alice".to_string());
        state.set_peer_status("alice", "Online", "");
        state.set_peer_status("bob", "Away", "");
        state
            .users
            .insert("bob".to_string(), "pubkey-bob".to_string());
        state
            .users
            .insert("alice".to_string(), "pubkey-alice".to_string());

        let suggestions = mention_suggestions(&state, "");
        assert_eq!(
            suggestions.first().map(|item| item.value.as_str()),
            Some("alice")
        );
    }

    #[test]
    fn palette_filter_finds_actions_by_keyword() {
        let actions = filtered_palette_actions("private");
        assert_eq!(
            actions.first().map(|action| action.label),
            Some("Start a DM")
        );

        let actions = filtered_palette_actions("voice mic");
        assert!(
            actions
                .iter()
                .any(|action| matches!(action.kind, PaletteActionKind::ToggleMute)),
            "voice mic should find mute toggle"
        );

        let actions = filtered_palette_actions("quote");
        assert_eq!(
            actions.first().map(|action| action.label),
            Some("Reply to latest message")
        );
    }

    #[test]
    fn ui_snapshot_carries_reply_context_for_timeline() {
        let mut state = make_test_state();
        state.message_history.push_back(DisplayedMessage {
            id: "msg-2".to_string(),
            ts: 2.0,
            channel: "general".to_string(),
            sender: "alice".to_string(),
            content: "reply body".to_string(),
            reply: Some(ReplyPreview {
                msg_id: "msg-1".to_string(),
                sender: Some("bob".to_string()),
                preview: Some("seed message".to_string()),
            }),
            payload: None,
            encrypted: true,
            edited: false,
        });

        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(
            snapshot
                .timeline
                .first()
                .and_then(|entry| entry.reply.as_ref())
                .and_then(|reply| reply.preview.as_deref()),
            Some("seed message")
        );
    }

    #[test]
    fn palette_selection_wraps_through_visible_actions() {
        let mut palette = PaletteState {
            open: true,
            query: "voice".to_string(),
            selected: 0,
        };
        let count = filtered_palette_actions(&palette.query).len();
        assert!(count > 1);

        move_palette_selection(&mut palette, false);
        assert_eq!(palette.selected, count - 1);

        move_palette_selection(&mut palette, true);
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn palette_resolves_dynamic_voice_commands() {
        let mut state = make_test_state();
        let voice_action = filtered_palette_actions("voice")
            .into_iter()
            .find(|action| matches!(action.kind, PaletteActionKind::ToggleVoice))
            .expect("voice action should exist");

        assert_eq!(
            resolve_palette_action(voice_action, &state),
            PaletteResolvedAction::Execute("/voice on".to_string())
        );

        state.voice_active = true;
        assert_eq!(
            resolve_palette_action(voice_action, &state),
            PaletteResolvedAction::Execute("/voice off".to_string())
        );
    }

    #[test]
    fn palette_resolves_audio_prefill() {
        let state = make_test_state();
        let audio_action = filtered_palette_actions("audio")
            .into_iter()
            .find(|action| action.label == "Attach audio")
            .expect("audio action should exist");

        assert_eq!(
            resolve_palette_action(audio_action, &state),
            PaletteResolvedAction::Prefill {
                value: "/audio \"\"".to_string(),
                cursor_back: 1,
            }
        );
    }

    #[test]
    fn layout_mode_hides_right_panel_on_narrow_terminals() {
        assert_eq!(layout_mode(107), UiLayoutMode::Narrow);
        assert_eq!(layout_mode(108), UiLayoutMode::Full);
    }

    #[test]
    fn right_panel_mode_prioritizes_media_then_suggestions() {
        let mut state = make_test_state();
        state.input_buffer = "hey @a".to_string();
        state.input_cursor = state.input_buffer.chars().count();
        state
            .users
            .insert("alice".to_string(), "pubkey-alice".to_string());
        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(right_panel_mode(&snapshot), RightPanelMode::Suggestions);

        state.message_history.push_back(DisplayedMessage {
            id: "media:general:img-1".to_string(),
            ts: 1.0,
            channel: "general".to_string(),
            sender: "alice".to_string(),
            content: "[image] preview.png".to_string(),
            reply: None,
            payload: Some(TimelinePayload::Media(TimelineMedia {
                file_id: "img-1".to_string(),
                filename: "preview.png".to_string(),
                media_kind: MediaKind::Image,
                mime: Some("image/png".to_string()),
                size: 64,
                duration_ms: None,
                received_bytes: 64,
                local_path: Some("/tmp/preview.png".to_string()),
                preview: Vec::new(),
                render_status: MediaRenderStatus::Complete,
            })),
            encrypted: false,
            edited: false,
        });
        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(right_panel_mode(&snapshot), RightPanelMode::Media);
    }

    #[test]
    fn composer_hint_is_contextual_without_blocking_typing() {
        let mut state = make_test_state();
        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(
            composer_hint(&snapshot),
            "Type a message or press Ctrl+K for actions"
        );

        state.input_buffer = "/image \"\"".to_string();
        state.input_cursor = state.input_buffer.chars().count().saturating_sub(1);
        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(composer_hint(&snapshot), "Add a file path inside quotes");

        state.ch = "dm:alice".to_string();
        state.trust_store.peers.insert(
            "alice".to_string(),
            PeerTrust {
                fingerprint: "abc".to_string(),
                trusted_at: 1,
                verified: false,
            },
        );
        let snapshot = UiSnapshot::from_state(&state);
        assert_eq!(
            composer_hint(&snapshot),
            "Verify fingerprint before sending private messages"
        );
    }

    #[test]
    fn ui_snapshot_prefers_latest_image_with_local_path_for_preview_panel() {
        let mut state = make_test_state();
        state.message_history.push_back(DisplayedMessage {
            id: "media:general:img-1".to_string(),
            ts: 1.0,
            channel: "general".to_string(),
            sender: "alice".to_string(),
            content: "[image] preview.png".to_string(),
            reply: None,
            payload: Some(TimelinePayload::Media(TimelineMedia {
                file_id: "img-1".to_string(),
                filename: "preview.png".to_string(),
                media_kind: MediaKind::Image,
                mime: Some("image/png".to_string()),
                size: 64,
                duration_ms: None,
                received_bytes: 64,
                local_path: Some("C:/tmp/preview.png".to_string()),
                preview: Vec::new(),
                render_status: MediaRenderStatus::Complete,
            })),
            encrypted: false,
            edited: false,
        });

        let snapshot = UiSnapshot::from_state(&state);
        let preview = snapshot
            .media_preview
            .expect("latest image should be promoted to the preview panel");
        assert_eq!(preview.title, "preview.png");
        assert_eq!(preview.path, "C:/tmp/preview.png");
    }
}
