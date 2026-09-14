mod app;
mod engine;
mod stats;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::{Result, bail};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            SetCursorStyle::DefaultUserShape,
            LeaveAlternateScreen,
            Show
        );
    }
}

fn main() -> Result<()> {
    if handle_info_argument()? {
        return Ok(());
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    run(&mut terminal)
}

fn handle_info_argument() -> Result<bool> {
    let mut arguments = std::env::args().skip(1);
    let Some(argument) = arguments.next() else {
        return Ok(false);
    };
    if arguments.next().is_some() {
        bail!("typeforge accepts at most one option; use --help");
    }
    match argument.as_str() {
        "-h" | "--help" => println!(
            "typeforge {}\n\nFocused terminal typing practice for words and real code.\n\nUsage: typeforge\n\nOptions:\n  -h, --help       Show help\n  -V, --version    Show version",
            env!("CARGO_PKG_VERSION")
        ),
        "-V" | "--version" => println!("typeforge {}", env!("CARGO_PKG_VERSION")),
        _ => bail!("unknown option {argument:?}; use --help"),
    }
    Ok(true)
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = app::App::new();
    let mut active_caret = None;
    loop {
        app.poll_loader();
        if app.state == app::AppState::Typing && app.session_timed_out() {
            app.finish_session();
        }
        if active_caret != Some(app.settings.caret_style) {
            execute!(
                terminal.backend_mut(),
                terminal_caret(app.settings.caret_style)
            )?;
            active_caret = Some(app.settings.caret_style);
        }
        terminal.draw(|frame| ui::render(frame, &app))?;
        if !event::poll(Duration::from_millis(40))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && control_shortcut(key.modifiers) {
            return Ok(());
        }
        if app.state != app::AppState::Typing
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            continue;
        }
        match app.state {
            app::AppState::Menu => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Up | KeyCode::Char('k') => app.menu_prev(),
                KeyCode::Down | KeyCode::Char('j') => app.menu_next(),
                KeyCode::Left | KeyCode::Char('h') => app.menu_change(-1),
                KeyCode::Right | KeyCode::Char('l') => app.menu_change(1),
                KeyCode::Enter => app.menu_activate(),
                KeyCode::Char('s') => app.state = app::AppState::Stats,
                KeyCode::Char('r')
                    if app.settings.practice == engine::content::PracticeKind::Code =>
                {
                    app.start_session(true)
                }
                _ => {}
            },
            app::AppState::Loading => {
                if key.code == KeyCode::Esc {
                    app.cancel_loading();
                }
            }
            app::AppState::Typing => handle_typing_key(&mut app, key),
            app::AppState::Results => match key.code {
                KeyCode::Esc => app.state = app::AppState::Menu,
                KeyCode::Tab => app.start_session(false),
                KeyCode::Enter => app.retry(),
                KeyCode::Char('s') => app.state = app::AppState::Stats,
                _ => {}
            },
            app::AppState::Stats => {
                if key.code == KeyCode::Esc {
                    app.state = app::AppState::Menu;
                }
            }
        }
        if app.state == app::AppState::Typing && app.session_complete() {
            app.finish_session();
        }
    }
}

fn handle_typing_key(app: &mut app::App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return;
    }
    if control_shortcut(key.modifiers) {
        match key.code {
            KeyCode::Backspace | KeyCode::Char('w') => app.delete_word(),
            KeyCode::Char('h') => app.backspace(),
            KeyCode::Char('r') => app.retry(),
            KeyCode::Char('u') => app.delete_line(),
            _ => {}
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && !key.modifiers.contains(KeyModifiers::CONTROL) {
        if key.code == KeyCode::Backspace {
            app.delete_word();
        }
        return;
    }
    match key.code {
        KeyCode::Esc => app.state = app::AppState::Menu,
        KeyCode::F(2) => app.show_typed = !app.show_typed,
        KeyCode::F(3) => app.retry(),
        KeyCode::Backspace => app.backspace(),
        KeyCode::Enter => app.type_char('\n'),
        KeyCode::Tab => app.cycle_session(),
        KeyCode::Char(character) => app.type_char(character),
        _ => {}
    }
}

fn terminal_caret(style: app::CaretStyle) -> SetCursorStyle {
    match style {
        app::CaretStyle::BlinkingBar => SetCursorStyle::BlinkingBar,
        app::CaretStyle::SteadyBar => SetCursorStyle::SteadyBar,
        app::CaretStyle::BlinkingBlock => SetCursorStyle::BlinkingBlock,
        app::CaretStyle::SteadyBlock => SetCursorStyle::SteadyBlock,
        app::CaretStyle::BlinkingUnderline => SetCursorStyle::BlinkingUnderScore,
        app::CaretStyle::SteadyUnderline => SetCursorStyle::SteadyUnderScore,
    }
}

fn control_shortcut(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_settings_map_to_native_terminal_shapes() {
        assert_eq!(
            terminal_caret(app::CaretStyle::BlinkingBar),
            SetCursorStyle::BlinkingBar
        );
        assert_eq!(
            terminal_caret(app::CaretStyle::SteadyUnderline),
            SetCursorStyle::SteadyUnderScore
        );
    }
}
