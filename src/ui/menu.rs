use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::app::{App, AppState};
use crate::engine::content::PracticeKind;
use crate::ui::{ACCENT, BORDER, ERROR, MUTED, PANEL, header, shell};

pub fn render(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let rows = match app.settings.practice {
        PracticeKind::Words => vec![
            ("practice", app.settings.practice.label().into()),
            ("language", app.settings.word_language.label().into()),
            (
                "wordlist",
                format!("Monkeytype {}", app.settings.word_list.label()),
            ),
            ("word length", app.settings.word_length.label().into()),
            ("session", app.settings.word_mode.label()),
            ("mistakes", app.settings.mistake_mode.label().into()),
            (
                "lowercase",
                if app.settings.lowercase_words {
                    "On"
                } else {
                    "Off"
                }
                .into(),
            ),
            ("caret", app.settings.caret_style.label().into()),
            ("", "Start session".into()),
        ],
        PracticeKind::Code => vec![
            ("practice", app.settings.practice.label().into()),
            ("language", app.settings.code_language.label().into()),
            ("mistakes", app.settings.mistake_mode.label().into()),
            (
                "autopairs",
                if app.settings.code_autopairs {
                    "On · () [] {}"
                } else {
                    "Off"
                }
                .into(),
            ),
            ("caret", app.settings.caret_style.label().into()),
            ("", "Start session".into()),
        ],
    };
    let items: Vec<ListItem> = rows
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let selected = index == app.menu_selection;
            let prefix = Span::styled(
                if selected { " › " } else { "   " },
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            );
            let line = if label.is_empty() {
                Line::from(vec![
                    prefix,
                    Span::styled(
                        if selected {
                            format!("{value}  ↵")
                        } else {
                            value
                        },
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    prefix,
                    Span::styled(format!("{label:<14}"), Style::default().fg(MUTED)),
                    Span::styled("‹ ", Style::default().fg(BORDER)),
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
                    Span::styled(" ›", Style::default().fg(BORDER)),
                ])
            };
            ListItem::new(line).style(if selected {
                Style::default().bg(PANEL)
            } else {
                Style::default()
            })
        })
        .collect();
    let width = 62.min(body.width);
    let height = (items.len() as u16 + 2).min(body.height);
    let area = centered(width, height, body);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(Span::styled(
                    " practice setup ",
                    Style::default().fg(ACCENT),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        area,
    );
    if app.state == AppState::Loading {
        let message = if app.settings.practice == PracticeKind::Code {
            "Finding real code…"
        } else {
            "Loading word list…"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    message,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("Esc to cancel", Style::default().fg(MUTED))),
            ])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" loading ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT)),
            ),
            centered(58, 5, body),
        );
    } else if let Some(status) = &app.status {
        let status_area = Rect::new(body.x, body.bottom().saturating_sub(4), body.width, 4);
        frame.render_widget(
            Paragraph::new(status.as_str())
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .style(Style::default().fg(ERROR))
                .block(
                    Block::default()
                        .title(" error ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(ERROR)),
                ),
            status_area,
        );
    }
    let controls = if app.state == AppState::Loading {
        "Esc cancel"
    } else if app.settings.practice == PracticeKind::Code {
        "↑↓/jk select · ←→/hl adjust · Enter solo · m multiplayer · r refresh · s stats · q quit"
    } else {
        "↑↓/jk select · ←→/hl adjust · Enter solo · m multiplayer · s stats · q quit"
    };
    frame.render_widget(
        Paragraph::new(controls)
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

pub fn centered(width: u16, height: u16, area: Rect) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
