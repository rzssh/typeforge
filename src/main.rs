mod app;
mod engine;
mod multiplayer;
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
    let launch = parse_arguments()?;
    match launch {
        Launch::Exit => return Ok(()),
        Launch::Relay(address) => return multiplayer::server::run(&address),
        Launch::App(_) => {}
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let Launch::App(room_code) = launch else {
        unreachable!()
    };
    run(&mut terminal, room_code)
}

enum Launch {
    App(Option<String>),
    Relay(String),
    Exit,
}

fn parse_arguments() -> Result<Launch> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => Ok(Launch::App(None)),
        [argument] if argument == "-h" || argument == "--help" => {
            println!(
                "typeforge {}\n\nFocused terminal typing practice for words and real code.\n\nUsage:\n  typeforge\n  typeforge join <ROOM>\n  typeforge relay [ADDRESS]\n\nOptions:\n  -h, --help       Show help\n  -V, --version    Show version",
                env!("CARGO_PKG_VERSION")
            );
            Ok(Launch::Exit)
        }
        [argument] if argument == "-V" || argument == "--version" => {
            println!("typeforge {}", env!("CARGO_PKG_VERSION"));
            Ok(Launch::Exit)
        }
        [command, code] if command == "join" => Ok(Launch::App(Some(code.clone()))),
        [command] if command == "relay" => Ok(Launch::Relay("127.0.0.1:8787".into())),
        [command, address] if command == "relay" => Ok(Launch::Relay(address.clone())),
        _ => bail!("invalid arguments; use --help"),
    }
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    room_code: Option<String>,
) -> Result<()> {
    let mut app = app::App::new();
    if let Some(room_code) = room_code {
        app.open_multiplayer(Some(room_code));
        app.room_entry_activate();
    }
    let mut active_caret = None;
    loop {
        app.poll_loader();
        app.poll_multiplayer();
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
                KeyCode::Char('m') => app.open_multiplayer(None),
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
            app::AppState::RoomEntry => match key.code {
                KeyCode::Esc => app.state = app::AppState::Menu,
                KeyCode::Up => app.room_entry_prev(),
                KeyCode::Down | KeyCode::Tab => app.room_entry_next(),
                KeyCode::Backspace => app.room_entry_backspace(),
                KeyCode::Enter => app.room_entry_activate(),
                KeyCode::Char(character) => app.room_entry_type(character),
                _ => {}
            },
            app::AppState::Lobby => match key.code {
                KeyCode::Esc => app.leave_room(),
                KeyCode::Char('r') => app.toggle_ready(),
                KeyCode::Enter => app.start_race(),
                _ => {}
            },
            app::AppState::Countdown => {
                if key.code == KeyCode::Esc {
                    app.leave_room();
                }
            }
            app::AppState::Typing => handle_typing_key(&mut app, key),
            app::AppState::Results if app.multiplayer.is_some() => match key.code {
                KeyCode::Esc => app.leave_room(),
                _ => {}
            },
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
            KeyCode::Char('r') if app.multiplayer.is_none() => app.retry(),
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
        KeyCode::Esc if app.multiplayer.is_some() => app.leave_room(),
        KeyCode::Esc => app.state = app::AppState::Menu,
        KeyCode::F(2) => app.show_typed = !app.show_typed,
        KeyCode::F(3) if app.multiplayer.is_none() => app.retry(),
        KeyCode::Backspace => app.backspace(),
        KeyCode::Enter => app.type_char('\n'),
        KeyCode::Tab if app.multiplayer.is_none() => app.cycle_session(),
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
