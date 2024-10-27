use std::{
    collections::VecDeque,
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Flex, Layout, Position},
    style::Stylize,
    text::Line,
    widgets::{Block, Paragraph},
    Frame,
};
use tokio::time::sleep;
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

use crate::errors::CliError;

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

struct CountdownState {
    auto_set_time: Duration,
    driver_set_time: Duration,
    disabled_set_time: Duration,
    current_time: Duration,
    start_time: Instant,
    running: bool,

    cursor_pos: usize,
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
    program_output: VecDeque<String>,

    countdown: CountdownState,
}

fn draw_tui(frame: &mut Frame, state: &mut TuiState) {
    let minutes = state.countdown.current_time.as_secs() / 60;
    let seconds = state.countdown.current_time.as_secs() % 60;
    let countdown_text = format!("{minutes:02}:{seconds:02}");

    let main_sections =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]);
    let [left_area, terminal_area] = main_sections.areas(frame.area());
    let options = Layout::vertical([Constraint::Min(3), Constraint::Percentage(100)]);
    let [countdown_area, mode_area] = options.areas(left_area);

    let countdown_block = Block::bordered().title("Countdown");
    let mut countdown = Paragraph::new(countdown_text);
    if state.countdown.running {
        countdown = countdown.green();
    } else if state.focus == Focus::Countdown {
        let area = countdown_block.inner(countdown_area);
        frame.set_cursor_position(Position::new(
            area.x
                + if state.countdown.cursor_pos > 1 {
                    state.countdown.cursor_pos + 1
                } else {
                    state.countdown.cursor_pos
                } as u16,
            area.y,
        ))
    }
    if let Focus::Countdown = state.focus {
        countdown = countdown.bold();
    }

    frame.render_widget(countdown, countdown_block.inner(countdown_area));
    frame.render_widget(countdown_block, countdown_area);

    let mode_block = Block::bordered().title("Match Mode");

    let [driver_area, auto_area, disabled_area] =
        Layout::vertical([Constraint::Max(1), Constraint::Max(1), Constraint::Max(1)])
            .flex(Flex::Start)
            .areas(mode_block.inner(mode_area));

    let mut driver = Line::raw("Driver");
    let mut auto = Line::raw("Auto");
    let mut disabled = Line::raw("Disabled");

    if let Focus::MatchMode(mode) = &state.focus {
        match mode {
            MatchModeFocus::Auto => auto = auto.bold(),
            MatchModeFocus::Driver => driver = driver.bold(),
            MatchModeFocus::Disabled => disabled = disabled.bold(),
        }
    }
    match state.current_mode {
        MatchMode::Auto => auto = auto.green(),
        MatchMode::Driver => driver = driver.green(),
        MatchMode::Disabled => disabled = disabled.green(),
    }

    frame.render_widget(driver, driver_area);
    frame.render_widget(auto, auto_area);
    frame.render_widget(disabled, disabled_area);
    frame.render_widget(mode_block, mode_area);

    let terminal_block = Block::bordered().title("Program Output");

    let height = terminal_block.inner(terminal_area).as_size().height as usize;
    state.program_output.truncate(height);

    let lines = Layout::vertical(vec![Constraint::Max(1); height])
        .flex(Flex::End)
        .split(terminal_block.inner(terminal_area));
    for (i, line) in lines.iter().rev().enumerate() {
        if let Some(term_line) = state.program_output.get(i) {
            let term_line = Line::raw(term_line);
            frame.render_widget(term_line, *line);
        }
    }
    frame.render_widget(terminal_block, terminal_area);
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
                if let Focus::Countdown = tui_state.focus {
                    if tui_state.countdown.cursor_pos > 0 {
                        tui_state.countdown.cursor_pos -= 1;
                    }
                }
                Control::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Focus::Countdown = tui_state.focus {
                    if tui_state.countdown.cursor_pos < 3 {
                        tui_state.countdown.cursor_pos += 1;
                    }
                }
                Control::None
            }
            KeyCode::Char(ch) if ch.is_numeric() => {
                if let Focus::Countdown = tui_state.focus {
                    let digit = ch.to_digit(10).unwrap() as u64;
                    let current_time = tui_state.countdown.current_time.as_secs();

                    let new_time = match tui_state.countdown.cursor_pos {
                        0 => digit * 600 + current_time % 600,
                        1 => digit * 60 + current_time % 60 + (current_time / 600) * 600,
                        2 => digit * 10 + current_time % 10 + (current_time / 60) * 60,
                        3 => digit + (current_time / 10) * 10,
                        _ => unreachable!(),
                    };

                    match tui_state.current_mode {
                        MatchMode::Auto => {
                            tui_state.countdown.auto_set_time = Duration::from_secs(new_time)
                        }
                        MatchMode::Driver => {
                            tui_state.countdown.driver_set_time = Duration::from_secs(new_time)
                        }
                        MatchMode::Disabled => {
                            tui_state.countdown.disabled_set_time = Duration::from_secs(new_time)
                        }
                    }
                    if tui_state.countdown.cursor_pos < 3 {
                        tui_state.countdown.cursor_pos += 1;
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
        program_output: VecDeque::new(),
        countdown: CountdownState {
            auto_set_time: Duration::from_secs(15),
            driver_set_time: Duration::from_secs(105),
            disabled_set_time: Duration::from_secs(0),
            current_time: Duration::from_secs(0),
            start_time: Instant::now(),
            running: false,
            cursor_pos: 0,
        },
    };

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
            let output = std::str::from_utf8(&output).unwrap();
            for line in output.lines() {
                tui_state.program_output.push_front(line.to_string());
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    ratatui::restore();
    Ok(())
}
