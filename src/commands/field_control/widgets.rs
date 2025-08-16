use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style, Stylize},
    symbols::border::ROUNDED,
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

pub fn set_duration_digit(digit: u8, pos: usize, duration: Duration) -> Duration {
    assert!((0..=9).contains(&digit), "Digit out of bounds");
    let digit = digit as u64;
    let current_duration = duration.as_secs();
    let new_time = match pos {
        0 => digit * 600 + current_duration % 600,
        1 => digit * 60 + current_duration % 60 + (current_duration / 600) * 600,
        2 => digit.min(5) * 10 + current_duration % 10 + (current_duration / 60) * 60,
        3 => digit + (current_duration / 10) * 10,
        _ => panic!("Invalid position"),
    };
    Duration::from_secs(new_time)
}

pub struct DurationInput {
    pub duration: Duration,
    cursor_position: usize,
    pub selected: bool,
}
impl DurationInput {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            cursor_position: 0,
            selected: false,
        }
    }
    pub fn set_cursor_position(&mut self, cursor_position: usize) {
        assert!(
            (0..=3).contains(&cursor_position),
            "Cursor position out of bounds"
        );
        self.cursor_position = cursor_position;
    }

    pub fn place_cursor(&self, frame: &mut Frame, area: Rect) {
        frame.set_cursor_position(Position::new(
            area.x
                + if self.cursor_position > 1 {
                    self.cursor_position + 1
                } else {
                    self.cursor_position
                } as u16,
            area.y,
        ))
    }
}
impl Widget for DurationInput {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let minutes = self.duration.as_secs() / 60;
        let seconds = self.duration.as_secs() % 60;
        let text = format!("{minutes:02}:{seconds:02}");

        let style = if self.selected {
            ratatui::prelude::Style::default().fg(ratatui::prelude::Color::LightBlue)
        } else {
            ratatui::prelude::Style::default()
        };

        buf.set_string(area.x, area.y, &text, style);
    }
}

pub struct Mode {
    pub name: String,
    pub selected: bool,
    pub current: bool,
    input: DurationInput,
}
impl Mode {
    pub fn new(name: String, duration: Duration) -> Self {
        Self {
            name,
            selected: false,
            current: false,
            input: DurationInput::new(duration),
        }
    }
    pub fn set_cursor_position(&mut self, cursor_position: usize) {
        self.input.set_cursor_position(cursor_position);
    }
    pub fn place_cursor(&self, frame: &mut Frame, area: Rect) {
        self.input.place_cursor(
            frame,
            Rect::new(
                area.x + self.name.len() as u16 + 2,
                area.y,
                area.width - self.name.len() as u16 - 2,
                area.height,
            ),
        );
    }

    pub fn select(&mut self) {
        self.selected = true;
        self.input.selected = true;
    }
}
impl Widget for Mode {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let style = if self.current {
            ratatui::prelude::Style::default().fg(ratatui::prelude::Color::LightGreen)
        } else if self.selected {
            ratatui::prelude::Style::default().fg(ratatui::prelude::Color::LightBlue)
        } else {
            ratatui::prelude::Style::default()
        };

        let name = format!("{}: ", self.name);
        buf.set_string(area.x, area.y, &name, style);
        self.input.render(
            Rect::new(
                area.x + name.len() as u16,
                area.y,
                area.width - name.len() as u16,
                area.height,
            ),
            buf,
        )
    }
}

pub struct HelpPopup;
impl HelpPopup {
    pub const HELP_TEXT: &'static str = "'q', 'esc' - Quit app or help
        'h', 'left' - Move cursor left
        'l', 'right' - Move cursor right
        'j', 'down' - Move focus down
        'k', 'up' - Move focus up
        'space', 'enter' - Select
        '0'-'9' - Set digit in mode duration input
        '?' - Show this help";
    pub const LINES: u16 = 9;
}
impl Widget for HelpPopup {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        Clear.render(area, buf);
        let block = Block::bordered()
            .border_set(ROUNDED)
            .title("Help")
            .title_style(Style::default().fg(Color::White).bold());
        Paragraph::new(Self::HELP_TEXT)
            .wrap(Wrap { trim: true })
            .block(block)
            .render(area, buf);
    }
}
