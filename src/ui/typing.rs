use std::sync::{Mutex, OnceLock};

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, Padding, Paragraph};
use two_face::re_exports::syntect::easy::HighlightLines;
use two_face::re_exports::syntect::highlighting::Theme;
use two_face::re_exports::syntect::parsing::SyntaxSet;
use two_face::re_exports::syntect::util::LinesWithEndings;

use crate::app::App;
use crate::engine::content::ContentSource;
use crate::engine::session::{CharResult, SessionMode};
use crate::ui::{ACCENT, ACCENT_DARK, BORDER, ERROR, MUTED, PANEL, SUCCESS, header, shell};

type ColorCache = Mutex<Option<(String, Vec<Color>)>>;

pub fn render(frame: &mut Frame, app: &App) {
    let (top, body, footer) = shell(frame);
    frame.render_widget(header(), top);
    let Some(session) = &app.session else {
        return;
    };
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(body);
    let label = match &session.content.source {
        ContentSource::Words { language, size } => {
            format!(
                "{} {} · {}",
                language.label(),
                size.label(),
                session.mode.label()
            )
        }
        ContentSource::Code(snippet) => format!(
            "{} · {} · {} · {}",
            snippet.language.label(),
            snippet.kind,
            snippet.repository,
            snippet.license
        ),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(label, Style::default().fg(Color::Gray))),
            Line::from(vec![
                Span::styled(
                    format!("[{}]", if app.show_typed { "typed" } else { "expected" }),
                    Style::default().fg(MUTED),
                ),
                Span::styled(
                    format!(
                        "  {:.0} WPM · {:.1}% accuracy · {} errors",
                        session.wpm(),
                        session.accuracy() * 100.0,
                        session.mistakes
                    ),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
            ]),
        ]),
        layout[0],
    );
    let base = syntax_colors(session);
    let (lines, visible_chars, visible_cursor) = if app.show_typed {
        let typed = typed_buffer(session);
        let cursor = typed.len();
        let chars = typed.iter().map(|(character, _)| *character).collect();
        (typed_lines(&typed), chars, cursor)
    } else {
        (
            expected_lines(session, &base),
            session.chars.clone(),
            session.cursor,
        )
    };
    if matches!(session.content.source, ContentSource::Words { .. }) {
        render_words(frame, lines, &visible_chars, visible_cursor, layout[1]);
    } else {
        render_code(frame, lines, &visible_chars, visible_cursor, layout[1]);
    }
    let (meter_ratio, meter_label) = meter(session);
    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .title(Span::styled(" progress ", Style::default().fg(MUTED)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(BORDER)),
            )
            .ratio(meter_ratio)
            .label(Span::styled(
                meter_label,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .gauge_style(Style::default().fg(ACCENT_DARK).bg(PANEL))
            .use_unicode(true),
        layout[2],
    );
    let mut footer_lines = Vec::new();
    if let Some((expected, typed)) = session.latest_error() {
        footer_lines.push(Line::from(Span::styled(
            format!(
                "typed {} · expected {} · backspace to correct",
                visible_mistake(typed),
                visible_mistake(expected)
            ),
            Style::default().fg(ERROR),
        )));
    }
    let controls = if footer.width < 90 {
        "F2 view · ^R restart · Tab next · ^W word · ^U line · Esc menu".into()
    } else {
        format!(
            "F2 view · Ctrl-R/F3 restart · Tab next · Ctrl-W delete word · Ctrl-U delete line · {} mistakes · Esc menu",
            session.mistake_mode.label()
        )
    };
    footer_lines.push(Line::from(controls));
    frame.render_widget(
        Paragraph::new(footer_lines)
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

fn render_words(
    frame: &mut Frame,
    lines: Vec<Line<'_>>,
    chars: &[char],
    cursor: usize,
    area: Rect,
) {
    let height = 7.min(area.height);
    let area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    let block = Block::default()
        .title(Span::styled(" words ", Style::default().fg(MUTED)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::uniform(1));
    let inner = block.inner(area);
    if inner.is_empty() {
        return;
    }
    let (lines, row, column) = wrap_words(lines, chars, cursor, inner.width as usize);
    let vertical = row.saturating_sub(inner.height.saturating_sub(2) as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((saturating_u16(vertical), 0)),
        area,
    );
    frame.set_cursor_position((
        inner.x + saturating_u16(column),
        inner.y + saturating_u16(row.saturating_sub(vertical)),
    ));
}

fn wrap_words(
    lines: Vec<Line<'_>>,
    chars: &[char],
    cursor: usize,
    width: usize,
) -> (Vec<Line<'static>>, usize, usize) {
    let width = width.max(1);
    let mut cells = lines
        .into_iter()
        .flat_map(|line| line.spans)
        .flat_map(|span| {
            let style = span.style;
            span.content
                .chars()
                .map(move |character| Span::styled(character.to_string(), style))
                .collect::<Vec<_>>()
        });
    let mut output = vec![Line::default()];
    let mut row = 0;
    let mut column = 0;
    let mut cursor_position = None;
    for (index, character) in chars.iter().enumerate() {
        let starts_word = index == 0 || chars[index - 1].is_whitespace();
        if starts_word && column > 0 {
            let word_length = chars[index..]
                .iter()
                .take_while(|character| !character.is_whitespace())
                .count();
            if word_length <= width && column + word_length > width {
                row += 1;
                column = 0;
                output.push(Line::default());
            }
        }
        if index == cursor {
            cursor_position = Some((row, column));
        }
        let cell = cells
            .next()
            .unwrap_or_else(|| Span::raw(character.to_string()));
        if character.is_whitespace() && column == 0 && index > 0 {
            continue;
        }
        output[row].spans.push(cell);
        column += 1;
        if column == width {
            row += 1;
            column = 0;
            output.push(Line::default());
        }
    }
    let (cursor_row, cursor_column) = cursor_position.unwrap_or((row, column));
    (output, cursor_row, cursor_column)
}

fn render_code(frame: &mut Frame, lines: Vec<Line<'_>>, chars: &[char], cursor: usize, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::uniform(1));
    let inner = block.inner(area);
    if inner.is_empty() {
        return;
    }
    let (row, column) = line_cursor(chars, cursor);
    let vertical = row.saturating_sub(inner.height.saturating_sub(3) as usize);
    let horizontal = column.saturating_sub(inner.width.saturating_sub(3) as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((saturating_u16(vertical), saturating_u16(horizontal))),
        area,
    );
    frame.set_cursor_position((
        inner.x + saturating_u16(column.saturating_sub(horizontal)),
        inner.y + saturating_u16(row.saturating_sub(vertical)),
    ));
}

fn line_cursor(chars: &[char], cursor: usize) -> (usize, usize) {
    let mut row = 0;
    let mut column = 0;
    for character in chars.iter().take(cursor) {
        if *character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (row, column)
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn meter(session: &crate::engine::session::Session) -> (f64, String) {
    match session.mode {
        SessionMode::Timed(seconds) => {
            let elapsed = session.elapsed_secs();
            let remaining = (seconds as f64 - elapsed).max(0.0);
            let ratio = (elapsed / seconds as f64).clamp(0.0, 1.0);
            (
                ratio,
                format!("{:.0}% · {remaining:.1}s left", ratio * 100.0),
            )
        }
        SessionMode::WordCount(total) => {
            let completed = session.chars[..session.cursor]
                .iter()
                .filter(|character| **character == ' ')
                .count() as u32
                + u32::from(session.is_complete() && !session.chars.is_empty());
            let ratio = (completed as f64 / total as f64).clamp(0.0, 1.0);
            (
                ratio,
                format!("{completed} / {total} words · {:.0}%", ratio * 100.0),
            )
        }
        SessionMode::Snippet => {
            let ratio = session.progress().clamp(0.0, 1.0);
            (
                ratio,
                format!(
                    "{:.0}% · {} / {} chars",
                    ratio * 100.0,
                    session.cursor,
                    session.chars.len()
                ),
            )
        }
    }
}

fn expected_lines<'a>(session: &crate::engine::session::Session, base: &[Color]) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    for (index, character) in session.chars.iter().enumerate() {
        if *character == '\n' {
            if let CharResult::Incorrect(_) = session.results[index] {
                lines
                    .last_mut()
                    .unwrap()
                    .spans
                    .push(Span::styled("↵", mistake_style()));
            }
            lines.push(Line::default());
            continue;
        }
        let style = match session.results[index] {
            CharResult::Correct | CharResult::Skipped => Style::default().fg(SUCCESS),
            CharResult::Incorrect(_) => mistake_style(),
            CharResult::Pending if session.is_auto_paired(index) => Style::default().fg(SUCCESS),
            CharResult::Pending => Style::default().fg(base[index]),
        };
        lines
            .last_mut()
            .unwrap()
            .spans
            .push(Span::styled(character.to_string(), style));
    }
    lines
}

fn typed_buffer(session: &crate::engine::session::Session) -> Vec<(char, bool)> {
    session
        .results
        .iter()
        .zip(&session.chars)
        .take(session.cursor)
        .filter_map(|(result, expected)| match result {
            CharResult::Correct | CharResult::Skipped => Some((*expected, false)),
            CharResult::Incorrect(typed) => Some((*typed, true)),
            CharResult::Pending => None,
        })
        .collect()
}

fn typed_lines(typed: &[(char, bool)]) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    for (character, mistake) in typed {
        if *character == '\n' {
            if *mistake {
                lines
                    .last_mut()
                    .unwrap()
                    .spans
                    .push(Span::styled("↵", mistake_style()));
            }
            lines.push(Line::default());
            continue;
        }
        let displayed = if *character == '\t' {
            "⇥".into()
        } else {
            character.to_string()
        };
        let style = if *mistake {
            mistake_style()
        } else {
            Style::default().fg(SUCCESS)
        };
        lines
            .last_mut()
            .unwrap()
            .spans
            .push(Span::styled(displayed, style));
    }
    lines
}

fn mistake_style() -> Style {
    Style::default()
        .fg(Color::White)
        .bg(Color::Rgb(160, 48, 60))
        .add_modifier(Modifier::BOLD)
}

fn syntax_colors(session: &crate::engine::session::Session) -> Vec<Color> {
    let default = Color::Rgb(190, 190, 200);
    let ContentSource::Code(snippet) = &session.content.source else {
        return vec![default; session.chars.len()];
    };
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME: OnceLock<Theme> = OnceLock::new();
    static COLORS: OnceLock<ColorCache> = OnceLock::new();
    let cache = COLORS.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock()
        && let Some((text, colors)) = guard.as_ref()
        && text == &session.content.text
    {
        return colors.clone();
    }
    let syntaxes = SYNTAXES.get_or_init(two_face::syntax::extra_newlines);
    let Some(syntax) = syntaxes.find_syntax_by_extension(snippet.language.extensions()[0]) else {
        return vec![default; session.chars.len()];
    };
    let theme = THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::Nord)
            .clone()
    });
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut colors = Vec::with_capacity(session.chars.len());
    for line in LinesWithEndings::from(&session.content.text) {
        let Ok(ranges) = highlighter.highlight_line(line, syntaxes) else {
            colors.extend(line.chars().map(|_| default));
            continue;
        };
        for (style, text) in ranges {
            let color = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            colors.extend(text.chars().map(|_| color));
        }
    }
    colors.resize(session.chars.len(), default);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((session.content.text.clone(), colors.clone()));
    }
    colors
}

fn visible_mistake(character: char) -> String {
    match character {
        '\n' => "↵".into(),
        '\t' => "⇥".into(),
        ' ' => "·".into(),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        expected_lines, line_cursor, meter, render_code, render_words, typed_buffer, wrap_words,
    };
    use crate::engine::content::{
        ContentSource, MistakeMode, TypingContent, WordLanguage, WordListSize,
    };
    use crate::engine::session::{Session, SessionMode};
    use crate::ui::SUCCESS;
    use ratatui::prelude::{Backend, Color, Line, Position, Terminal};

    #[test]
    fn line_cursor_tracks_source_lines() {
        assert_eq!(
            line_cursor(&"abcdef".chars().collect::<Vec<_>>(), 3),
            (0, 3)
        );
        assert_eq!(
            line_cursor(&"ab\ncd".chars().collect::<Vec<_>>(), 5),
            (1, 2)
        );
    }

    #[test]
    fn words_keep_caret_at_text_position() {
        let text = "one two three";
        let chars: Vec<_> = text.chars().collect();
        let backend = ratatui::backend::TestBackend::new(64, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_words(frame, vec![Line::from(text)], &chars, 4, frame.area());
            })
            .unwrap();
        assert_eq!(
            terminal.backend_mut().get_cursor_position().unwrap(),
            Position::new(6, 8)
        );
    }

    #[test]
    fn words_wrap_without_leading_spaces() {
        let text = "one two three";
        let chars: Vec<_> = text.chars().collect();
        let (lines, row, column) = wrap_words(vec![Line::from(text)], &chars, 8, 7);
        let rendered: Vec<String> = lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect()
            })
            .collect();
        assert_eq!(rendered, vec!["one two", "three"]);
        assert_eq!((row, column), (1, 0));
    }

    #[test]
    fn long_code_line_keeps_caret_inside_panel() {
        let chars: Vec<_> = "x".repeat(100).chars().collect();
        let backend = ratatui::backend::TestBackend::new(64, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_code(
                    frame,
                    vec![Line::from("x".repeat(100))],
                    &chars,
                    chars.len(),
                    frame.area(),
                );
            })
            .unwrap();
        let position = terminal.backend_mut().get_cursor_position().unwrap();
        assert!(position.x < 64);
        assert!(position.y < 20);
    }

    #[test]
    fn auto_paired_closer_is_green_before_cursor_reaches_it() {
        let mut session = Session::new(
            TypingContent {
                text: "(x)".into(),
                source: ContentSource::Words {
                    language: WordLanguage::English,
                    size: WordListSize::Top200,
                },
            },
            SessionMode::Snippet,
            MistakeMode::Strict,
            true,
        );
        session.type_char('(');
        let lines = expected_lines(&session, &[Color::White; 3]);
        assert_eq!(lines[0].spans[2].style.fg, Some(SUCCESS));
    }

    #[test]
    fn typed_buffer_keeps_actual_mistakes() {
        let mut session = Session::new(
            TypingContent {
                text: "ab".into(),
                source: ContentSource::Words {
                    language: WordLanguage::English,
                    size: WordListSize::Top200,
                },
            },
            SessionMode::WordCount(1),
            MistakeMode::Free,
            false,
        );
        session.type_char('a');
        session.type_char('x');
        assert_eq!(typed_buffer(&session), vec![('a', false), ('x', true)]);
    }

    #[test]
    fn word_count_meter_reports_words() {
        let mut session = Session::new(
            TypingContent {
                text: "one two".into(),
                source: ContentSource::Words {
                    language: WordLanguage::English,
                    size: WordListSize::Top200,
                },
            },
            SessionMode::WordCount(2),
            MistakeMode::Free,
            false,
        );
        for character in "one ".chars() {
            session.type_char(character);
        }
        assert_eq!(meter(&session), (0.5, "1 / 2 words · 50%".into()));
    }
}
