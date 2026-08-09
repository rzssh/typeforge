use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::{SessionResult, Settings};

const MAX_SESSIONS: usize = 1_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsStore {
    pub version: u32,
    pub settings: Option<Settings>,
    pub sessions: Vec<SessionRecord>,
    pub seen_snippets: HashSet<String>,
    pub character_attempts: HashMap<char, CharacterStats>,
    pub confusions: HashMap<String, u32>,
    pub last_repository: Option<String>,
    pub recent_repositories: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterStats {
    pub correct: u32,
    pub attempts: u32,
    pub total_time_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub language: String,
    pub test_id: String,
    pub duration_secs: f64,
    pub wpm_net: f64,
    pub cpm: f64,
    pub accuracy: f64,
    pub total_keystrokes: u32,
    pub correct_keystrokes: u32,
    pub mistakes: u32,
    pub corrections: u32,
    pub source: Option<String>,
}

impl StatsStore {
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (
                Self::default(),
                Some("Data directory unavailable; settings and stats will not persist".into()),
            );
        };
        Self::load_from(&path)
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("Could not read stats: {error}")),
                );
            }
        };
        match serde_json::from_str(&text) {
            Ok(store) => (store, None),
            Err(error) => {
                let backup = corrupt_path(path);
                let status = match fs::rename(path, &backup) {
                    Ok(()) => format!("Invalid stats preserved at {}: {error}", backup.display()),
                    Err(backup_error) => {
                        format!("Invalid stats could not be preserved: {error}; {backup_error}")
                    }
                };
                (Self::default(), Some(status))
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path().context("data directory unavailable")?;
        self.save_to(&path)
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("stats path has no parent")?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    pub fn record(&mut self, result: &SessionResult) {
        let id = self.sessions.last().map_or(1, |session| session.id + 1);
        self.sessions.push(SessionRecord {
            id,
            timestamp: Utc::now(),
            content: result.content.clone(),
            language: result.language.clone(),
            test_id: result.test_id.clone(),
            duration_secs: result.duration_secs,
            wpm_net: result.wpm,
            cpm: result.cpm,
            accuracy: result.accuracy,
            total_keystrokes: result.total_keystrokes,
            correct_keystrokes: result.correct_keystrokes,
            mistakes: result.mistakes,
            corrections: result.corrections,
            source: result.source.clone(),
        });
        if self.sessions.len() > MAX_SESSIONS {
            self.sessions.drain(..self.sessions.len() - MAX_SESSIONS);
        }
        for (character, (total_ms, attempts, correct)) in &result.character_stats {
            let stats = self.character_attempts.entry(*character).or_default();
            stats.total_time_ms += total_ms;
            stats.attempts += attempts;
            stats.correct += correct;
        }
        for (pair, count) in &result.confusion {
            *self.confusions.entry(pair.clone()).or_default() += count;
        }
        if let Some(id) = &result.snippet_id {
            self.seen_snippets.insert(id.clone());
        }
        if let Some(repository) = &result.repository {
            self.remember_repository(repository);
        }
    }

    pub fn remember_repository(&mut self, repository: &str) {
        self.last_repository = Some(repository.into());
        self.recent_repositories
            .retain(|existing| existing != repository);
        self.recent_repositories.push(repository.into());
        if self.recent_repositories.len() > 5 {
            self.recent_repositories
                .drain(..self.recent_repositories.len() - 5);
        }
    }

    pub fn average_wpm(&self) -> f64 {
        average(self.sessions.iter().map(|session| session.wpm_net))
    }

    pub fn average_accuracy(&self) -> f64 {
        average(self.sessions.iter().map(|session| session.accuracy))
    }

    pub fn total_time(&self) -> f64 {
        self.sessions
            .iter()
            .map(|session| session.duration_secs)
            .sum()
    }

    pub fn best_wpm(&self, test_id: &str) -> f64 {
        self.sessions
            .iter()
            .filter(|session| session.test_id == test_id)
            .map(|session| session.wpm_net)
            .fold(0.0, f64::max)
    }

    pub fn recent_wpm(&self, test_id: &str, limit: usize) -> Vec<f64> {
        self.sessions
            .iter()
            .rev()
            .filter(|session| session.test_id == test_id)
            .take(limit)
            .map(|session| session.wpm_net)
            .collect()
    }

    pub fn weakest_characters(&self) -> Vec<(char, f64, f64)> {
        let mut values: Vec<_> = self
            .character_attempts
            .iter()
            .filter(|(_, stats)| stats.attempts >= 3)
            .map(|(character, stats)| {
                (
                    *character,
                    stats.correct as f64 / stats.attempts as f64,
                    stats.total_time_ms as f64 / stats.attempts as f64,
                )
            })
            .collect();
        values.sort_by(|left, right| left.1.total_cmp(&right.1).then(right.2.total_cmp(&left.2)));
        values.truncate(8);
        values
    }

    pub fn top_confusions(&self) -> Vec<(String, u32)> {
        let mut values: Vec<_> = self
            .confusions
            .iter()
            .map(|(pair, count)| (pair.clone(), *count))
            .collect();
        values.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        values.truncate(8);
        values
    }

    fn path() -> Option<PathBuf> {
        dirs::data_dir().map(|directory| directory.join("typeforge").join("stats.json"))
    }
}

fn corrupt_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| "stats.json".into(), |name| name.to_string_lossy());
    path.with_file_name(format!(
        "{name}.corrupt-{}-{}",
        Utc::now().timestamp_millis(),
        std::process::id()
    ))
}

fn average(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<_> = values.collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("typeforge-history-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn result(test_id: &str, wpm: f64) -> SessionResult {
        SessionResult {
            content: "30s".into(),
            language: "English 1k".into(),
            test_id: test_id.into(),
            new_personal_best: false,
            wpm,
            cpm: wpm * 5.0,
            accuracy: 0.98,
            duration_secs: 30.0,
            total_keystrokes: 100,
            correct_keystrokes: 98,
            mistakes: 2,
            corrections: 2,
            source: None,
            snippet_id: None,
            repository: None,
            confusion: HashMap::new(),
            character_stats: HashMap::new(),
        }
    }

    #[test]
    fn settings_and_sessions_round_trip() {
        let directory = temporary_directory();
        let path = directory.join("stats.json");
        let settings = Settings {
            lowercase_words: false,
            ..Settings::default()
        };
        let mut store = StatsStore {
            settings: Some(settings.clone()),
            ..StatsStore::default()
        };
        store.record(&result("words:english:1k:30s", 72.0));
        store.save_to(&path).unwrap();

        let (loaded, status) = StatsStore::load_from(&path);
        assert!(status.is_none());
        assert_eq!(loaded.settings, Some(settings));
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].test_id, "words:english:1k:30s");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recent_results_and_personal_best_are_comparable() {
        let mut store = StatsStore::default();
        store.record(&result("words:english:1k:30s", 40.0));
        store.record(&result("code:rust", 90.0));
        store.record(&result("words:english:1k:30s", 55.0));

        assert_eq!(store.best_wpm("words:english:1k:30s"), 55.0);
        assert_eq!(
            store.recent_wpm("words:english:1k:30s", 5),
            vec![55.0, 40.0]
        );
    }

    #[test]
    fn corrupt_stats_are_preserved() {
        let directory = temporary_directory();
        let path = directory.join("stats.json");
        fs::write(&path, b"not json").unwrap();

        let (store, status) = StatsStore::load_from(&path);
        assert!(store.sessions.is_empty());
        assert!(status.unwrap().contains("Invalid stats preserved"));
        assert!(!path.exists());
        assert!(fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("stats.json.corrupt-")
        }));
        fs::remove_dir_all(directory).unwrap();
    }
}
