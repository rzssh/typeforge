use std::thread;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use super::protocol::{ClientMessage, ServerMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone)]
pub enum NetworkEvent {
    Status(ConnectionStatus),
    Message(Box<ServerMessage>),
}

pub struct NetworkClient {
    commands: UnboundedSender<ClientMessage>,
    events: UnboundedReceiver<NetworkEvent>,
}

impl NetworkClient {
    pub fn connect(server_url: String, initial: ClientMessage) -> Self {
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (event_sender, events) = mpsc::unbounded_channel();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run(server_url, initial, command_receiver, event_sender))
                }
                Err(error) => {
                    let _ =
                        event_sender.send(NetworkEvent::Message(Box::new(ServerMessage::Error {
                            message: format!("Could not start network runtime: {error}"),
                            fatal: true,
                        })));
                }
            }
        });
        Self { commands, events }
    }

    pub fn send(&self, message: ClientMessage) {
        let _ = self.commands.send(message);
    }

    pub fn try_recv(&mut self) -> Option<NetworkEvent> {
        self.events.try_recv().ok()
    }
}

async fn run(
    server_url: String,
    initial: ClientMessage,
    mut commands: UnboundedReceiver<ClientMessage>,
    events: UnboundedSender<NetworkEvent>,
) {
    let mut hello = initial;
    let mut connected_once = false;
    let mut retry_delay = 1;
    loop {
        if commands.is_closed() {
            return;
        }
        let status = if connected_once {
            ConnectionStatus::Reconnecting
        } else {
            ConnectionStatus::Connecting
        };
        let _ = events.send(NetworkEvent::Status(status));
        let config = WebSocketConfig::default().max_message_size(Some(128 * 1024));
        let connection = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async_with_config(&server_url, Some(config), true),
        )
        .await;
        let (mut socket, _) = match connection {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                let _ = events.send(NetworkEvent::Message(Box::new(ServerMessage::Error {
                    message: format!("Could not connect: {error}"),
                    fatal: false,
                })));
                tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                retry_delay = (retry_delay * 2).min(5);
                continue;
            }
            Err(_) => {
                let _ = events.send(NetworkEvent::Message(Box::new(ServerMessage::Error {
                    message: "Connection timed out".into(),
                    fatal: false,
                })));
                continue;
            }
        };
        let Ok(payload) = serde_json::to_string(&hello) else {
            return;
        };
        if socket.send(Message::Text(payload.into())).await.is_err() {
            continue;
        }
        connected_once = true;
        retry_delay = 1;
        let _ = events.send(NetworkEvent::Status(ConnectionStatus::Connected));
        let mut reconnect = true;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else {
                        let _ = socket.close(None).await;
                        return;
                    };
                    let Ok(payload) = serde_json::to_string(&command) else {
                        continue;
                    };
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                frame = socket.next() => {
                    let Some(Ok(frame)) = frame else {
                        break;
                    };
                    let Message::Text(text) = frame else {
                        continue;
                    };
                    let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else {
                        continue;
                    };
                    if let ServerMessage::Welcome {
                        player_id,
                        resume_token,
                        room,
                    } = &message
                    {
                        let name = room
                            .players
                            .iter()
                            .find(|player| &player.id == player_id)
                            .map(|player| player.name.clone())
                            .unwrap_or_default();
                        hello = ClientMessage::Join {
                            code: room.code.clone(),
                            name,
                            resume_token: Some(resume_token.clone()),
                        };
                    }
                    if matches!(&message, ServerMessage::Error { fatal: true, .. }) {
                        reconnect = false;
                    }
                    let _ = events.send(NetworkEvent::Message(Box::new(message)));
                    if !reconnect {
                        return;
                    }
                }
            }
        }
        if !reconnect {
            return;
        }
        tokio::time::sleep(Duration::from_secs(retry_delay)).await;
    }
}
