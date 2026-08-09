mod menu;
mod results;
mod stats;
mod typing;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, AppState};

pub const ACCENT: Color = Color::Rgb(196, 158, 255);
pub const ACCENT_DARK: Color = Color::Rgb(156, 105, 220);
pub const BORDER: Color = Color::Rgb(55, 55, 68);
pub const ERROR: Color = Color::Rgb(245, 120, 130);
pub const MUTED: Color = Color::Rgb(105, 105, 120);
pub const PANEL: Color = Color::Rgb(36, 33, 48);
pub const SUCCESS: Color = Color::Rgb(105, 190, 125);

const MINIMUM_WIDTH: u16 = 64;
const MINIMUM_HEIGHT: u16 = 20;

pub fn render(frame: &mut Frame, app: &App) {
    if !terminal_size_supported(frame.area()) {
        frame.render_widget(
            Paragraph::new("Terminal too small · minimum 64×20")
                .alignment(Alignment::Center)
                .style(Style::default().fg(ERROR)),
            frame.area(),
        );
        return;
    }
    match app.state {
        AppState::Menu | AppState::Loading => menu::render(frame, app),
        AppState::Typing => typing::render(frame, app),
        AppState::Results => results::render(frame, app),
        AppState::Stats => stats::render(frame, app),
    }
}

pub fn shell(frame: &Frame) -> (Rect, Rect, Rect) {
    let outer = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(2),
    ])
    .margin(2)
    .split(frame.area());
    (outer[0], outer[1], outer[2])
}

fn terminal_size_supported(area: Rect) -> bool {
    area.width >= MINIMUM_WIDTH && area.height >= MINIMUM_HEIGHT
}

pub fn header() -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(
            "▣ typeforge",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  code + word typing", Style::default().fg(MUTED)),
        Span::styled(
            format!("  v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(BORDER),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_size_boundary_is_explicit() {
        assert!(terminal_size_supported(Rect::new(0, 0, 64, 20)));
        assert!(!terminal_size_supported(Rect::new(0, 0, 63, 20)));
        assert!(!terminal_size_supported(Rect::new(0, 0, 64, 19)));
    }

    #[test]
    fn menu_renders_at_minimum_size() {
        let backend = ratatui::backend::TestBackend::new(64, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::new();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("typeforge"));
        assert!(content.contains("Start session"));
    }
}
