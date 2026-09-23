use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::engine::content::{
    CodeLanguage, ContentSource, MistakeMode, PracticeKind, Snippet, TypingContent, WordLanguage,
    WordLength, WordListSize,
};
use crate::engine::corpus::{self, LoadRequest, LoadResult};
use crate::engine::session::{KeystrokeEvent, ProgressEvent, Session, SessionMode};
use crate::multiplayer::client::{ConnectionStatus, NetworkClient, NetworkEvent};
use crate::multiplayer::protocol::{ClientMessage, RoomPhase, RoomSnapshot, ServerMessage, now_ms};
use crate::stats::history::StatsStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Menu,
    Loading,
    RoomEntry,
    Lobby,
    Countdown,
    Typing,
    Results,
    Analysis,
    Stats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaretStyle {
    BlinkingBar,
    SteadyBar,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
}

impl CaretStyle {
    pub const ALL: [Self; 6] = [
        Self::BlinkingBar,
        Self::SteadyBar,
        Self::BlinkingBlock,
        Self::SteadyBlock,
        Self::BlinkingUnderline,
        Self::SteadyUnderline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::BlinkingBar => "blinking bar",
            Self::SteadyBar => "steady bar",
            Self::BlinkingBlock => "blinking block",
            Self::SteadyBlock => "steady block",
            Self::BlinkingUnderline => "blinking underline",
            Self::SteadyUnderline => "steady underline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub practice: PracticeKind,
    pub word_language: WordLanguage,
    pub word_list: WordListSize,
    pub word_length: WordLength,
    pub word_mode: SessionMode,
    pub code_language: CodeLanguage,
    pub mistake_mode: MistakeMode,
    pub lowercase_words: bool,
    pub code_autopairs: bool,
    pub caret_style: CaretStyle,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            practice: PracticeKind::Words,
            word_language: WordLanguage::English,
            word_list: WordListSize::Top1k,
            word_length: WordLength::Any,
            word_mode: SessionMode::Timed(30),
            code_language: CodeLanguage::Rust,
            mistake_mode: MistakeMode::Strict,
            lowercase_words: true,
            code_autopairs: false,
            caret_style: CaretStyle::BlinkingBar,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionAnalysis {
    pub text: String,
    pub keystrokes: Vec<KeystrokeEvent>,
    pub progress: Vec<ProgressEvent>,
    pub correct_positions: Vec<bool>,
    pub final_cursor: usize,
}

#[derive(Debug, Clone)]
pub struct SessionResult {
    pub content: String,
    pub language: String,
    pub test_id: String,
    pub new_personal_best: bool,
    pub wpm: f64,
    pub cpm: f64,
    pub accuracy: f64,
    pub duration_secs: f64,
    pub total_keystrokes: u32,
    pub correct_keystrokes: u32,
    pub mistakes: u32,
    pub corrections: u32,
    pub source: Option<String>,
    pub snippet_id: Option<String>,
    pub repository: Option<String>,
    pub confusion: HashMap<String, u32>,
    pub character_stats: HashMap<char, (u64, u32, u32)>,
    pub analysis: SessionAnalysis,
}

pub struct MultiplayerState {
    pub room: Option<RoomSnapshot>,
    pub player_id: Option<String>,
    pub connection: ConnectionStatus,
    pub server_url: String,
    client: NetworkClient,
    clock_offset_ms: i64,
    last_progress_sent: Instant,
}

pub struct App {
    pub state: AppState,
    pub settings: Settings,
    pub menu_selection: usize,
    pub room_entry_selection: usize,
    pub analysis_line: usize,
    pub room_code_input: String,
    pub player_name: String,
    pub session: Option<Session>,
    pub last_result: Option<SessionResult>,
    pub stats_store: StatsStore,
    pub status: Option<String>,
    pub show_typed: bool,
    pub multiplayer: Option<MultiplayerState>,
    loader: Option<Receiver<Result<LoadResult, String>>>,
    last_content: Option<TypingContent>,
    hosting_room: bool,
}

impl App {
    pub fn new() -> Self {
        let (stats_store, status) = StatsStore::load();
        Self::with_stats(stats_store, status)
    }

    fn with_stats(stats_store: StatsStore, status: Option<String>) -> Self {
        let settings = stats_store.settings.clone().unwrap_or_default();
        Self {
            state: AppState::Menu,
            settings,
            menu_selection: 0,
            room_entry_selection: 1,
            analysis_line: 0,
            room_code_input: String::new(),
            player_name: default_player_name(),
            session: None,
            last_result: None,
            stats_store,
            status,
            show_typed: false,
            multiplayer: None,
            loader: None,
            last_content: None,
            hosting_room: false,
        }
    }

    pub fn menu_rows(&self) -> usize {
        match self.settings.practice {
            PracticeKind::Words => 9,
            PracticeKind::Code => 6,
        }
    }

    pub fn menu_next(&mut self) {
        self.menu_selection = (self.menu_selection + 1) % self.menu_rows();
    }

    pub fn menu_prev(&mut self) {
        self.menu_selection = self
            .menu_selection
            .checked_sub(1)
            .unwrap_or(self.menu_rows() - 1);
    }

    pub fn menu_change(&mut self, direction: i8) {
        let forward = direction > 0;
        match (self.settings.practice, self.menu_selection) {
            (_, 0) => {
                self.settings.practice = match self.settings.practice {
                    PracticeKind::Words => PracticeKind::Code,
                    PracticeKind::Code => PracticeKind::Words,
                };
                self.menu_selection = self.menu_selection.min(self.menu_rows() - 1);
            }
            (PracticeKind::Words, 1) => {
                self.settings.word_language = match self.settings.word_language {
                    WordLanguage::English => WordLanguage::Russian,
                    WordLanguage::Russian => WordLanguage::English,
                }
            }
            (PracticeKind::Words, 2) => {
                self.settings.word_list = cycle(
                    self.settings.word_list,
                    &[
                        WordListSize::Top200,
                        WordListSize::Top1k,
                        WordListSize::Top5k,
                        WordListSize::Top10k,
                    ],
                    forward,
                )
            }
            (PracticeKind::Words, 3) => {
                self.settings.word_length = cycle(
                    self.settings.word_length,
                    &[
                        WordLength::Any,
                        WordLength::Short,
                        WordLength::Medium,
                        WordLength::Long,
                    ],
                    forward,
                )
            }
            (PracticeKind::Words, 4) => {
                self.settings.word_mode = cycle(
                    self.settings.word_mode,
                    &[
                        SessionMode::Timed(15),
                        SessionMode::Timed(30),
                        SessionMode::Timed(60),
                        SessionMode::Timed(120),
                        SessionMode::WordCount(10),
                        SessionMode::WordCount(25),
                        SessionMode::WordCount(50),
                        SessionMode::WordCount(100),
                    ],
                    forward,
                )
            }
            (PracticeKind::Words, 5) | (PracticeKind::Code, 2) => {
                self.settings.mistake_mode = match self.settings.mistake_mode {
                    MistakeMode::Strict => MistakeMode::Free,
                    MistakeMode::Free => MistakeMode::Strict,
                }
            }
            (PracticeKind::Words, 6) => {
                self.settings.lowercase_words = !self.settings.lowercase_words;
            }
            (PracticeKind::Words, 7) | (PracticeKind::Code, 4) => {
                self.settings.caret_style =
                    cycle(self.settings.caret_style, &CaretStyle::ALL, forward)
            }
            (PracticeKind::Code, 1) => {
                self.settings.code_language =
                    cycle(self.settings.code_language, &CodeLanguage::ALL, forward)
            }
            (PracticeKind::Code, 3) => {
                self.settings.code_autopairs = !self.settings.code_autopairs;
            }
            _ => {}
        }
        self.stats_store.settings = Some(self.settings.clone());
        if let Err(error) = self.stats_store.save() {
            self.status = Some(format!("Could not save settings: {error}"));
        }
    }

    pub fn menu_activate(&mut self) {
        if self.menu_selection + 1 == self.menu_rows() {
            self.multiplayer = None;
            self.start_session(false);
        } else {
            self.menu_change(1);
        }
    }

    pub fn open_multiplayer(&mut self, code: Option<String>) {
        self.status = None;
        self.room_code_input = code
            .unwrap_or_default()
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .take(6)
            .collect::<String>()
            .to_ascii_uppercase();
        self.room_entry_selection = if self.room_code_input.is_empty() {
            1
        } else {
            2
        };
        self.state = AppState::RoomEntry;
    }

    pub fn room_entry_next(&mut self) {
        self.room_entry_selection = (self.room_entry_selection + 1) % 4;
    }

    pub fn room_entry_prev(&mut self) {
        self.room_entry_selection = self.room_entry_selection.checked_sub(1).unwrap_or(3);
    }

    pub fn room_entry_type(&mut self, character: char) {
        match self.room_entry_selection {
            0 if !character.is_control() && self.player_name.chars().count() < 16 => {
                self.player_name.push(character);
            }
            1 if character.is_ascii_alphanumeric() && self.room_code_input.len() < 6 => {
                self.room_code_input.push(character.to_ascii_uppercase());
            }
            _ => {}
        }
    }

    pub fn room_entry_backspace(&mut self) {
        match self.room_entry_selection {
            0 => {
                self.player_name.pop();
            }
            1 => {
                self.room_code_input.pop();
            }
            _ => {}
        }
    }

    pub fn room_entry_activate(&mut self) {
        match self.room_entry_selection {
            0 => self.room_entry_selection = 1,
            1 | 2 if !self.room_code_input.is_empty() => self.join_room(),
            3 => {
                self.hosting_room = true;
                self.start_session(false);
            }
            _ => self.status = Some("Enter a room code".into()),
        }
    }

    fn join_room(&mut self) {
        let message = ClientMessage::Join {
            code: self.room_code_input.clone(),
            name: self.player_name.clone(),
            resume_token: None,
        };
        self.connect_multiplayer(message);
        self.state = AppState::Lobby;
    }

    fn connect_multiplayer(&mut self, initial: ClientMessage) {
        let server_url =
            std::env::var("TYPEFORGE_SERVER").unwrap_or_else(|_| "ws://127.0.0.1:8787".into());
        self.status = None;
        self.multiplayer = Some(MultiplayerState {
            room: None,
            player_id: None,
            connection: ConnectionStatus::Connecting,
            server_url: server_url.clone(),
            client: NetworkClient::connect(server_url, initial),
            clock_offset_ms: 0,
            last_progress_sent: Instant::now(),
        });
    }

    pub fn leave_room(&mut self) {
        self.multiplayer = None;
        self.session = None;
        self.state = AppState::Menu;
    }

    pub fn toggle_ready(&mut self) {
        let Some(multiplayer) = &self.multiplayer else {
            return;
        };
        let ready = multiplayer
            .room
            .as_ref()
            .zip(multiplayer.player_id.as_ref())
            .and_then(|(room, player_id)| {
                room.players.iter().find(|player| &player.id == player_id)
            })
            .is_some_and(|player| !player.ready);
        multiplayer.client.send(ClientMessage::SetReady { ready });
    }

    pub fn start_race(&self) {
        let Some(multiplayer) = &self.multiplayer else {
            return;
        };
        let is_host = multiplayer
            .room
            .as_ref()
            .zip(multiplayer.player_id.as_ref())
            .is_some_and(|(room, player_id)| &room.host_id == player_id);
        if is_host {
            multiplayer.client.send(ClientMessage::Start);
        }
    }

    pub fn start_session(&mut self, refresh: bool) {
        self.status = None;
        let request = match self.settings.practice {
            PracticeKind::Words => {
                LoadRequest::Words(self.settings.word_language, self.settings.word_list)
            }
            PracticeKind::Code if refresh => LoadRequest::Refresh(self.settings.code_language),
            PracticeKind::Code => LoadRequest::Code(self.settings.code_language),
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = corpus::load(request).map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
        });
        self.loader = Some(receiver);
        self.state = AppState::Loading;
    }

    pub fn poll_loader(&mut self) {
        let Some(receiver) = &self.loader else {
            return;
        };
        let Ok(result) = receiver.try_recv() else {
            return;
        };
        self.loader = None;
        match result {
            Ok(LoadResult::Words(words)) => self.begin_words(words),
            Ok(LoadResult::Code(snippets)) => self.begin_code(snippets),
            Err(error) => {
                self.status = Some(error);
                self.state = AppState::Menu;
            }
        }
    }

    pub fn poll_multiplayer(&mut self) {
        let mut events = Vec::new();
        if let Some(multiplayer) = &mut self.multiplayer {
            while let Some(event) = multiplayer.client.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                NetworkEvent::Status(status) => {
                    if let Some(multiplayer) = &mut self.multiplayer {
                        multiplayer.connection = status;
                    }
                }
                NetworkEvent::Message(message) => match *message {
                    ServerMessage::Welcome {
                        player_id,
                        resume_token: _,
                        room,
                    } => {
                        if let Some(multiplayer) = &mut self.multiplayer {
                            multiplayer.player_id = Some(player_id);
                            let nonce = now_ms();
                            multiplayer.client.send(ClientMessage::Ping { nonce });
                        }
                        self.apply_room(room);
                    }
                    ServerMessage::Snapshot { room } => self.apply_room(room),
                    ServerMessage::Pong {
                        nonce,
                        server_time_ms,
                    } => {
                        if let Some(multiplayer) = &mut self.multiplayer {
                            let midpoint = nonce.saturating_add(now_ms().saturating_sub(nonce) / 2);
                            multiplayer.clock_offset_ms = server_time_ms as i64 - midpoint as i64;
                        }
                    }
                    ServerMessage::Error { message, fatal } => {
                        self.status = Some(message);
                        if fatal {
                            self.multiplayer = None;
                            self.state = AppState::RoomEntry;
                        }
                    }
                },
            }
        }
        self.advance_countdown();
        let should_send = self.state == AppState::Typing
            && self.multiplayer.as_ref().is_some_and(|multiplayer| {
                multiplayer.last_progress_sent.elapsed() >= Duration::from_millis(100)
            });
        if should_send {
            self.send_progress(false);
        }
    }

    fn apply_room(&mut self, room: RoomSnapshot) {
        self.status = None;
        let phase = room.phase;
        if let Some(multiplayer) = &mut self.multiplayer {
            multiplayer.room = Some(room.clone());
        }
        if matches!(self.state, AppState::Results | AppState::Analysis)
            && self.multiplayer.is_some()
        {
            return;
        }
        match phase {
            RoomPhase::Lobby => self.state = AppState::Lobby,
            RoomPhase::Countdown { starts_at_ms } | RoomPhase::Racing { starts_at_ms } => {
                if self.session.is_none() {
                    self.last_content = Some(room.content.clone());
                    self.session = Some(Session::new(
                        room.content,
                        room.mode,
                        room.mistake_mode,
                        room.autopairs,
                    ));
                }
                if self.multiplayer_now_ms() >= starts_at_ms {
                    self.start_multiplayer_session(starts_at_ms);
                } else {
                    self.state = AppState::Countdown;
                }
            }
            RoomPhase::Finished { .. } if self.last_result.is_some() => {
                self.state = AppState::Results;
            }
            RoomPhase::Finished { starts_at_ms } => {
                if self.session.is_none() {
                    self.last_content = Some(room.content.clone());
                    self.session = Some(Session::new(
                        room.content,
                        room.mode,
                        room.mistake_mode,
                        room.autopairs,
                    ));
                    self.start_multiplayer_session(starts_at_ms);
                }
            }
        }
    }

    fn multiplayer_now_ms(&self) -> u64 {
        let offset = self
            .multiplayer
            .as_ref()
            .map(|multiplayer| multiplayer.clock_offset_ms)
            .unwrap_or(0);
        now_ms().saturating_add_signed(offset)
    }

    fn advance_countdown(&mut self) {
        if self.state != AppState::Countdown {
            return;
        }
        let starts_at_ms = self
            .multiplayer
            .as_ref()
            .and_then(|multiplayer| multiplayer.room.as_ref())
            .and_then(|room| room.phase.starts_at_ms());
        if let Some(starts_at_ms) = starts_at_ms
            && self.multiplayer_now_ms() >= starts_at_ms
        {
            self.start_multiplayer_session(starts_at_ms);
        }
    }

    fn start_multiplayer_session(&mut self, starts_at_ms: u64) {
        let elapsed = self.multiplayer_now_ms().saturating_sub(starts_at_ms);
        if let Some(session) = &mut self.session
            && session.start_time.is_none()
        {
            session.start_with_elapsed(Duration::from_millis(elapsed));
        }
        self.state = AppState::Typing;
    }

    fn send_progress(&mut self, finished: bool) {
        let Some(session) = &self.session else {
            return;
        };
        let message = ClientMessage::Progress {
            cursor: session.cursor,
            wpm: session.wpm(),
            accuracy: session.accuracy(),
            mistakes: session.mistakes,
            finished,
            elapsed_ms: (session.elapsed_secs() * 1_000.0) as u64,
        };
        if let Some(multiplayer) = &mut self.multiplayer {
            multiplayer.client.send(message);
            multiplayer.last_progress_sent = Instant::now();
        }
    }

    pub fn countdown_remaining_ms(&self) -> Option<u64> {
        self.multiplayer
            .as_ref()
            .and_then(|multiplayer| multiplayer.room.as_ref())
            .and_then(|room| room.phase.starts_at_ms())
            .map(|starts_at_ms| starts_at_ms.saturating_sub(self.multiplayer_now_ms()))
    }

    pub fn cancel_loading(&mut self) {
        self.loader = None;
        self.hosting_room = false;
        self.state = AppState::Menu;
    }

    fn begin_words(&mut self, words: Vec<String>) {
        let candidates: Vec<_> = words
            .into_iter()
            .filter(|word| self.settings.word_length.accepts(word))
            .collect();
        if candidates.is_empty() {
            self.status = Some("No words match selected length".into());
            self.state = AppState::Menu;
            return;
        }
        let count = word_sample_size(self.settings.word_mode);
        let mut rng = rand::thread_rng();
        let selected: Vec<_> = (0..count)
            .filter_map(|_| candidates.choose(&mut rng).cloned())
            .map(|word| {
                if self.settings.lowercase_words {
                    word.to_lowercase()
                } else {
                    word
                }
            })
            .collect();
        let content = TypingContent {
            text: selected.join(" "),
            source: ContentSource::Words {
                language: self.settings.word_language,
                size: self.settings.word_list,
            },
        };
        self.begin(content, self.settings.word_mode);
    }

    fn begin_code(&mut self, snippets: Vec<Snippet>) {
        let mut candidates: Vec<_> = snippets
            .iter()
            .filter(|snippet| !self.stats_store.seen_snippets.contains(&snippet.id))
            .filter(|snippet| {
                !self
                    .stats_store
                    .recent_repositories
                    .contains(&snippet.repository)
            })
            .cloned()
            .collect();
        if candidates.is_empty() {
            candidates = snippets
                .iter()
                .filter(|snippet| !self.stats_store.seen_snippets.contains(&snippet.id))
                .filter(|snippet| {
                    self.stats_store
                        .last_repository
                        .as_ref()
                        .is_none_or(|repository| repository != &snippet.repository)
                })
                .cloned()
                .collect();
        }
        if candidates.is_empty() {
            for snippet in &snippets {
                self.stats_store.seen_snippets.remove(&snippet.id);
            }
            candidates = snippets;
        }
        let mut repositories: Vec<_> = candidates
            .iter()
            .map(|snippet| snippet.repository.as_str())
            .collect();
        repositories.sort_unstable();
        repositories.dedup();
        let mut rng = rand::thread_rng();
        let repository = repositories.choose(&mut rng).copied();
        let Some(snippet) = candidates
            .iter()
            .filter(|snippet| Some(snippet.repository.as_str()) == repository)
            .collect::<Vec<_>>()
            .choose(&mut rng)
            .map(|snippet| (*snippet).clone())
        else {
            self.status = Some("Snippet cache empty; press r to fetch another repository".into());
            self.state = AppState::Menu;
            return;
        };
        self.begin(
            TypingContent {
                text: snippet.text.clone(),
                source: ContentSource::Code(snippet),
            },
            SessionMode::Snippet,
        );
    }

    fn begin(&mut self, content: TypingContent, mode: SessionMode) {
        let autopairs =
            self.settings.code_autopairs && matches!(&content.source, ContentSource::Code(_));
        self.last_content = Some(content.clone());
        if self.hosting_room {
            self.hosting_room = false;
            self.connect_multiplayer(ClientMessage::Create {
                name: self.player_name.clone(),
                content,
                mode,
                mistake_mode: self.settings.mistake_mode,
                autopairs,
            });
            self.state = AppState::Lobby;
            return;
        }
        self.session = Some(Session::new(
            content,
            mode,
            self.settings.mistake_mode,
            autopairs,
        ));
        self.state = AppState::Typing;
    }

    pub fn retry(&mut self) {
        if let Some(content) = self.last_content.clone() {
            let mode = match content.source {
                ContentSource::Words { .. } => self.settings.word_mode,
                ContentSource::Code(_) => SessionMode::Snippet,
            };
            self.begin(content, mode);
        }
    }

    pub fn type_char(&mut self, character: char) {
        if let Some(session) = &mut self.session {
            session.type_char(character);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(session) = &mut self.session {
            session.backspace();
        }
    }

    pub fn delete_word(&mut self) {
        if let Some(session) = &mut self.session {
            session.delete_word();
        }
    }

    pub fn delete_line(&mut self) {
        if let Some(session) = &mut self.session {
            session.delete_line();
        }
    }

    pub fn cycle_session(&mut self) {
        if let Some(session) = self.session.take()
            && let ContentSource::Code(snippet) = session.content.source
        {
            self.stats_store.seen_snippets.insert(snippet.id);
            self.stats_store.remember_repository(&snippet.repository);
            let _ = self.stats_store.save();
        }
        self.start_session(false);
    }

    pub fn session_complete(&self) -> bool {
        self.session.as_ref().is_some_and(Session::is_complete)
    }

    pub fn session_timed_out(&self) -> bool {
        self.session.as_ref().is_some_and(Session::is_timed_out)
    }

    pub fn open_analysis(&mut self) {
        if self.last_result.is_some() {
            self.analysis_line = 0;
            self.state = AppState::Analysis;
        }
    }

    pub fn close_analysis(&mut self) {
        self.state = AppState::Results;
    }

    pub fn analysis_line_next(&mut self) {
        let lines = self
            .last_result
            .as_ref()
            .map(|result| result.analysis.text.split('\n').count())
            .unwrap_or(0);
        self.analysis_line = (self.analysis_line + 1).min(lines.saturating_sub(1));
    }

    pub fn analysis_line_prev(&mut self) {
        self.analysis_line = self.analysis_line.saturating_sub(1);
    }

    pub fn finish_session(&mut self) {
        self.send_progress(true);
        let Some(session) = self.session.take() else {
            return;
        };
        let (content, language, test_id, source, snippet_id, repository) =
            match &session.content.source {
                ContentSource::Words { language, size } => (
                    session.mode.label(),
                    format!("{} {}", language.label(), size.label()),
                    format!(
                        "words:{}:{}:{}:{}:{}:{}",
                        language.slug(),
                        size.label(),
                        self.settings.word_length.label(),
                        session.mode.label(),
                        session.mistake_mode.label(),
                        self.settings.lowercase_words
                    ),
                    None,
                    None,
                    None,
                ),
                ContentSource::Code(snippet) => (
                    snippet.kind.clone(),
                    snippet.language.label().into(),
                    format!(
                        "code:{}:{}{}",
                        snippet.language.slug(),
                        session.mistake_mode.label(),
                        if self.settings.code_autopairs {
                            ":autopairs"
                        } else {
                            ""
                        }
                    ),
                    Some(format!(
                        "{} · {} · {}\n{}",
                        snippet.repository, snippet.path, snippet.license, snippet.url
                    )),
                    Some(snippet.id.clone()),
                    Some(snippet.repository.clone()),
                ),
            };
        let analysis = SessionAnalysis {
            text: session.content.text.clone(),
            keystrokes: session.keystrokes.clone(),
            progress: session.progress.clone(),
            correct_positions: session
                .results
                .iter()
                .map(|result| matches!(result, crate::engine::session::CharResult::Correct))
                .collect(),
            final_cursor: session.cursor,
        };
        let character_stats = session
            .char_time_ms
            .iter()
            .map(|(character, (time, attempts))| {
                (
                    *character,
                    (
                        *time,
                        *attempts,
                        *session.char_correct.get(character).unwrap_or(&0),
                    ),
                )
            })
            .collect();
        let wpm = session.wpm();
        let result = SessionResult {
            content,
            language,
            new_personal_best: session.total_keystrokes > 0
                && wpm > self.stats_store.best_wpm(&test_id),
            test_id,
            wpm,
            cpm: session.cpm(),
            accuracy: session.accuracy(),
            duration_secs: session.elapsed_secs(),
            total_keystrokes: session.total_keystrokes,
            correct_keystrokes: session.correct_keystrokes,
            mistakes: session.mistakes,
            corrections: session.corrections,
            source,
            snippet_id,
            repository,
            confusion: session.confusion,
            character_stats,
            analysis,
        };
        self.stats_store.record(&result);
        if let Err(error) = self.stats_store.save() {
            self.status = Some(format!("Could not save stats: {error}"));
        }
        self.last_result = Some(result);
        self.state = AppState::Results;
    }
}

fn default_player_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "player".into())
        .chars()
        .filter(|character| !character.is_control())
        .take(16)
        .collect()
}

fn word_sample_size(mode: SessionMode) -> usize {
    match mode {
        SessionMode::WordCount(count) => count as usize,
        SessionMode::Timed(seconds) => seconds as usize * 5,
        SessionMode::Snippet => 0,
    }
}

fn cycle<T: Copy + PartialEq>(current: T, values: &[T], forward: bool) -> T {
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let next = if forward {
        (index + 1) % values.len()
    } else {
        index.checked_sub(1).unwrap_or(values.len() - 1)
    };
    values[next]
}

#[cfg(test)]
mod tests {
    use super::{
        App, CaretStyle, ContentSource, SessionMode, Settings, StatsStore, TypingContent,
        WordLanguage, WordListSize, word_sample_size,
    };

    #[test]
    fn lowercase_words_is_enabled_by_default() {
        assert!(Settings::default().lowercase_words);
    }

    #[test]
    fn timed_sessions_have_enough_words_for_300_wpm() {
        assert_eq!(word_sample_size(SessionMode::Timed(120)), 600);
        assert_eq!(word_sample_size(SessionMode::WordCount(25)), 25);
    }

    #[test]
    fn saved_settings_are_restored() {
        let settings = Settings {
            lowercase_words: false,
            caret_style: CaretStyle::SteadyBlock,
            ..Settings::default()
        };
        let app = App::with_stats(
            StatsStore {
                settings: Some(settings.clone()),
                ..StatsStore::default()
            },
            None,
        );
        assert_eq!(app.settings, settings);
    }

    #[test]
    fn retry_resets_active_session() {
        let mut app = App::with_stats(StatsStore::default(), None);
        app.begin(
            TypingContent {
                text: "ab".into(),
                source: ContentSource::Words {
                    language: WordLanguage::English,
                    size: WordListSize::Top200,
                },
            },
            SessionMode::WordCount(1),
        );
        app.type_char('a');
        app.retry();
        assert_eq!(app.session.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn old_settings_receive_default_caret() {
        let settings: Settings = serde_json::from_str(
            r#"{"practice":"Words","word_language":"English","word_list":"Top1k","word_length":"Any","word_mode":{"Timed":30},"code_language":"Rust","mistake_mode":"Strict","lowercase_words":true}"#,
        )
        .unwrap();
        assert_eq!(settings.caret_style, CaretStyle::BlinkingBar);
        assert!(!settings.code_autopairs);
    }
}
