use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use super::protocol::{
    COUNTDOWN_MS, ClientMessage, MAX_PLAYERS, PlayerSnapshot, RoomPhase, RoomSnapshot,
    ServerMessage, now_ms,
};

struct RelayState {
    rooms: HashMap<String, Room>,
}

struct Room {
    code: String,
    host_id: String,
    content: crate::engine::content::TypingContent,
    mode: crate::engine::session::SessionMode,
    mistake_mode: crate::engine::content::MistakeMode,
    autopairs: bool,
    phase: RoomPhase,
    players: HashMap<String, Player>,
    next_order: usize,
    last_active: Instant,
}

struct Player {
    snapshot: PlayerSnapshot,
    resume_token: String,
    connection_id: String,
    order: usize,
    sender: UnboundedSender<ServerMessage>,
}

impl Room {
    fn snapshot(&self) -> RoomSnapshot {
        let mut players: Vec<_> = self
            .players
            .values()
            .map(|player| (player.order, player.snapshot.clone()))
            .collect();
        players.sort_by_key(|(order, _)| *order);
        RoomSnapshot {
            code: self.code.clone(),
            host_id: self.host_id.clone(),
            content: self.content.clone(),
            mode: self.mode,
            mistake_mode: self.mistake_mode,
            autopairs: self.autopairs,
            phase: self.phase,
            players: players.into_iter().map(|(_, player)| player).collect(),
        }
    }

    fn broadcast(&self) {
        let message = ServerMessage::Snapshot {
            room: self.snapshot(),
        };
        for player in self.players.values() {
            let _ = player.sender.send(message.clone());
        }
    }
}

pub fn run(address: &str) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(address))
}

async fn serve(address: &str) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("could not bind relay to {address}"))?;
    println!("TypeForge relay listening on {address}");
    let state = Arc::new(Mutex::new(RelayState {
        rooms: HashMap::new(),
    }));
    let cleanup_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let Ok(mut state) = cleanup_state.lock() else {
                continue;
            };
            state.rooms.retain(|_, room| {
                room.players
                    .values()
                    .any(|player| player.snapshot.connected)
                    || room.last_active.elapsed() < Duration::from_secs(300)
            });
        }
    });
    loop {
        let (stream, _) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state).await {
                eprintln!("relay connection: {error:#}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, state: Arc<Mutex<RelayState>>) -> Result<()> {
    let config = WebSocketConfig::default().max_message_size(Some(64 * 1024));
    let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await?;
    let first = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .context("timed out waiting for room request")?
        .context("connection closed before room request")??;
    let Message::Text(text) = first else {
        bail!("first message was not text");
    };
    let hello: ClientMessage = serde_json::from_str(&text).context("invalid room request")?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let identity = match connect(&state, hello, sender) {
        Ok(identity) => identity,
        Err(message) => {
            send_socket(
                &mut socket,
                &ServerMessage::Error {
                    message,
                    fatal: true,
                },
            )
            .await?;
            return Ok(());
        }
    };
    let mut last_progress = Instant::now() - Duration::from_millis(100);
    loop {
        tokio::select! {
            outgoing = receiver.recv() => {
                let Some(outgoing) = outgoing else {
                    break;
                };
                if send_socket(&mut socket, &outgoing).await.is_err() {
                    break;
                }
            }
            incoming = socket.next() => {
                let Some(Ok(incoming)) = incoming else {
                    break;
                };
                let Message::Text(text) = incoming else {
                    continue;
                };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else {
                    continue;
                };
                if matches!(&message, ClientMessage::Progress { finished: false, .. }) {
                    if last_progress.elapsed() < Duration::from_millis(50) {
                        continue;
                    }
                    last_progress = Instant::now();
                }
                handle_message(&state, &identity.0, &identity.1, message);
            }
        }
    }
    disconnect(&state, &identity.0, &identity.1, &identity.2);
    Ok(())
}

fn connect(
    state: &Arc<Mutex<RelayState>>,
    hello: ClientMessage,
    sender: UnboundedSender<ServerMessage>,
) -> std::result::Result<(String, String, String), String> {
    let mut state = state.lock().map_err(|_| "Relay unavailable".to_string())?;
    match hello {
        ClientMessage::Create {
            name,
            content,
            mode,
            mistake_mode,
            autopairs,
        } => {
            validate_content(&content)?;
            let name = validate_name(&name)?;
            let code = unique_room_code(&state.rooms);
            let player_id = token(12);
            let resume_token = token(32);
            let connection_id = token(12);
            let snapshot = PlayerSnapshot {
                id: player_id.clone(),
                name,
                ready: false,
                connected: true,
                cursor: 0,
                wpm: 0.0,
                accuracy: 1.0,
                mistakes: 0,
                finished: false,
                elapsed_ms: None,
            };
            let mut players = HashMap::new();
            players.insert(
                player_id.clone(),
                Player {
                    snapshot,
                    resume_token: resume_token.clone(),
                    connection_id: connection_id.clone(),
                    order: 0,
                    sender: sender.clone(),
                },
            );
            let room = Room {
                code: code.clone(),
                host_id: player_id.clone(),
                content,
                mode,
                mistake_mode,
                autopairs,
                phase: RoomPhase::Lobby,
                players,
                next_order: 1,
                last_active: Instant::now(),
            };
            let snapshot = room.snapshot();
            state.rooms.insert(code.clone(), room);
            let _ = sender.send(ServerMessage::Welcome {
                player_id: player_id.clone(),
                resume_token,
                room: snapshot,
            });
            Ok((code, player_id, connection_id))
        }
        ClientMessage::Join {
            code,
            name,
            resume_token,
        } => {
            let code = code.trim().to_ascii_uppercase();
            let name = validate_name(&name)?;
            let room = state
                .rooms
                .get_mut(&code)
                .ok_or_else(|| "Room not found".to_string())?;
            room.last_active = Instant::now();
            if let Some((player_id, player)) = resume_token.as_ref().and_then(|token| {
                room.players
                    .iter_mut()
                    .find(|(_, player)| &player.resume_token == token)
            }) {
                player.snapshot.connected = true;
                player.sender = sender.clone();
                player.connection_id = token(12);
                let player_id = player_id.clone();
                let resume_token = player.resume_token.clone();
                let connection_id = player.connection_id.clone();
                let snapshot = room.snapshot();
                let _ = sender.send(ServerMessage::Welcome {
                    player_id: player_id.clone(),
                    resume_token,
                    room: snapshot,
                });
                room.broadcast();
                return Ok((code, player_id, connection_id));
            }
            if room.phase != RoomPhase::Lobby {
                return Err("Race already started".into());
            }
            if room.players.len() >= MAX_PLAYERS {
                return Err("Room is full".into());
            }
            if room
                .players
                .values()
                .any(|player| player.snapshot.name.eq_ignore_ascii_case(&name))
            {
                return Err("Name already in use".into());
            }
            let player_id = token(12);
            let resume_token = token(32);
            let snapshot = PlayerSnapshot {
                id: player_id.clone(),
                name,
                ready: false,
                connected: true,
                cursor: 0,
                wpm: 0.0,
                accuracy: 1.0,
                mistakes: 0,
                finished: false,
                elapsed_ms: None,
            };
            let connection_id = token(12);
            let order = room.next_order;
            room.next_order += 1;
            room.players.insert(
                player_id.clone(),
                Player {
                    snapshot,
                    resume_token: resume_token.clone(),
                    connection_id: connection_id.clone(),
                    order,
                    sender: sender.clone(),
                },
            );
            let snapshot = room.snapshot();
            let _ = sender.send(ServerMessage::Welcome {
                player_id: player_id.clone(),
                resume_token,
                room: snapshot,
            });
            room.broadcast();
            Ok((code, player_id, connection_id))
        }
        _ => Err("Expected create or join request".into()),
    }
}

fn handle_message(
    state: &Arc<Mutex<RelayState>>,
    code: &str,
    player_id: &str,
    message: ClientMessage,
) {
    let state_for_start = Arc::clone(state);
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(room) = state.rooms.get_mut(code) else {
        return;
    };
    room.last_active = Instant::now();
    match message {
        ClientMessage::SetReady { ready } if room.phase == RoomPhase::Lobby => {
            if let Some(player) = room.players.get_mut(player_id) {
                player.snapshot.ready = ready;
            }
            room.broadcast();
        }
        ClientMessage::Start if room.phase == RoomPhase::Lobby => {
            if room.host_id != player_id {
                send_error(room, player_id, "Only the host can start");
                return;
            }
            if room
                .players
                .values()
                .filter(|player| player.snapshot.connected)
                .count()
                < 2
            {
                send_error(room, player_id, "At least two players are required");
                return;
            }
            let starts_at_ms = now_ms() + COUNTDOWN_MS;
            room.phase = RoomPhase::Countdown { starts_at_ms };
            room.broadcast();
            let code = code.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(COUNTDOWN_MS)).await;
                let Ok(mut state) = state_for_start.lock() else {
                    return;
                };
                let Some(room) = state.rooms.get_mut(&code) else {
                    return;
                };
                if room.phase == (RoomPhase::Countdown { starts_at_ms }) {
                    room.phase = RoomPhase::Racing { starts_at_ms };
                    room.broadcast();
                }
            });
        }
        ClientMessage::Progress {
            cursor,
            wpm,
            accuracy,
            mistakes,
            finished,
            elapsed_ms,
        } if room
            .phase
            .starts_at_ms()
            .is_some_and(|start| now_ms() >= start) =>
        {
            let max_cursor = room.content.text.chars().count();
            let cursor = cursor.min(max_cursor);
            let completed = match room.mode {
                crate::engine::session::SessionMode::Timed(seconds) => {
                    elapsed_ms >= seconds.saturating_mul(1_000)
                }
                crate::engine::session::SessionMode::WordCount(_)
                | crate::engine::session::SessionMode::Snippet => cursor == max_cursor,
            };
            let finished = finished && completed;
            if let Some(player) = room.players.get_mut(player_id) {
                player.snapshot.cursor = cursor;
                player.snapshot.wpm = if wpm.is_finite() { wpm.max(0.0) } else { 0.0 };
                player.snapshot.accuracy = if accuracy.is_finite() {
                    accuracy.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                player.snapshot.mistakes = mistakes;
                player.snapshot.finished = finished;
                player.snapshot.elapsed_ms = finished.then_some(elapsed_ms);
            }
            let connected: Vec<_> = room
                .players
                .values()
                .filter(|player| player.snapshot.connected)
                .collect();
            if !connected.is_empty()
                && connected.iter().all(|player| player.snapshot.finished)
                && let Some(starts_at_ms) = room.phase.starts_at_ms()
            {
                room.phase = RoomPhase::Finished { starts_at_ms };
            }
            room.broadcast();
        }
        ClientMessage::Ping { nonce } => {
            if let Some(player) = room.players.get(player_id) {
                let _ = player.sender.send(ServerMessage::Pong {
                    nonce,
                    server_time_ms: now_ms(),
                });
            }
        }
        _ => {}
    }
}

fn disconnect(state: &Arc<Mutex<RelayState>>, code: &str, player_id: &str, connection_id: &str) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(room) = state.rooms.get_mut(code) else {
        return;
    };
    let current_connection = room
        .players
        .get(player_id)
        .is_some_and(|player| player.connection_id == connection_id);
    if !current_connection {
        return;
    }
    if let Some(player) = room.players.get_mut(player_id) {
        player.snapshot.connected = false;
    }
    if room.host_id == player_id
        && let Some(next_host) = room
            .players
            .values()
            .find(|player| player.snapshot.connected)
    {
        room.host_id = next_host.snapshot.id.clone();
    }
    room.last_active = Instant::now();
    room.broadcast();
}

async fn send_socket(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    message: &ServerMessage,
) -> Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(message)?.into()))
        .await?;
    Ok(())
}

fn send_error(room: &Room, player_id: &str, message: &str) {
    if let Some(player) = room.players.get(player_id) {
        let _ = player.sender.send(ServerMessage::Error {
            message: message.into(),
            fatal: false,
        });
    }
}

fn validate_name(name: &str) -> std::result::Result<String, String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(16)
        .collect();
    if name.is_empty() {
        Err("Enter a player name".into())
    } else {
        Ok(name)
    }
}

fn validate_content(
    content: &crate::engine::content::TypingContent,
) -> std::result::Result<(), String> {
    if content.text.is_empty() {
        return Err("Race content is empty".into());
    }
    if content.text.len() > 50_000 {
        return Err("Race content is too large".into());
    }
    Ok(())
}

fn unique_room_code(rooms: &HashMap<String, Room>) -> String {
    loop {
        let code: String = (0..6)
            .map(|_| {
                const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
                let index = rand::thread_rng().gen_range(0..ALPHABET.len());
                ALPHABET[index] as char
            })
            .collect();
        if !rooms.contains_key(&code) {
            return code;
        }
    }
}

fn token(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..length)
        .map(|_| {
            let index = rand::thread_rng().gen_range(0..ALPHABET.len());
            ALPHABET[index] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::content::{
        ContentSource, MistakeMode, TypingContent, WordLanguage, WordListSize,
    };
    use crate::engine::session::SessionMode;

    fn state() -> Arc<Mutex<RelayState>> {
        Arc::new(Mutex::new(RelayState {
            rooms: HashMap::new(),
        }))
    }

    fn content() -> TypingContent {
        TypingContent {
            text: "fn race() {}".into(),
            source: ContentSource::Words {
                language: WordLanguage::English,
                size: WordListSize::Top200,
            },
        }
    }

    fn create(state: &Arc<Mutex<RelayState>>, name: &str) -> (String, String, String, String) {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let (code, player_id, connection_id) = connect(
            state,
            ClientMessage::Create {
                name: name.into(),
                content: content(),
                mode: SessionMode::Snippet,
                mistake_mode: MistakeMode::Strict,
                autopairs: false,
            },
            sender,
        )
        .unwrap();
        let ServerMessage::Welcome { resume_token, .. } = receiver.try_recv().unwrap() else {
            panic!("expected welcome");
        };
        (code, player_id, connection_id, resume_token)
    }

    #[test]
    fn reconnect_replaces_the_old_connection() {
        let state = state();
        let (code, player_id, old_connection, resume_token) = create(&state, "host");
        disconnect(&state, &code, &player_id, &old_connection);
        let (sender, _) = mpsc::unbounded_channel();
        let (_, _, new_connection) = connect(
            &state,
            ClientMessage::Join {
                code: code.clone(),
                name: "host".into(),
                resume_token: Some(resume_token),
            },
            sender,
        )
        .unwrap();
        disconnect(&state, &code, &player_id, &old_connection);
        let state = state.lock().unwrap();
        let player = &state.rooms[&code].players[&player_id];
        assert!(player.snapshot.connected);
        assert_eq!(player.connection_id, new_connection);
    }

    #[tokio::test]
    async fn host_can_start_when_a_guest_is_not_ready() {
        let state = state();
        let (code, host_id, _, _) = create(&state, "host");
        let (sender, _) = mpsc::unbounded_channel();
        connect(
            &state,
            ClientMessage::Join {
                code: code.clone(),
                name: "guest".into(),
                resume_token: None,
            },
            sender,
        )
        .unwrap();
        handle_message(&state, &code, &host_id, ClientMessage::Start);
        assert!(matches!(
            state.lock().unwrap().rooms[&code].phase,
            RoomPhase::Countdown { .. }
        ));
    }

    #[tokio::test]
    async fn websocket_create_returns_a_room_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = state();
        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(stream, server_state).await.unwrap();
        });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .unwrap();
        let create = ClientMessage::Create {
            name: "host".into(),
            content: content(),
            mode: SessionMode::Snippet,
            mistake_mode: MistakeMode::Strict,
            autopairs: false,
        };
        socket
            .send(Message::Text(
                serde_json::to_string(&create).unwrap().into(),
            ))
            .await
            .unwrap();
        let Message::Text(message) = socket.next().await.unwrap().unwrap() else {
            panic!("expected text");
        };
        let ServerMessage::Welcome { room, .. } = serde_json::from_str(&message).unwrap() else {
            panic!("expected welcome");
        };
        assert_eq!(room.code.len(), 6);
        assert_eq!(room.players.len(), 1);
    }

    #[test]
    fn timed_race_finishes_without_reaching_the_content_end() {
        let state = state();
        let (code, player_id, _, _) = create(&state, "host");
        {
            let mut state = state.lock().unwrap();
            let room = state.rooms.get_mut(&code).unwrap();
            room.mode = SessionMode::Timed(15);
            room.phase = RoomPhase::Racing { starts_at_ms: 0 };
        }
        handle_message(
            &state,
            &code,
            &player_id,
            ClientMessage::Progress {
                cursor: 1,
                wpm: 80.0,
                accuracy: 0.98,
                mistakes: 1,
                finished: true,
                elapsed_ms: 15_000,
            },
        );
        assert!(
            state.lock().unwrap().rooms[&code].players[&player_id]
                .snapshot
                .finished
        );
    }

    #[test]
    fn progress_is_bounded_by_shared_content() {
        let state = state();
        let (code, player_id, _, _) = create(&state, "host");
        state.lock().unwrap().rooms.get_mut(&code).unwrap().phase =
            RoomPhase::Racing { starts_at_ms: 0 };
        handle_message(
            &state,
            &code,
            &player_id,
            ClientMessage::Progress {
                cursor: usize::MAX,
                wpm: 80.0,
                accuracy: 0.98,
                mistakes: 1,
                finished: false,
                elapsed_ms: 500,
            },
        );
        let state = state.lock().unwrap();
        let room = &state.rooms[&code];
        assert_eq!(
            room.players[&player_id].snapshot.cursor,
            room.content.text.chars().count()
        );
    }
}
