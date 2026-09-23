use std::collections::BTreeMap;

use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph,
};

use crate::app::App;
use crate::engine::session::{KeystrokeEvent, ProgressEvent};
use crate::ui::{ACCENT, ACCENT_DARK, BORDER, ERROR, MUTED, PANEL, SUCCESS, header, shell};

struct LineAnalysis<'a> {
    number: usize,
    text: &'a str,
    start_position: usize,
    start_ms: u64,
    end_ms: u64,
    correct: usize,
    mistakes: Vec<&'a KeystrokeEvent>,
}

impl LineAnalysis<'_> {
    fn wpm(&self) -> f64 {
        let seconds = self.end_ms.saturating_sub(self.start_ms) as f64 / 1_000.0;
        self.correct as f64 / 5.0 / (seconds.max(0.1) / 60.0)
    }
}

pub fn render(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let Some(result) = &app.last_result else {
        return;
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{:.0} WPM · {:.1}% acc · {} errors · {} fixes · {:.1}s",
            result.wpm,
            result.accuracy * 100.0,
            result.mistakes,
            result.corrections,
            result.duration_secs
        ))
        .style(Style::default().fg(Color::Gray)),
        Rect::new(top.x, top.y + 1, top.width, 1),
    );
    let lines = line_analyses(
        &result.analysis.text,
        &result.analysis.keystrokes,
        &result.analysis.correct_positions,
        result.analysis.final_cursor,
        result.duration_secs,
    );
    let selected_index = app.analysis_line.min(lines.len().saturating_sub(1));
    let selected = &lines[selected_index];
    let areas = Layout::vertical([Constraint::Length(8), Constraint::Min(4)]).split(body);
    let speed = cumulative_points(&result.analysis.progress, result.duration_secs);
    let mistakes: Vec<_> = result
        .analysis
        .keystrokes
        .iter()
        .filter(|event| event.typed != event.expected)
        .collect();
    let error_points: Vec<_> = mistakes
        .iter()
        .map(|event| {
            let seconds = event.elapsed_ms as f64 / 1_000.0;
            (
                seconds,
                cumulative_wpm_at(&result.analysis.progress, seconds),
            )
        })
        .collect();
    let selected_speed = selected_points(&result.analysis.progress, selected);
    let duration = result.duration_secs.max(1.0);
    let maximum_wpm = speed
        .iter()
        .map(|(_, wpm)| *wpm)
        .fold(result.wpm, f64::max)
        .max(10.0);
    let ceiling = (maximum_wpm / 10.0).ceil() * 10.0;
    let mut datasets = vec![
        Dataset::default()
            .name("cumulative WPM")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(ACCENT_DARK))
            .data(&speed),
        Dataset::default()
            .name("selected line")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            .data(&selected_speed),
    ];
    if !error_points.is_empty() {
        datasets.push(
            Dataset::default()
                .name("mistakes")
                .marker(Marker::Custom('×'))
                .graph_type(GraphType::Scatter)
                .style(Style::default().fg(ERROR).add_modifier(Modifier::BOLD))
                .data(&error_points),
        );
    }
    let start = selected.start_ms as f64 / 1_000.0;
    let end = selected.end_ms as f64 / 1_000.0;
    frame.render_widget(
        Chart::new(datasets)
            .block(
                Block::default()
                    .title(format!(
                        " speed · line {} · {start:.1}–{end:.1}s · {:.0} WPM ",
                        selected.number,
                        selected.wpm()
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(BORDER)),
            )
            .x_axis(
                Axis::default()
                    .style(Style::default().fg(MUTED))
                    .bounds([0.0, duration])
                    .labels(vec![
                        Line::from("0s"),
                        Line::from(format!("{duration:.1}s")),
                    ]),
            )
            .y_axis(
                Axis::default()
                    .style(Style::default().fg(MUTED))
                    .bounds([0.0, ceiling])
                    .labels(vec![Line::from("0"), Line::from(format!("{ceiling:.0}"))]),
            ),
        areas[0],
    );
    let visible = areas[1].height.saturating_sub(2) as usize;
    let first = selected_index
        .saturating_sub(visible / 2)
        .min(lines.len().saturating_sub(visible));
    let items = lines
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, line)| line_item(line, index == selected_index))
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(
                    " lines · {}/{} · {} mistakes ",
                    selected_index + 1,
                    lines.len(),
                    mistakes.len()
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        areas[1],
    );
    frame.render_widget(
        Paragraph::new("↑↓ select line · highlighted on graph · Esc results")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

fn line_analyses<'a>(
    text: &'a str,
    events: &'a [KeystrokeEvent],
    correct_positions: &[bool],
    final_cursor: usize,
    duration_secs: f64,
) -> Vec<LineAnalysis<'a>> {
    let source_lines: Vec<_> = text.split('\n').collect();
    let active_position = final_cursor.min(text.chars().count().saturating_sub(1));
    let duration_ms = (duration_secs * 1_000.0) as u64;
    let mut offset = 0;
    let mut previous_end = 0;
    source_lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let has_newline = index + 1 < source_lines.len();
            let end = offset + text.chars().count() + usize::from(has_newline);
            let line_events: Vec<_> = events
                .iter()
                .filter(|event| event.position >= offset && event.position < end)
                .collect();
            let mut end_ms = line_events
                .iter()
                .map(|event| event.elapsed_ms)
                .max()
                .unwrap_or(previous_end)
                .max(previous_end);
            if active_position >= offset && active_position < end {
                end_ms = end_ms.max(duration_ms);
            }
            let line = LineAnalysis {
                number: index + 1,
                text,
                start_position: offset,
                start_ms: previous_end,
                end_ms,
                correct: correct_positions
                    .get(offset..end)
                    .unwrap_or_default()
                    .iter()
                    .filter(|correct| **correct)
                    .count(),
                mistakes: line_events
                    .into_iter()
                    .filter(|event| event.typed != event.expected)
                    .collect(),
            };
            offset = end;
            previous_end = end_ms;
            line
        })
        .collect()
}

fn line_item<'a>(line: &LineAnalysis<'a>, selected: bool) -> ListItem<'a> {
    let mut spans = vec![
        Span::styled(
            if selected { " › " } else { "   " },
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{:>3}  {:>3.0} WPM  {:>2}×  ",
                line.number,
                line.wpm(),
                line.mistakes.len()
            ),
            Style::default().fg(if selected { SUCCESS } else { MUTED }),
        ),
    ];
    let mut mistakes = BTreeMap::<usize, Vec<char>>::new();
    for event in &line.mistakes {
        mistakes
            .entry(event.position)
            .or_default()
            .push(event.typed);
    }
    for (offset, character) in line.text.chars().enumerate() {
        let position = line.start_position + offset;
        if let Some(typed) = mistakes.get(&position) {
            spans.push(Span::styled(
                format_error(character, typed),
                Style::default()
                    .fg(Color::White)
                    .bg(ERROR)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                character.to_string(),
                Style::default().fg(Color::Gray),
            ));
        }
    }
    let newline_position = line.start_position + line.text.chars().count();
    if let Some(typed) = mistakes.get(&newline_position) {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format_error('\n', typed),
            Style::default()
                .fg(Color::White)
                .bg(ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line.text.is_empty() && !mistakes.contains_key(&newline_position) {
        spans.push(Span::styled("↵", Style::default().fg(MUTED)));
    }
    ListItem::new(Line::from(spans)).style(if selected {
        Style::default().bg(PANEL)
    } else {
        Style::default()
    })
}

fn format_error(expected: char, typed: &[char]) -> String {
    format!(
        "{}→{}",
        visible(expected),
        typed
            .iter()
            .map(|character| visible(*character))
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn cumulative_points(events: &[ProgressEvent], duration_secs: f64) -> Vec<(f64, f64)> {
    let duration = duration_secs.max(0.1);
    let mut points = vec![(0.0, 0.0)];
    for second in 1..=duration.ceil() as usize {
        let time = (second as f64).min(duration);
        points.push((time, cumulative_wpm_at(events, time)));
    }
    points
}

fn selected_points(events: &[ProgressEvent], line: &LineAnalysis<'_>) -> Vec<(f64, f64)> {
    let start = line.start_ms as f64 / 1_000.0;
    let end = line.end_ms as f64 / 1_000.0;
    let mut points = vec![(start, cumulative_wpm_at(events, start))];
    for second in start.ceil() as usize..=end.floor() as usize {
        let time = second as f64;
        if time > start && time < end {
            points.push((time, cumulative_wpm_at(events, time)));
        }
    }
    if end > start {
        points.push((end, cumulative_wpm_at(events, end)));
    }
    points
}

fn cumulative_wpm_at(events: &[ProgressEvent], seconds: f64) -> f64 {
    let end_ms = (seconds * 1_000.0) as u64;
    let correct = events
        .iter()
        .rev()
        .find(|event| event.elapsed_ms <= end_ms)
        .map(|event| event.correct_chars)
        .unwrap_or(0);
    correct as f64 / 5.0 / (seconds.max(0.1) / 60.0)
}

fn visible(character: char) -> String {
    match character {
        '\n' => "↵".into(),
        '\t' => "⇥".into(),
        ' ' => "·".into(),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::app::{AppState, SessionAnalysis, SessionResult};
    use ratatui::{Terminal, backend::TestBackend};

    fn event_at(elapsed_ms: u64, position: usize, expected: char, typed: char) -> KeystrokeEvent {
        KeystrokeEvent {
            elapsed_ms,
            position,
            expected,
            typed,
            accepted: expected == typed,
        }
    }

    fn event(elapsed_ms: u64, accepted: bool) -> KeystrokeEvent {
        event_at(elapsed_ms, 0, 'a', if accepted { 'a' } else { 'x' })
    }

    fn progress(elapsed_ms: u64, correct_chars: u32) -> ProgressEvent {
        ProgressEvent {
            elapsed_ms,
            correct_chars,
        }
    }

    #[test]
    fn cumulative_wpm_uses_the_whole_session() {
        let events = vec![progress(1_000, 1), progress(6_000, 2)];
        assert!((cumulative_wpm_at(&events, 6.0) - 4.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn cumulative_wpm_reflects_deleted_correct_characters() {
        let events = vec![progress(0, 1), progress(500, 0), progress(1_000, 1)];
        assert_eq!(cumulative_wpm_at(&events, 0.75), 0.0);
        assert!((cumulative_wpm_at(&events, 1.0) - 12.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn speed_graph_includes_the_session_end() {
        let points = cumulative_points(&[progress(500, 1)], 1.5);
        assert_eq!(points.last().unwrap().0, 1.5);
    }

    #[test]
    fn lines_combine_timing_and_mistakes() {
        let events = vec![
            event_at(0, 0, 'a', 'a'),
            event_at(500, 1, 'b', 'b'),
            event_at(1_000, 2, '\n', '\n'),
            event_at(1_500, 3, 'c', 'x'),
            event_at(1_800, 3, 'c', 'c'),
            event_at(2_000, 4, 'd', 'd'),
        ];
        let lines = line_analyses("ab\ncd", &events, &[true; 5], 5, 3.0);
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (0, 1_000));
        assert_eq!((lines[1].start_ms, lines[1].end_ms), (1_000, 3_000));
        assert_eq!(lines[1].mistakes.len(), 1);
        assert!((lines[1].wpm() - 12.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn analysis_renders_at_the_minimum_terminal_size() {
        let mut app = App::new();
        app.state = AppState::Analysis;
        app.last_result = Some(SessionResult {
            content: "snippet".into(),
            language: "Rust".into(),
            test_id: "code:rust:strict".into(),
            new_personal_best: false,
            wpm: 60.0,
            cpm: 300.0,
            accuracy: 0.9,
            duration_secs: 10.0,
            total_keystrokes: 10,
            correct_keystrokes: 9,
            mistakes: 1,
            corrections: 1,
            source: None,
            snippet_id: None,
            repository: None,
            confusion: HashMap::new(),
            character_stats: HashMap::new(),
            analysis: SessionAnalysis {
                text: "abc\ndef".into(),
                keystrokes: vec![event(500, false)],
                progress: vec![progress(500, 0)],
                correct_positions: vec![false; 7],
                final_cursor: 1,
            },
        });
        let backend = TestBackend::new(64, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &app))
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("60 WPM · 90.0% acc · 1 errors · 1 fixes · 10.0s"));
        assert!(content.contains("speed · line 1"));
        assert!(content.contains("lines · 1/2"));
        assert!(content.contains("a→x"));
    }
}
