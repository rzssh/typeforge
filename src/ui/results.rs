use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::app::App;
use crate::ui::{ACCENT, BORDER, MUTED, SUCCESS, header, shell};

pub fn render(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let Some(result) = &app.last_result else {
        return;
    };
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(5),
        Constraint::Min(6),
    ])
    .split(body);
    let headline = if result.new_personal_best {
        ("★ new personal best", SUCCESS)
    } else {
        ("session complete", ACCENT)
    };
    frame.render_widget(
        Paragraph::new(headline.0)
            .alignment(Alignment::Center)
            .style(Style::default().fg(headline.1).add_modifier(Modifier::BOLD)),
        rows[0],
    );
    let metrics = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(rows[1]);
    let values = [
        (" wpm ", format!("{:.0}", result.wpm)),
        (" accuracy ", format!("{:.1}%", result.accuracy * 100.0)),
        (" mistakes ", result.mistakes.to_string()),
        (" time ", format!("{:.1}s", result.duration_secs)),
    ];
    for (area, (label, value)) in metrics.iter().zip(values) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    value,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
            ])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(Span::styled(label, Style::default().fg(MUTED)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(BORDER)),
            ),
            *area,
        );
    }
    let personal_best = app.stats_store.best_wpm(&result.test_id);
    let recent = app
        .stats_store
        .recent_wpm(&result.test_id, 5)
        .into_iter()
        .map(|wpm| format!("{wpm:.0}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut details = vec![
        Line::from(Span::styled(
            format!("{} · {}", result.language, result.content),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "{} CPM · {} corrections",
            result.cpm.round(),
            result.corrections
        )),
        Line::from(Span::styled(
            format!("personal best · {personal_best:.0} WPM"),
            Style::default().fg(if result.new_personal_best {
                SUCCESS
            } else {
                ACCENT
            }),
        )),
        Line::from(Span::styled(
            format!("recent comparable · {recent}"),
            Style::default().fg(MUTED),
        )),
    ];
    if let Some(source) = &result.source {
        let mut source_lines = source.lines();
        if let Some(source) = source_lines.next() {
            details.push(Line::from(vec![
                Span::styled(
                    "source · ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(source.to_owned()),
            ]));
        }
        details.extend(source_lines.map(|line| Line::from(line.to_owned())));
    }
    frame.render_widget(
        Paragraph::new(details)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Gray))
            .wrap(ratatui::widgets::Wrap { trim: true })
            .block(
                Block::default()
                    .title(" summary ")
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1))
                    .border_style(Style::default().fg(BORDER)),
            ),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("a analysis · Tab next · Enter retry · s stats · Esc menu")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}
