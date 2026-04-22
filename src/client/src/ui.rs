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
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

use crate::handlers;
use crate::media::{render_message_lines, RgbColor, StyledFragment, StyledLine};
use crate::state::{ActivityEntry, ClientState, SharedState};
use clifford::error::{ChatifyError, ChatifyResult};

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
struct UiSnapshot {
    title: String,
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
    known_users: usize,
    dm_trust_label: Option<String>,
    voice: VoiceSnapshot,
}

impl UiSnapshot {
    fn from_state(state: &ClientState) -> Self {
        let current_scope = state.ch.clone();
        let title = if current_scope.starts_with("dm:") {
            format!("Direct Message {}", format_scope_label(&current_scope))
        } else {
            format!("Room {}", format_scope_label(&current_scope))
        };

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
        for message in visible_messages {
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

        let cutoff = (clifford::now() as u64).saturating_sub(30);
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

        let composer_suggestions = mention_query(&state.input_buffer, state.input_cursor)
            .map(|query| mention_suggestions(state, &query.query))
            .unwrap_or_default();

        Self {
            title,
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
            known_users: state.users.len(),
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
    let input_thread = InputThread::start();

    {
        let mut state_lock = state.lock().await;
        state_lock.add_activity(
            "Chat UI ready. Enter sends, Tab completes mentions, Alt+Up/Down switches rooms, Alt+R reacts, Alt+I attaches an image, Alt+V toggles voice.",
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

        terminal
            .terminal
            .draw(|frame| render(frame, &snapshot))
            .map_err(ChatifyError::from)?;

        let mut actions = Vec::new();
        while let Ok(event) = input_thread.try_recv() {
            let action = handle_event(&state, event).await;
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

async fn handle_event(state: &SharedState, event: Event) -> UiAction {
    let Event::Key(key) = event else {
        return UiAction::None;
    };

    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return UiAction::None;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            UiAction::Quit
        }
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

fn render(frame: &mut ratatui::Frame<'_>, snapshot: &UiSnapshot) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, root[0], snapshot);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Min(40),
            Constraint::Length(30),
        ])
        .split(root[1]);

    render_sidebar(frame, body[0], snapshot);
    render_timeline(frame, body[1], snapshot);
    render_right_panel(frame, body[2], snapshot);
    render_composer(frame, root[2], snapshot);
    render_footer(frame, root[3], snapshot);
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
            &snapshot.title,
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    let subtitle = Line::from(vec![
        Span::styled(&snapshot.subtitle, Style::default().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(
            if snapshot.voice.active {
                "VOICE ON"
            } else {
                "VOICE OFF"
            },
            Style::default().fg(if snapshot.voice.active {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} user(s)", snapshot.known_users),
            Style::default().fg(Color::Yellow),
        ),
    ]);

    let header = Paragraph::new(Text::from(vec![title, subtitle]))
        .block(Block::default().borders(Borders::ALL).title("Session"))
        .wrap(Wrap { trim: true });
    frame.render_widget(header, area);
}

fn render_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Conversations",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
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
                Style::default().fg(if online {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
            ));
        }
        if scope.has_voice {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("[VC]", Style::default().fg(Color::Green)));
        }
        if scope.unread > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("[{}]", scope.unread),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
        if let Some(status_text) = &scope.status_text {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(status_text, Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    let sidebar = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title("Rooms"))
        .wrap(Wrap { trim: true });
    frame.render_widget(sidebar, area);
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
            Line::from("Use Alt+Up/Down to move between rooms."),
        ]))
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Timeline {}",
            format_scope_label(&snapshot.current_scope)
        )))
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let mut lines = Vec::new();
    for item in &snapshot.timeline {
        if item.unread_divider_before {
            lines.push(Line::from(vec![
                Span::styled(
                    "--- new messages ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{}]", snapshot.unread_marker_count),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }

        if item.show_sender {
            if item.is_system {
                lines.push(Line::from(vec![Span::styled(
                    if item.when.is_empty() {
                        "system".to_string()
                    } else {
                        format!("{} system", item.when)
                    },
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
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
                spans.push(Span::styled(
                    item.reaction_summary.clone(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::default());
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
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Timeline {}",
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

fn render_right_panel(frame: &mut ratatui::Frame<'_>, area: Rect, snapshot: &UiSnapshot) {
    let columns = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(6),
        ])
        .split(area);

    let mut status_lines = vec![
        Line::from(vec![
            Span::styled("Current ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_scope_label(&snapshot.current_scope),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Media   ", Style::default().fg(Color::DarkGray)),
            Span::raw(if snapshot.media_enabled {
                "enabled"
            } else {
                "disabled"
            }),
        ]),
    ];

    if let Some(label) = &snapshot.dm_trust_label {
        status_lines.push(Line::from(vec![
            Span::styled("Trust   ", Style::default().fg(Color::DarkGray)),
            Span::styled(label, Style::default().fg(Color::Yellow)),
        ]));
    }

    let voice_label = if let Some(room) = &snapshot.voice.room {
        format!(
            "{} in {}",
            if snapshot.voice.active {
                "connected"
            } else {
                "idle"
            },
            format_scope_label(room)
        )
    } else {
        "idle".to_string()
    };
    status_lines.push(Line::from(vec![
        Span::styled("Voice   ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            voice_label,
            Style::default().fg(if snapshot.voice.active {
                Color::Green
            } else {
                Color::Gray
            }),
        ),
    ]));
    status_lines.push(Line::from(vec![
        Span::styled("State   ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!(
            "mute={} deafen={} speaking={}",
            on_off(snapshot.voice.muted),
            on_off(snapshot.voice.deafened),
            on_off(snapshot.voice.speaking)
        )),
    ]));
    status_lines.push(Line::from(vec![
        Span::styled("Members ", Style::default().fg(Color::DarkGray)),
        Span::raw(if snapshot.voice.members.is_empty() {
            "none".to_string()
        } else {
            snapshot.voice.members.join(", ")
        }),
    ]));

    let status = Paragraph::new(Text::from(status_lines))
        .block(Block::default().borders(Borders::ALL).title("Room Info"))
        .wrap(Wrap { trim: true });
    frame.render_widget(status, columns[0]);

    if !snapshot.composer_suggestions.is_empty() {
        let suggestion_lines: Vec<Line<'_>> = snapshot
            .composer_suggestions
            .iter()
            .take(columns[1].height.saturating_sub(2) as usize)
            .map(|suggestion| {
                Line::from(vec![
                    Span::styled(
                        suggestion.value.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        suggestion.detail.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect();

        let suggestions = Paragraph::new(Text::from(suggestion_lines))
            .block(Block::default().borders(Borders::ALL).title("Suggestions"))
            .wrap(Wrap { trim: true });
        frame.render_widget(suggestions, columns[1]);
    } else {
        let people_lines: Vec<Line<'_>> = snapshot
            .online_people
            .iter()
            .take(columns[1].height.saturating_sub(2) as usize)
            .map(|person| {
                Line::from(vec![
                    Span::styled(
                        if person.online { "o " } else { "- " },
                        Style::default().fg(if person.online {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
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
                    Span::styled(
                        person.status_text.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect();

        let people = Paragraph::new(Text::from(people_lines))
            .block(Block::default().borders(Borders::ALL).title("People"))
            .wrap(Wrap { trim: true });
        frame.render_widget(people, columns[1]);
    }

    let activity_lines: Vec<Line<'_>> = snapshot
        .activity
        .iter()
        .rev()
        .take(columns[2].height.saturating_sub(2) as usize)
        .map(render_activity_line)
        .collect();

    let activity = Paragraph::new(Text::from(activity_lines))
        .block(Block::default().borders(Borders::ALL).title("Activity"))
        .wrap(Wrap { trim: true });
    frame.render_widget(activity, columns[2]);
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
    if !snapshot.composer_suggestions.is_empty() {
        title.push_str(" [Tab complete]");
    }

    let visible_width = area.width.saturating_sub(4) as usize;
    let (visible_input, cursor_column) =
        visible_input_fragment(&snapshot.input_buffer, snapshot.input_cursor, visible_width);
    let composer = Paragraph::new(visible_input)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(composer, area);

    let cursor_x = area.x + 1 + cursor_column as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, _snapshot: &UiSnapshot) {
    let footer = Paragraph::new(
        "Enter send | Tab mention | Alt+Up/Down rooms | PageUp/PageDown scroll | Alt+R react | Alt+I image | Alt+V voice | Alt+M mute | Alt+D deafen | Ctrl+C quit",
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

#[cfg(test)]
mod tests {
    use super::{apply_mention_completion, mention_query, mention_suggestions};
    use crate::args::ClientConfig;
    use crate::state::ClientState;
    use tokio::sync::mpsc;

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
}
