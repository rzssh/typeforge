use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::content::{MistakeMode, TypingContent};
use crate::engine::session::SessionMode;

pub const MAX_PLAYERS: usize = 8;
pub const COUNTDOWN_MS: u64 = 3_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Create {
        name: String,
        content: TypingContent,
        mode: SessionMode,
        mistake_mode: MistakeMode,
        autopairs: bool,
    },
    Join {
        code: String,
        name: String,
        resume_token: Option<String>,
    },
    SetReady {
        ready: bool,
    },
    Start,
    Progress {
        cursor: usize,
        wpm: f64,
        accuracy: f64,
        mistakes: u32,
        finished: bool,
        elapsed_ms: u64,
    },
    Ping {
        nonce: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        player_id: String,
        resume_token: String,
        room: RoomSnapshot,
    },
    Snapshot {
        room: RoomSnapshot,
    },
    Pong {
        nonce: u64,
        server_time_ms: u64,
    },
    Error {
        message: String,
        fatal: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub code: String,
    pub host_id: String,
    pub content: TypingContent,
    pub mode: SessionMode,
    pub mistake_mode: MistakeMode,
    pub autopairs: bool,
    pub phase: RoomPhase,
    pub players: Vec<PlayerSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoomPhase {
    Lobby,
    Countdown { starts_at_ms: u64 },
    Racing { starts_at_ms: u64 },
    Finished { starts_at_ms: u64 },
}

impl RoomPhase {
    pub fn starts_at_ms(self) -> Option<u64> {
        match self {
            Self::Lobby => None,
            Self::Countdown { starts_at_ms }
            | Self::Racing { starts_at_ms }
            | Self::Finished { starts_at_ms } => Some(starts_at_ms),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: String,
    pub name: String,
    pub ready: bool,
    pub connected: bool,
    pub cursor: usize,
    pub wpm: f64,
    pub accuracy: f64,
    pub mistakes: u32,
    pub finished: bool,
    pub elapsed_ms: Option<u64>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
