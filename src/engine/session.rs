use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engine::content::{MistakeMode, TypingContent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    Timed(u64),
    WordCount(u32),
    Snippet,
}

impl SessionMode {
    pub fn label(self) -> String {
        match self {
            Self::Timed(seconds) => format!("{seconds}s"),
            Self::WordCount(words) => format!("{words} words"),
            Self::Snippet => "complete snippet".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharResult {
    Pending,
    Correct,
    Skipped,
    Incorrect(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeystrokeEvent {
    pub elapsed_ms: u64,
    pub position: usize,
    pub expected: char,
    pub typed: char,
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub elapsed_ms: u64,
    pub correct_chars: u32,
}

pub struct Session {
    pub content: TypingContent,
    pub chars: Vec<char>,
    pub results: Vec<CharResult>,
    pub cursor: usize,
    pub mode: SessionMode,
    pub mistake_mode: MistakeMode,
    pub start_time: Option<Instant>,
    pub total_keystrokes: u32,
    pub correct_keystrokes: u32,
    pub mistakes: u32,
    pub corrections: u32,
    pub confusion: HashMap<String, u32>,
    strict_error_start: Option<usize>,
    last_keystroke: Option<Instant>,
    pub char_time_ms: HashMap<char, (u64, u32)>,
    pub char_correct: HashMap<char, u32>,
    pub keystrokes: Vec<KeystrokeEvent>,
    pub progress: Vec<ProgressEvent>,
    autopair_openers: Vec<Option<usize>>,
}

impl Session {
    pub fn new(
        content: TypingContent,
        mode: SessionMode,
        mistake_mode: MistakeMode,
        autopairs: bool,
    ) -> Self {
        let chars: Vec<char> = content.text.chars().collect();
        let autopair_openers = if autopairs {
            matched_bracket_openers(&chars)
        } else {
            vec![None; chars.len()]
        };
        Self {
            content,
            results: vec![CharResult::Pending; chars.len()],
            chars,
            cursor: 0,
            mode,
            mistake_mode,
            start_time: None,
            total_keystrokes: 0,
            correct_keystrokes: 0,
            mistakes: 0,
            corrections: 0,
            confusion: HashMap::new(),
            strict_error_start: None,
            last_keystroke: None,
            char_time_ms: HashMap::new(),
            char_correct: HashMap::new(),
            keystrokes: Vec::new(),
            progress: Vec::new(),
            autopair_openers,
        }
    }

    pub fn type_char(&mut self, typed: char) {
        self.skip_auto_paired_closers();
        if self.cursor >= self.chars.len() {
            return;
        }
        let now = Instant::now();
        self.start_time.get_or_insert(now);
        let expected = self.chars[self.cursor];
        let elapsed_ms = self
            .last_keystroke
            .map(|last| now.duration_since(last).as_millis() as u64)
            .unwrap_or(0);
        self.last_keystroke = Some(now);
        self.total_keystrokes += 1;
        self.keystrokes.push(KeystrokeEvent {
            elapsed_ms: self
                .start_time
                .map(|start| now.duration_since(start).as_millis() as u64)
                .unwrap_or(0),
            position: self.cursor,
            expected,
            typed,
            accepted: typed == expected && self.strict_error_start.is_none(),
        });
        let timing = self.char_time_ms.entry(expected).or_default();
        timing.0 += elapsed_ms;
        timing.1 += 1;

        if typed == expected && self.strict_error_start.is_none() {
            self.results[self.cursor] = CharResult::Correct;
            self.correct_keystrokes += 1;
            *self.char_correct.entry(expected).or_default() += 1;
            self.cursor += 1;
            self.skip_indentation_after_newline(expected);
        } else {
            self.results[self.cursor] = CharResult::Incorrect(typed);
            self.mistakes += 1;
            if typed != expected {
                *self
                    .confusion
                    .entry(format!("{}→{}", visible(expected), visible(typed)))
                    .or_default() += 1;
            }
            if self.mistake_mode == MistakeMode::Strict && self.strict_error_start.is_none() {
                self.strict_error_start = Some(self.cursor);
            }
            self.cursor += 1;
        }
        self.skip_auto_paired_closers();
        self.record_progress(now);
    }

    fn record_progress(&mut self, now: Instant) {
        let Some(start) = self.start_time else {
            return;
        };
        self.progress.push(ProgressEvent {
            elapsed_ms: now.duration_since(start).as_millis() as u64,
            correct_chars: self.correct_keystrokes,
        });
    }

    fn skip_indentation_after_newline(&mut self, expected: char) {
        if expected != '\n' {
            return;
        }
        while matches!(self.chars.get(self.cursor), Some(' ' | '\t')) {
            self.results[self.cursor] = CharResult::Skipped;
            self.cursor += 1;
        }
    }

    fn skip_auto_paired_closers(&mut self) {
        let auto_paired_tail = self.strict_error_start.is_none()
            && self.chars.get(self.cursor) == Some(&'\n')
            && self.chars[self.cursor..]
                .iter()
                .enumerate()
                .all(|(offset, character)| {
                    character.is_whitespace() || self.is_auto_paired(self.cursor + offset)
                })
            && (self.cursor..self.chars.len()).any(|index| self.is_auto_paired(index));
        if auto_paired_tail {
            self.results[self.cursor..].fill(CharResult::Skipped);
            self.cursor = self.chars.len();
            return;
        }
        while self.is_auto_paired(self.cursor) {
            self.results[self.cursor] = CharResult::Skipped;
            self.cursor += 1;
        }
    }

    pub fn is_auto_paired(&self, index: usize) -> bool {
        self.autopair_openers
            .get(index)
            .and_then(|opener| *opener)
            .is_some_and(|opener| self.results[opener] == CharResult::Correct)
    }

    pub fn backspace(&mut self) {
        self.skip_auto_pairs_backward();
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        if self.strict_error_start == Some(self.cursor) {
            self.strict_error_start = None;
        }
        if self.results[self.cursor] == CharResult::Correct {
            self.correct_keystrokes = self.correct_keystrokes.saturating_sub(1);
        }
        self.results[self.cursor] = CharResult::Pending;
        self.corrections += 1;
        self.record_progress(Instant::now());
    }

    fn skip_auto_pairs_backward(&mut self) {
        while self.cursor > 0 && self.is_auto_paired(self.cursor - 1) {
            self.cursor -= 1;
            self.results[self.cursor] = CharResult::Pending;
        }
    }

    pub fn delete_word(&mut self) {
        self.skip_auto_pairs_backward();
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
        let Some(previous) = self.chars.get(self.cursor.wrapping_sub(1)).copied() else {
            return;
        };
        let word = previous.is_alphanumeric() || previous == '_';
        while self.cursor > 0 {
            let character = self.chars[self.cursor - 1];
            if character.is_whitespace()
                || (character.is_alphanumeric() || character == '_') != word
            {
                break;
            }
            self.backspace();
        }
    }

    pub fn delete_line(&mut self) {
        self.skip_auto_pairs_backward();
        while self.cursor > 0 && self.chars[self.cursor - 1] != '\n' {
            self.backspace();
        }
    }

    pub fn latest_error(&self) -> Option<(char, char)> {
        if let Some(index) = self.strict_error_start
            && let CharResult::Incorrect(typed) = self.results[index]
        {
            return Some((self.chars[index], typed));
        }
        self.results[..self.cursor.min(self.results.len())]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, result)| match result {
                CharResult::Incorrect(typed) if self.chars[index] != *typed => {
                    Some((self.chars[index], *typed))
                }
                _ => None,
            })
    }

    pub fn start_with_elapsed(&mut self, elapsed: Duration) {
        let now = Instant::now();
        self.start_time = Some(now.checked_sub(elapsed).unwrap_or(now));
    }

    pub fn is_complete(&self) -> bool {
        self.cursor >= self.chars.len() && self.strict_error_start.is_none()
    }

    pub fn is_timed_out(&self) -> bool {
        matches!(self.mode, SessionMode::Timed(seconds) if self.start_time.is_some_and(|start| start.elapsed().as_secs_f64() >= seconds as f64))
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start_time
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }

    pub fn accuracy(&self) -> f64 {
        if self.total_keystrokes == 0 {
            return 1.0;
        }
        self.correct_keystrokes as f64 / self.total_keystrokes as f64
    }

    pub fn wpm(&self) -> f64 {
        let elapsed = self.elapsed_secs();
        if elapsed < 0.1 {
            return 0.0;
        }
        (self.correct_keystrokes as f64 / 5.0) / (elapsed / 60.0)
    }

    pub fn cpm(&self) -> f64 {
        let elapsed = self.elapsed_secs();
        if elapsed < 0.1 {
            return 0.0;
        }
        self.correct_keystrokes as f64 / (elapsed / 60.0)
    }

    pub fn progress(&self) -> f64 {
        if self.chars.is_empty() {
            1.0
        } else {
            self.cursor as f64 / self.chars.len() as f64
        }
    }
}

fn matched_bracket_openers(chars: &[char]) -> Vec<Option<usize>> {
    let mut openers = Vec::new();
    let mut matches = vec![None; chars.len()];
    for (index, character) in chars.iter().copied().enumerate() {
        if let Some(closer) = match character {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        } {
            openers.push((index, closer));
        } else if openers
            .last()
            .is_some_and(|(_, closer)| *closer == character)
        {
            matches[index] = openers.pop().map(|(index, _)| index);
        }
    }
    matches
}

fn visible(character: char) -> String {
    match character {
        ' ' => "space".into(),
        '\n' => "enter".into(),
        '\t' => "tab".into(),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::content::{ContentSource, WordLanguage, WordListSize};

    fn content(text: &str) -> TypingContent {
        TypingContent {
            text: text.into(),
            source: ContentSource::Words {
                language: WordLanguage::English,
                size: WordListSize::Top200,
            },
        }
    }

    #[test]
    fn strict_mode_marks_tail_until_error_is_deleted() {
        let mut session = Session::new(
            content("abcd"),
            SessionMode::WordCount(1),
            MistakeMode::Strict,
            false,
        );
        session.type_char('x');
        session.type_char('b');
        assert_eq!(session.cursor, 2);
        assert_eq!(session.results[1], CharResult::Incorrect('b'));
        session.backspace();
        session.backspace();
        session.type_char('a');
        assert_eq!(session.cursor, 1);
        assert_eq!(session.mistakes, 2);
    }

    #[test]
    fn corrected_mistakes_remain_in_the_keystroke_timeline() {
        let mut session = Session::new(
            content("ab"),
            SessionMode::WordCount(1),
            MistakeMode::Strict,
            false,
        );
        session.type_char('x');
        session.backspace();
        session.type_char('a');
        assert_eq!(session.keystrokes.len(), 2);
        assert_eq!(session.keystrokes[0].typed, 'x');
        assert!(!session.keystrokes[0].accepted);
        assert!(session.keystrokes[1].accepted);
    }

    #[test]
    fn progress_timeline_tracks_deleted_correct_characters() {
        let mut session = Session::new(
            content("ab"),
            SessionMode::WordCount(1),
            MistakeMode::Strict,
            false,
        );
        session.type_char('a');
        session.backspace();
        session.type_char('a');
        assert_eq!(
            session
                .progress
                .iter()
                .map(|event| event.correct_chars)
                .collect::<Vec<_>>(),
            vec![1, 0, 1]
        );
    }

    #[test]
    fn free_mode_advances_on_error() {
        let mut session = Session::new(
            content("ab"),
            SessionMode::WordCount(1),
            MistakeMode::Free,
            false,
        );
        session.type_char('x');
        assert_eq!(session.cursor, 1);
    }

    #[test]
    fn strict_error_tail_cannot_complete_session() {
        let mut session = Session::new(
            content("ab"),
            SessionMode::WordCount(1),
            MistakeMode::Strict,
            false,
        );
        session.type_char('x');
        session.type_char('b');
        assert_eq!(session.cursor, 2);
        assert!(!session.is_complete());
    }

    #[test]
    fn word_delete_uses_editor_boundaries() {
        let mut session = Session::new(
            content("alpha beta_value + gamma"),
            SessionMode::WordCount(3),
            MistakeMode::Free,
            false,
        );
        for character in "alpha beta_value + ".chars() {
            session.type_char(character);
        }
        session.delete_word();
        assert_eq!(session.cursor, "alpha beta_value ".chars().count());
        session.delete_word();
        assert_eq!(session.cursor, "alpha ".chars().count());
    }

    #[test]
    fn newline_skips_existing_indentation() {
        let mut session = Session::new(
            content("a\n    b"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            false,
        );
        session.type_char('a');
        session.type_char('\n');
        assert_eq!(session.cursor, 6);
    }

    #[test]
    fn autopairs_skip_matched_closing_brackets() {
        let mut session = Session::new(
            content("call([x]) { y }"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "call(".chars() {
            session.type_char(character);
        }
        assert!(session.is_auto_paired(8));
        assert_eq!(session.results[8], CharResult::Pending);
        for character in "[x { y ".chars() {
            session.type_char(character);
        }
        assert!(session.is_complete());
        assert_eq!(session.total_keystrokes, 12);
        assert_eq!(
            session
                .results
                .iter()
                .filter(|result| **result == CharResult::Skipped)
                .count(),
            3
        );
    }

    #[test]
    fn backspace_skips_nested_auto_paired_closers() {
        let mut session = Session::new(
            content("([x])y"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "([x".chars() {
            session.type_char(character);
        }
        session.backspace();
        assert_eq!(session.cursor, 2);
        assert_eq!(session.results[2], CharResult::Pending);
        assert_eq!(session.results[3..5], [CharResult::Pending; 2]);
        assert_eq!(session.corrections, 1);
        session.type_char('x');
        assert_eq!(session.cursor, 5);
    }

    #[test]
    fn backspace_removes_empty_auto_pair_as_one_input() {
        let mut session = Session::new(
            content("()x"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        session.type_char('(');
        session.backspace();
        assert_eq!(session.cursor, 0);
        assert_eq!(&session.results[..2], &[CharResult::Pending; 2]);
        assert!(!session.is_auto_paired(1));
        assert_eq!(session.corrections, 1);
    }

    #[test]
    fn backspace_crosses_auto_pair_without_clearing_earlier_strict_error() {
        let mut session = Session::new(
            content("(ab)c"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        session.type_char('(');
        session.type_char('x');
        session.type_char('b');
        session.backspace();
        assert_eq!(session.cursor, 2);
        assert_eq!(session.latest_error(), Some(('a', 'x')));
        session.backspace();
        assert_eq!(session.cursor, 1);
        assert_eq!(session.latest_error(), None);
    }

    #[test]
    fn backspace_does_not_skip_unmatched_closers() {
        let mut session = Session::new(
            content("x}y"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        session.type_char('x');
        session.type_char('}');
        session.backspace();
        assert_eq!(session.cursor, 1);
        assert_eq!(session.results[1], CharResult::Pending);
    }

    #[test]
    fn word_delete_ignores_auto_paired_closers() {
        let mut session = Session::new(
            content("(word) x"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "(word".chars() {
            session.type_char(character);
        }
        session.delete_word();
        assert_eq!(session.cursor, 1);
        assert_eq!(session.results[5], CharResult::Pending);
    }

    #[test]
    fn line_delete_stops_before_auto_paired_closing_line() {
        let mut session = Session::new(
            content("{\nx\n}y"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "{\nx\n".chars() {
            session.type_char(character);
        }
        session.delete_line();
        assert_eq!(session.cursor, 4);
        assert_eq!(session.results[4], CharResult::Pending);
    }

    #[test]
    fn autopairs_finish_before_final_closing_line() {
        let mut session = Session::new(
            content("{\nx\n}"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "{\nx".chars() {
            session.type_char(character);
        }
        assert!(session.is_complete());
        assert_eq!(&session.results[3..], &[CharResult::Skipped; 2]);
    }

    #[test]
    fn autopairs_skip_closers_after_free_mode_errors() {
        let mut session = Session::new(
            content("(a)"),
            SessionMode::Snippet,
            MistakeMode::Free,
            true,
        );
        session.type_char('(');
        session.type_char('x');
        assert!(session.is_complete());
        assert_eq!(session.results[2], CharResult::Skipped);
    }

    #[test]
    fn autopairs_do_not_skip_unmatched_closers() {
        let mut session = Session::new(
            content("print(\"}\")"),
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        for character in "print(\"}\"".chars() {
            session.type_char(character);
        }
        assert!(session.is_complete());
        assert_eq!(session.results[7], CharResult::Correct);
        assert_eq!(session.results[9], CharResult::Skipped);
    }
}
