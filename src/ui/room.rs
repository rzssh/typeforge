use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::app::{App, MultiplayerState};
use crate::engine::session::SessionMode;
use crate::multiplayer::client::ConnectionStatus;
use crate::multiplayer::protocol::PlayerSnapshot;
use crate::ui::menu::centered;
use crate::ui::{ACCENT, BORDER, ERROR, MUTED, PANEL, SUCCESS, header, shell};

pub fn render_entry(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let settings = if app.settings.practice == crate::engine::content::PracticeKind::Code {
        format!(
            "{} · {} · {}",
            app.settings.code_language.label(),
            app.settings.mistake_mode.label(),
            if app.settings.code_autopairs {
                "autopairs"
            } else {
                "no autopairs"
            }
        )
    } else {
        format!(
            "{} {} · {}",
            app.settings.word_language.label(),
            app.settings.word_list.label(),
            app.settings.word_mode.label()
        )
    };
    let rows = [
        ("name", app.player_name.clone()),
        (
            "room code",
            if app.room_code_input.is_empty() {
                "______".into()
            } else {
                app.room_code_input.clone()
            },
        ),
        ("", "Join room".into()),
        ("", format!("Create room · {settings}")),
    ];
    let items = rows
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let selected = app.room_entry_selection == index;
            let prefix = if selected { " › " } else { "   " };
            let line = if label.is_empty() {
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(ACCENT)),
                    Span::styled(
                        value,
                        Style::default()
                            .fg(if selected { ACCENT } else { Color::White })
                            .add_modifier(if selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(ACCENT)),
                    Span::styled(format!("{label:<12}"), Style::default().fg(MUTED)),
                    Span::styled(value, Style::default().fg(Color::White)),
                    Span::styled(
                        if selected { "  ▏" } else { "" },
                        Style::default().fg(ACCENT),
                    ),
                ])
            };
            ListItem::new(line).style(if selected {
                Style::default().bg(PANEL)
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(Span::styled(" multiplayer ", Style::default().fg(ACCENT)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        centered(62.min(body.width), 8, body),
    );
    if let Some(status) = &app.status {
        frame.render_widget(
            Paragraph::new(status.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().fg(ERROR)),
            Rect::new(body.x, body.bottom().saturating_sub(2), body.width, 1),
        );
    }
    frame.render_widget(
        Paragraph::new("↑↓ select · type to edit · Enter continue · Esc setup")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

pub fn render_lobby(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let Some(multiplayer) = &app.multiplayer else {
        return;
    };
    let status = connection_label(&multiplayer.connection);
    let Some(room) = &multiplayer.room else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    status,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("Esc to cancel", Style::default().fg(MUTED))),
            ])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" multiplayer ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT)),
            ),
            centered(54, 5, body),
        );
        return;
    };
    let invite = if multiplayer.server_url == "ws://127.0.0.1:8787" {
        format!("typeforge join {}", room.code)
    } else {
        format!(
            "TYPEFORGE_SERVER='{}' typeforge join {}",
            multiplayer.server_url, room.code
        )
    };
    let areas = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .split(body);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("room {}", room.code),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  ·  {status}"), Style::default().fg(MUTED)),
            ]),
            Line::from(Span::styled(
                format!("invite · {invite}"),
                Style::default().fg(Color::White),
            )),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        areas[0],
    );
    render_players(frame, multiplayer, areas[1]);
    if let Some(status) = &app.status {
        frame.render_widget(
            Paragraph::new(status.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().fg(ERROR)),
            areas[2],
        );
    } else {
        let ready = room.players.iter().filter(|player| player.ready).count();
        frame.render_widget(
            Paragraph::new(format!(
                "{ready}/{} ready · host may start anytime",
                room.players.len()
            ))
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
            areas[2],
        );
    }
    let is_host = multiplayer
        .player_id
        .as_ref()
        .is_some_and(|player_id| player_id == &room.host_id);
    let controls = if is_host {
        "r ready · Enter start countdown · Esc leave"
    } else {
        "r ready · waiting for host · Esc leave"
    };
    frame.render_widget(
        Paragraph::new(controls)
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

pub fn render_countdown(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let remaining = app.countdown_remaining_ms().unwrap_or(0);
    let number = remaining.div_ceil(1_000).max(1);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                number.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled("get ready", Style::default().fg(MUTED))),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .title(" race starts in ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        ),
        centered(34, 6, body),
    );
    frame.render_widget(
        Paragraph::new("Input unlocks when the countdown reaches zero")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

pub fn render_results(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let Some(multiplayer) = &app.multiplayer else {
        return;
    };
    let Some(room) = &multiplayer.room else {
        return;
    };
    let mut players = room.players.clone();
    match room.mode {
        SessionMode::Timed(_) => players.sort_by(|left, right| {
            right
                .finished
                .cmp(&left.finished)
                .then_with(|| right.wpm.total_cmp(&left.wpm))
                .then_with(|| right.accuracy.total_cmp(&left.accuracy))
        }),
        SessionMode::WordCount(_) | SessionMode::Snippet => players.sort_by_key(|player| {
            (
                player.elapsed_ms.is_none(),
                player.elapsed_ms.unwrap_or(u64::MAX),
            )
        }),
    }
    let local_rank = multiplayer.player_id.as_ref().and_then(|player_id| {
        players
            .iter()
            .position(|player| &player.id == player_id)
            .map(|rank| rank + 1)
    });
    let title = match local_rank {
        Some(1) => "★ victory".to_string(),
        Some(rank) => format!("finished #{rank}"),
        None => "race results".into(),
    };
    let areas = Layout::vertical([Constraint::Length(2), Constraint::Min(8)]).split(body);
    frame.render_widget(
        Paragraph::new(title)
            .alignment(Alignment::Center)
            .style(Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)),
        areas[0],
    );
    let items = players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            let color = player_color(index);
            let result = match player.elapsed_ms {
                Some(elapsed) => format!(
                    "{:.1}s · {:.0} WPM · {:.1}% · {} errors",
                    elapsed as f64 / 1_000.0,
                    player.wpm,
                    player.accuracy * 100.0,
                    player.mistakes
                ),
                None if player.connected => format!(
                    "{:.0}% · typing",
                    progress(player, room.content.text.chars().count())
                ),
                None => "disconnected".into(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {:>2}. ● ", index + 1), Style::default().fg(color)),
                Span::styled(
                    format!("{:<16}", player.name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(result, Style::default().fg(MUTED)),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(Span::styled(
                    format!(" room {} ", room.code),
                    Style::default().fg(ACCENT),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        areas[1],
    );
    frame.render_widget(
        Paragraph::new("Esc leave room")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

fn render_players(frame: &mut Frame, multiplayer: &MultiplayerState, area: Rect) {
    let Some(room) = &multiplayer.room else {
        return;
    };
    let items = room
        .players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            let host = if player.id == room.host_id {
                " host"
            } else {
                ""
            };
            let status = if !player.connected {
                "reconnecting"
            } else if player.ready {
                "ready"
            } else {
                "not ready"
            };
            ListItem::new(Line::from(vec![
                Span::styled(" ● ", Style::default().fg(player_color(index))),
                Span::styled(
                    format!("{:<16}", player.name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    status,
                    Style::default().fg(if player.ready { SUCCESS } else { MUTED }),
                ),
                Span::styled(host, Style::default().fg(ACCENT)),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(Span::styled(" racers ", Style::default().fg(ACCENT)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        area,
    );
}

fn connection_label(status: &ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Connecting => "connecting…",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Reconnecting => "reconnecting…",
    }
}

fn progress(player: &PlayerSnapshot, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        player.cursor as f64 / total as f64 * 100.0
    }
}

pub fn player_color(index: usize) -> Color {
    const COLORS: [Color; 8] = [
        Color::Rgb(196, 158, 255),
        Color::Rgb(90, 200, 250),
        Color::Rgb(255, 170, 80),
        Color::Rgb(245, 110, 160),
        Color::Rgb(105, 210, 145),
        Color::Rgb(235, 220, 95),
        Color::Rgb(130, 160, 255),
        Color::Rgb(220, 130, 235),
    ];
    COLORS[index % COLORS.len()]
}
