use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{Color, Style, Stylize},
    symbols::{self, border::Set},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_term::{
    vt100,
    widget::{Cursor, PseudoTerminal},
};
use vex_v5_serial::{
    connection::{
        serial::{SerialConnection, SerialError},
        Connection,
    },
    packets::{
        controller::{UserFifoPacket, UserFifoPayload, UserFifoReplyPacket},
        match_mode::{MatchMode, SetMatchModePacket, SetMatchModePayload, SetMatchModeReplyPacket},
        system::{GetSystemVersionPacket, GetSystemVersionReplyPacket, ProductType},
    },
};
use widgets::{set_duration_digit, Mode};

use crate::errors::CliError;

mod widgets;

async fn set_match_mode(
    connection: &mut SerialConnection,
    match_mode: MatchMode,
) -> Result<(), SerialError> {
    connection
        .packet_handshake::<SetMatchModeReplyPacket>(
            Duration::from_millis(500),
            10,
            SetMatchModePacket::new(SetMatchModePayload {
                match_mode,
                match_time: 0,
            }),
        )
        .await?;
    Ok(())
}

async fn try_read_terminal(connection: &mut SerialConnection) -> Result<Vec<u8>, CliError> {
    let read = connection
        .packet_handshake::<UserFifoReplyPacket>(
            Duration::from_millis(100),
            1,
            UserFifoPacket::new(UserFifoPayload {
                channel: 1, // stdio channel
                write: None,
            }),
        )
        .await?
        .try_into_inner()?;

    let mut data = Vec::new();
    if let Some(read) = read.data {
        data.extend(read.0.as_bytes());
    }

    Ok(data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchModeFocus {
    Auto,
    Driver,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    MatchMode(MatchModeFocus),
    Countdown,
}

struct CursorPos(usize);
impl CursorPos {
    fn move_left(&mut self) {
        if self.0 > 0 {
            self.0 -= 1;
        }
    }
    fn move_right(&mut self) {
        if self.0 < 3 {
            self.0 += 1;
        }
    }
}

struct CountdownState {
    auto_set_time: Duration,
    auto_cursor_pos: CursorPos,
    driver_set_time: Duration,
    driver_cursor_pos: CursorPos,
    disabled_set_time: Duration,
    disabled_cursor_pos: CursorPos,
    current_time: Duration,
    start_time: Instant,
    running: bool,
}
impl CountdownState {
    fn current_set_time(&self, match_mode: MatchMode) -> Duration {
        match match_mode {
            MatchMode::Auto => self.auto_set_time,
            MatchMode::Driver => self.driver_set_time,
            MatchMode::Disabled => self.disabled_set_time,
        }
    }
}

struct TuiState {
    current_mode: MatchMode,
    focus: Focus,
    parser: vt100::Parser,

    countdown: CountdownState,
}

fn draw_tui(frame: &mut Frame, state: &mut TuiState) {
    let title_style = Style::default().fg(Color::White).bold();

    let minutes = state.countdown.current_time.as_secs() / 60;
    let seconds = state.countdown.current_time.as_secs() % 60;
    let countdown_text = format!("{minutes:02}:{seconds:02}");

    let main_sections = Layout::horizontal([Constraint::Min(20), Constraint::Percentage(100)]);
    let [left_area, terminal_area] = main_sections.areas(frame.area());
    let options = Layout::vertical([Constraint::Min(2), Constraint::Percentage(100)]);
    let [countdown_area, mode_area] = options.areas(left_area);

    let countdown_block = Block::default()
        .borders(Borders::BOTTOM.complement())
        .border_set(symbols::border::ROUNDED)
        .title("Countdown")
        .title_style(title_style);
    let mut countdown = Paragraph::new(countdown_text);
    if state.countdown.running {
        countdown = countdown.green();
    }
    if let Focus::Countdown = state.focus {
        countdown = countdown.fg(Color::LightBlue);
    }

    frame.render_widget(countdown, countdown_block.inner(countdown_area));
    frame.render_widget(countdown_block, countdown_area);

    let mode_block = Block::bordered()
        .border_set(Set {
            top_left: symbols::line::NORMAL.vertical_right,
            top_right: symbols::line::NORMAL.vertical_left,
            ..symbols::border::ROUNDED
        })
        .title("Match Mode")
        .title_style(title_style);

    let [driver_area, auto_area, disabled_area] =
        Layout::vertical([Constraint::Max(1), Constraint::Max(1), Constraint::Max(1)])
            .flex(Flex::Start)
            .areas(mode_block.inner(mode_area));

    let mut driver = Mode::new(String::from("Driver"), state.countdown.driver_set_time);
    driver.set_cursor_position(state.countdown.driver_cursor_pos.0);
    let mut auto = Mode::new(String::from("Auto"), state.countdown.auto_set_time);
    auto.set_cursor_position(state.countdown.auto_cursor_pos.0);
    let mut disabled = Mode::new(String::from("Disabled"), state.countdown.disabled_set_time);
    disabled.set_cursor_position(state.countdown.disabled_cursor_pos.0);

    if let Focus::MatchMode(mode) = &state.focus {
        match mode {
            MatchModeFocus::Auto => {
                auto.select();
                auto.place_cursor(frame, auto_area);
            }
            MatchModeFocus::Driver => {
                driver.select();
                driver.place_cursor(frame, driver_area);
            }
            MatchModeFocus::Disabled => {
                disabled.select();
                disabled.place_cursor(frame, disabled_area);
            }
        }
    }
    match state.current_mode {
        MatchMode::Auto => auto.current = true,
        MatchMode::Driver => driver.current = true,
        MatchMode::Disabled => disabled.current = true,
    }

    frame.render_widget(driver, driver_area);
    frame.render_widget(auto, auto_area);
    frame.render_widget(disabled, disabled_area);
    frame.render_widget(mode_block, mode_area);

    let terminal_block = Block::bordered()
        .border_set(symbols::border::ROUNDED)
        .title("Program Output")
        .title_style(title_style);

    let size = terminal_block.inner(terminal_area).as_size();
    state.parser.set_size(size.height + 1, size.width);

    let mut cursor = Cursor::default();
    cursor.hide();

    let terminal = PseudoTerminal::new(state.parser.screen())
        .cursor(cursor)
        .block(terminal_block)
        .style(Style::default().fg(Color::White).bg(Color::Black));
    frame.render_widget(terminal, terminal_area);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    None,
    Exit,
    ChangeMode(MatchMode),
}

fn handle_events(tui_state: &mut TuiState) -> io::Result<Control> {
    Ok(match event::read()? {
        Event::Key(key) => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Control::Exit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Control::Exit,
            KeyCode::Char('j') | KeyCode::Down => {
                match tui_state.focus {
                    Focus::Countdown => tui_state.focus = Focus::MatchMode(MatchModeFocus::Driver),
                    Focus::MatchMode(MatchModeFocus::Driver) => {
                        tui_state.focus = Focus::MatchMode(MatchModeFocus::Auto)
                    }
                    Focus::MatchMode(MatchModeFocus::Auto) => {
                        tui_state.focus = Focus::MatchMode(MatchModeFocus::Disabled)
                    }
                    Focus::MatchMode(MatchModeFocus::Disabled) => {
                        tui_state.focus = Focus::Countdown
                    }
                }
                Control::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                match tui_state.focus {
                    Focus::Countdown => {
                        tui_state.focus = Focus::MatchMode(MatchModeFocus::Disabled)
                    }
                    Focus::MatchMode(MatchModeFocus::Driver) => tui_state.focus = Focus::Countdown,
                    Focus::MatchMode(MatchModeFocus::Auto) => {
                        tui_state.focus = Focus::MatchMode(MatchModeFocus::Driver)
                    }
                    Focus::MatchMode(MatchModeFocus::Disabled) => {
                        tui_state.focus = Focus::MatchMode(MatchModeFocus::Auto)
                    }
                }
                Control::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                match tui_state.focus {
                    Focus::Countdown => tui_state.countdown.running = !tui_state.countdown.running,
                    Focus::MatchMode(MatchModeFocus::Driver) => {
                        tui_state.current_mode = MatchMode::Driver;
                    }
                    Focus::MatchMode(MatchModeFocus::Auto) => {
                        tui_state.current_mode = MatchMode::Auto;
                    }
                    Focus::MatchMode(MatchModeFocus::Disabled) => {
                        tui_state.current_mode = MatchMode::Disabled;
                    }
                }
                Control::ChangeMode(tui_state.current_mode)
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Focus::MatchMode(mode) = tui_state.focus {
                    match mode {
                        MatchModeFocus::Auto => tui_state.countdown.auto_cursor_pos.move_left(),
                        MatchModeFocus::Driver => tui_state.countdown.driver_cursor_pos.move_left(),
                        MatchModeFocus::Disabled => {
                            tui_state.countdown.disabled_cursor_pos.move_left()
                        }
                    }
                }

                Control::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Focus::MatchMode(mode) = tui_state.focus {
                    match mode {
                        MatchModeFocus::Auto => tui_state.countdown.auto_cursor_pos.move_right(),
                        MatchModeFocus::Driver => {
                            tui_state.countdown.driver_cursor_pos.move_right()
                        }
                        MatchModeFocus::Disabled => {
                            tui_state.countdown.disabled_cursor_pos.move_right()
                        }
                    }
                }

                Control::None
            }
            KeyCode::Char(ch) if ch.is_numeric() => {
                let digit = ch.to_digit(10).unwrap() as u8;

                if let Focus::MatchMode(mode) = tui_state.focus {
                    match mode {
                        MatchModeFocus::Auto => {
                            tui_state.countdown.auto_set_time = set_duration_digit(
                                digit,
                                tui_state.countdown.auto_cursor_pos.0,
                                tui_state.countdown.auto_set_time,
                            );
                            tui_state.countdown.auto_cursor_pos.move_right();
                        }
                        MatchModeFocus::Driver => {
                            tui_state.countdown.driver_set_time = set_duration_digit(
                                digit,
                                tui_state.countdown.driver_cursor_pos.0,
                                tui_state.countdown.driver_set_time,
                            );
                            tui_state.countdown.driver_cursor_pos.move_right()
                        }
                        MatchModeFocus::Disabled => {
                            tui_state.countdown.disabled_set_time = set_duration_digit(
                                digit,
                                tui_state.countdown.disabled_cursor_pos.0,
                                tui_state.countdown.disabled_set_time,
                            );
                            tui_state.countdown.disabled_cursor_pos.move_right()
                        }
                    }
                }
                Control::None
            }
            _ => Control::None,
        },
        _ => Control::None,
    })
}

fn handle_countdown(tui_state: &mut TuiState) -> Control {
    if tui_state.countdown.running {
        let elapsed = tui_state.countdown.start_time.elapsed();
        tui_state.countdown.current_time = tui_state
            .countdown
            .current_set_time(tui_state.current_mode)
            .checked_sub(elapsed)
            .unwrap_or_default();
        if tui_state.countdown.current_time.as_secs() == 0 {
            tui_state.countdown.start_time = Instant::now();
            match tui_state.current_mode {
                MatchMode::Auto => {
                    tui_state.current_mode = MatchMode::Driver;
                    return Control::ChangeMode(MatchMode::Driver);
                }
                MatchMode::Driver => {
                    tui_state.current_mode = MatchMode::Disabled;
                    tui_state.countdown.running = false;
                    return Control::ChangeMode(MatchMode::Disabled);
                }
                MatchMode::Disabled => {
                    tui_state.current_mode = MatchMode::Auto;
                    return Control::ChangeMode(MatchMode::Auto);
                }
            }
        }
    } else {
        tui_state.countdown.current_time =
            tui_state.countdown.current_set_time(tui_state.current_mode);
        tui_state.countdown.start_time = Instant::now();
    }

    Control::None
}

pub async fn run_field_control_tui(connection: &mut SerialConnection) -> Result<(), CliError> {
    let response = connection
        .packet_handshake::<GetSystemVersionReplyPacket>(
            Duration::from_millis(700),
            5,
            GetSystemVersionPacket::new(()),
        )
        .await?;
    if let ProductType::Brain = response.payload.product_type {
        return Err(CliError::BrainConnectionSetMatchMode);
    }

    let mut tui_state = TuiState {
        current_mode: MatchMode::Disabled,
        focus: Focus::MatchMode(MatchModeFocus::Driver),
        parser: vt100::Parser::new(1, 1, 0),
        countdown: CountdownState {
            auto_set_time: Duration::from_secs(15),
            auto_cursor_pos: CursorPos(0),
            driver_set_time: Duration::from_secs(105),
            driver_cursor_pos: CursorPos(0),
            disabled_set_time: Duration::from_secs(0),
            disabled_cursor_pos: CursorPos(0),
            current_time: Duration::from_secs(0),
            start_time: Instant::now(),
            running: false,
        },
    };

    set_match_mode(connection, tui_state.current_mode).await?;

    let mut terminal = ratatui::init();
    loop {
        if let Control::ChangeMode(mode) = handle_countdown(&mut tui_state) {
            set_match_mode(connection, mode).await?;
        }
        if event::poll(Duration::from_millis(1))? {
            match handle_events(&mut tui_state)? {
                Control::None => {}
                Control::Exit => break,
                Control::ChangeMode(mode) => {
                    set_match_mode(connection, mode).await?;
                }
            }
        }
        terminal.draw(|frame| draw_tui(frame, &mut tui_state))?;

        if let Ok(output) = try_read_terminal(connection).await {
            if !output.is_empty() {
                for byte in output.iter() {
                    let byte = if *byte == b'\n' {
                        &[b'\r', b'\n']
                    } else {
                        std::slice::from_ref(byte)
                    };
                    tui_state.parser.process(byte);
                }
            }
        }
    }
    ratatui::restore();
    Ok(())
}
