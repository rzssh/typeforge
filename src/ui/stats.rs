use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::app::App;
use crate::ui::{ACCENT, BORDER, MUTED, header, shell};

pub fn render(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let store = &app.stats_store;
    let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(2)
        .split(body);
    let summary = vec![
        metric("sessions", store.sessions.len()),
        metric("average WPM", format!("{:.1}", store.average_wpm())),
        metric(
            "accuracy",
            format!("{:.1}%", store.average_accuracy() * 100.0),
        ),
        metric(
            "practice time",
            format!("{:.1} min", store.total_time() / 60.0),
        ),
        metric("code seen", store.seen_snippets.len()),
    ];
    frame.render_widget(panel(" overview ", summary), chunks[0]);
    let weak = store.weakest_characters();
    let confusions = store.top_confusions();
    let mut diagnostics = vec![Line::from(Span::styled(
        "weak characters",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))];
    if weak.is_empty() {
        diagnostics.push(Line::from(Span::styled(
            "Complete more sessions to rank them",
            Style::default().fg(MUTED),
        )));
    } else {
        diagnostics.extend(weak.into_iter().map(|(character, accuracy, ms)| {
            Line::from(format!(
                "{}  {:>5.1}%  {:>5.0}ms",
                visible(character),
                accuracy * 100.0,
                ms
            ))
        }));
    }
    diagnostics.push(Line::from(""));
    diagnostics.push(Line::from(Span::styled(
        "common confusions",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    if confusions.is_empty() {
        diagnostics.push(Line::from(Span::styled(
            "No repeated confusions yet",
            Style::default().fg(MUTED),
        )));
    } else {
        diagnostics.extend(
            confusions
                .into_iter()
                .map(|(pair, count)| Line::from(format!("{pair:<16} {count}"))),
        );
    }
    frame.render_widget(panel(" diagnostics ", diagnostics), chunks[1]);
    frame.render_widget(
        Paragraph::new("Esc menu")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

fn metric(label: &str, value: impl ToString) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<15}"), Style::default().fg(MUTED)),
        Span::styled(
            value.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn panel(title: &'static str, lines: Vec<Line<'static>>) -> Paragraph<'static> {
    Paragraph::new(lines).block(
        Block::default()
            .title(Span::styled(title, Style::default().fg(ACCENT)))
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .border_style(Style::default().fg(BORDER)),
    )
}

fn visible(character: char) -> String {
    match character {
        ' ' => "space".into(),
        '\n' => "enter".into(),
        '\t' => "tab".into(),
        value => value.to_string(),
    }
}
