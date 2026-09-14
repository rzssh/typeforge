use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::engine::content::{
    CodeLanguage, ContentSource, MistakeMode, PracticeKind, Snippet, TypingContent, WordLanguage,
    WordLength, WordListSize,
};
use crate::engine::corpus::{self, LoadRequest, LoadResult};
use crate::engine::session::{Session, SessionMode};
use crate::stats::history::StatsStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Menu,
    Loading,
    Typing,
    Results,
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
}

pub struct App {
    pub state: AppState,
    pub settings: Settings,
    pub menu_selection: usize,
    pub session: Option<Session>,
    pub last_result: Option<SessionResult>,
    pub stats_store: StatsStore,
    pub status: Option<String>,
    pub show_typed: bool,
    loader: Option<Receiver<Result<LoadResult, String>>>,
    last_content: Option<TypingContent>,
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
            session: None,
            last_result: None,
            stats_store,
            status,
            show_typed: false,
            loader: None,
            last_content: None,
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
            self.start_session(false);
        } else {
            self.menu_change(1);
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

    pub fn cancel_loading(&mut self) {
        self.loader = None;
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

    pub fn finish_session(&mut self) {
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
        };
        self.stats_store.record(&result);
        if let Err(error) = self.stats_store.save() {
            self.status = Some(format!("Could not save stats: {error}"));
        }
        self.last_result = Some(result);
        self.state = AppState::Results;
    }
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
