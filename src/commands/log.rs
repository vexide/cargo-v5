use clap::Args;
use chrono::NaiveTime;
use serde::{Serialize, Deserialize};
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::time::Duration;
use std::option::Option;
use tabwriter::{Alignment, TabWriter};
use vex_v5_serial::packets::log::{ReadLogPagePacket, ReadLogPagePayload, ReadLogPageReplyPacket};
use vex_v5_serial::packets::log::Log as V5SerialLog;

use vex_v5_serial::connection::{serial::SerialConnection, Connection};

use crate::errors::CliError;

const MAX_LOGS_PER_PAGE: u32 = 254;

#[derive(Args, Debug)]
pub struct LogOpts {
    #[arg(long, default_value = "None")]
    page: Option<NonZeroU32>,
    #[arg(long)]
    no_color: bool,
}

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
enum LogCategory {
    FieldControl,
    Warning,
    Error,
    Battery,
    #[default]
    Default
}

impl LogCategory {
    fn ansi_color(&self) -> &'static str {
        match self {
            // Bold white
            LogCategory::FieldControl => "\x1B[1m",
            // Yellow
            LogCategory::Warning => "\x1B[33m",
            // Red
            LogCategory::Error => "\x1B[31m",
            // Green
            LogCategory::Battery => "\x1B[32m",
            // Blue
            LogCategory::Default => "\x1B[34m",
        }
    }
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
struct Log {
    pub timestamp: Duration,
    pub category: LogCategory,
    pub text: String
}

impl Log {
    fn decode_log(log: V5SerialLog) -> Log {
        let timestamp = Duration::from_millis(log.time);
        
        let category = if matches!(log.log_type, 10..=0xc) {
            LogCategory::FieldControl
        } else if (128..u8::MAX).contains(&log.log_type) {
            LogCategory::Warning
        } else if matches!(
            log.description,
            2 | 8 | 9 | 0xf | 0x10 | 0x11 | 0x12 | 0x16 | 0x17 | 0x18 | 14
        ) {
            LogCategory::Error
        } else if log.description == 13 {
            LogCategory::Battery
        } else {
            LogCategory::Default
        };

        let text = match log.log_type {
            4 if log.description == 7 => format!("Field tether connected"),
            9 if log.description == 7 => format!("Radio linked"),
            10 => {
                if log.description & 0b11000000 == 0 {
                    format!(
                        "VRC-{}-{}",
                        log.description & 0b00111111,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    )
                } else {
                    format!(
                        "XXX-{}-{}",
                        log.description & 0b00111111,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    )
                }
            }
            11 => {
                let match_round = decode_match_round(log.description);
                match log.description {
                    2..=8 => format!("{}-{}-{}", match_round, log.code, log.spare),
                    9 | 99 => format!(
                        "{}-{:.04}",
                        match_round,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    ),
                    _ => format!("Match error"),
                }
            }
            12 => format!(
                "--> {:.02}:{:.02}:{:.02}",
                log.code, log.spare, log.description
            ),
            0..=127 => {
                let device_string = decode_device_type(log.spare);
                let type_string = decode_log_type(log.log_type);
                let error_string = decode_error_message(log.description);

                match log.description {
                    2 => format!("{} {}", type_string, error_string),
                    7 | 8 => match log.log_type {
                        3 => format!(
                            "{} {} on port {}",
                            device_string, error_string, log.code
                        ),
                        4 => format!("Field tether disconnected"),
                        _ => format!("{} {}", type_string, error_string),
                    },
                    9 => format!("{}", error_string),
                    11 => {
                        if log.spare == 2 {
                            format!("{} Run", decode_default_program(0));
                        } else if log.spare == 1 && log.code == 0 {
                            format!("{} Run", decode_default_program(1));
                        } else {
                            format!("{} slot {}", error_string, log.code);
                        }
                    }
                    13 => {
                        if log.code == 0 {
                            format!("{}", error_string);
                        } else if log.code == 0xff {
                            format!("Power off");
                        } else if log.code == 0xf0 {
                            format!("Reset");
                        }
                    }
                    14 => format!(
                        "{} {:.2}V {}% Capacity",
                        error_string,
                        log.code as f32 * 0.064,
                        log.spare,
                    ),
                    15 => {
                        if log.spare == 0 {
                            format!("{} Voltage", error_string);
                        } else {
                            format!("{} Cell {}", error_string, log.spare);
                        }
                    }
                    16 => format!("{} AFE fault", error_string),
                    17 => format!("Motor {} on port {}", error_string, log.code),
                    18 => format!(
                        "Motor {} {} on port {}",
                        error_string, log.spare, log.code
                    ),
                    22 => format!("{} Error", error_string),
                    23 => format!("Motor {} Error", error_string),
                    1 | 3..=6 | 10 | 12 | 19..=21 | 24 | 25 => format!("{}", error_string),
                    26.. => {                
                        format!(
                            "?: {:.02X} {:.02X} {:.02X} {:.02X}",
                            log.code, log.spare, log.description, log.log_type
                        );
                    }
                }
            }
            128 => match log.code {
                0x11 => format!("Program error: Invalid"),
                0x12 => format!("Program error: Abort"),
                0x13 => format!("Program error: SDK"),
                0x14 => format!("Program error: SDK Mismatch"),
                _ => format!(
                    "U {:.02X}:{:.02X}:{:.02X}",
                    log.code, log.spare, log.description
                ),
            },
            144 => format!("Program: Tamper"),
            160 => {
                let r1 = if (log.spare & 1) != 0 {
                    Some("R1")
                } else {
                    None
                };
                let r2 = if (log.spare & 2) != 0 {
                    Some("R2")
                } else {
                    None
                };
                let b1 = if (log.spare & 4) != 0 {
                    Some("B1")
                } else {
                    None
                };
                let b2 = if (log.spare & 8) != 0 {
                    Some("B2")
                } else {
                    None
                };

                match log.code {
                    1 => format!(
                        "FC: Cable - {}{}{}{}{}",
                        r1.unwrap_or_default(),
                        b1.unwrap_or_default(),
                        r2.unwrap_or_default(),
                        b2.unwrap_or_default(),
                        log.description
                    ),
                    2 => format!(
                        "FC: Radio - {}{}{}{}{}",
                        r1.unwrap_or_default(),
                        b1.unwrap_or_default(),
                        r2.unwrap_or_default(),
                        b2.unwrap_or_default(),
                        log.description
                    ),
                    _ => format!(
                        "FC: {:.02X}:{:.02X}:{:.02X}",
                        log.code, log.spare, log.description
                    ),
                }
            }
            _ => format!(
                "X: {:.02X}:{:.02X}:{:.02X}",
                log.code, log.spare, log.description
            ),
        };
        Log {
            timestamp,
            category,
            text,
        }
    }
}

pub async fn log(connection: &mut SerialConnection, opts: LogOpts) -> Result<(), CliError> {
    let LogOpts { page, no_color } = opts;
    let mut tw = TabWriter::new(io::stdout())
        .tab_indent(false)
        .padding(1)
        .ansi(true)
        .alignment(Alignment::Right);

    let mut entries = Vec::new();
    let page_range = match page {
        Some(page) => page.get()..(page.get() + 1),
        None => {
            let log_count = 
                connection
                    .packet_handshake::<GetLogCountReplyPacket>(
                        Duration::from_millis(500),
                        10,
                        GetLogCountPacket::new(()),
                    )
                    .await?
                    .payload
                    .count;
            let pages = log_count.div_ceil(MAX_LOGS_PER_PAGE);
            1..(pages+1)
        }
    });
    for page in page_range {
        entries.extend(
            connection
                .packet_handshake::<ReadLogPageReplyPacket>(
                    Duration::from_millis(500),
                    10,
                    ReadLogPagePacket::new(ReadLogPagePayload {
                        offset: MAX_LOGS_PER_PAGE * page.get(),
                        count: MAX_LOGS_PER_PAGE,
                    }),
                )
                .await?
                .payload
                .entries
                .into_iter()
                .enumerate()
                .map(|(i, log)| ((MAX_LOGS_PER_PAGE * page) - (i as u32), log))
                .rev(),
        )
    }

    // TODO: remove
    assert!(entries.iter().is_sorted());

    for (i, Log { timestamp, category, text }) in entries {
        let time = (NaiveTime::MIN + timestamp).format("%H:%M:%S");
        let color = if no_color {
            ""
        } else {
            category.ansi_color()
        }; 
        writeln!(&mut tw, "{color}{i}:\t[{time}]\t{text}\x1B[0m")?;
    }

    tw.flush()?;

    Ok(())
}

pub const fn decode_match_round(description: u8) -> &'static str {
    match description {
        1 => "Q",
        2 => "R16",
        3 => "QF",
        4 => "SF",
        5 => "F",
        6 => "PS",
        7 => "DS",
        8 | 9 => "P",
        99 => "X",
        _ => "UNK",
    }
}

pub const fn decode_log_type(log_type: u8) -> &'static str {
    match log_type {
        1 => "Brain",
        2 => "Battery",
        4 => "Field",
        5 => "Alert",
        6 => "NXP",
        7 => "Program",
        8 => "Controller",
        _ => "Unknown",
    }
}

pub const fn decode_device_type(device_type: u8) -> &'static str {
    match device_type {
        0 => "NXB",
        1 => "NXA",
        2 | 5 | 21 | 25 => "Motor",
        3 => "LED",
        4 => "Rotation",
        6 => "Inertial",
        7 => "Distance",
        8 => "Radio",
        9 => "Controller",
        10 => "Brain",
        11 => "Vision",
        12 => "ADI",
        13 => "Partner Controller",
        14 => "Battery",
        16 => "Optical",
        17 => "Electromagnet",
        20 => "GPS",
        26 | 29 => "Device",
        27 => "Light Tower",
        28 => "Arm",
        30 => "Pneumatic",
        31 => "Motor Controller 55",
        _ => "Unknown Device",
    }
}

pub const fn decode_default_program(default_program: u8) -> &'static str {
    match default_program {
        0 => "Driver",
        1 => "Clawbot",
        2 => "Sensor Demo",
        3 => "Tutorial",
        _ => "Unknown Default Program",
    }
}

pub const fn decode_error_message(log_description: u8) -> &'static str {
    match log_description {
        2 => "Download failure",
        3 => "Auton start",
        4 => "Match pause",
        5 => "Driver start",
        6 => "Match end",
        7 => "connected",
        8 => "disconnected",
        9 => "Lost radio connection",
        10 => "Disabled",
        11 => "Program run",
        12 => "Program stop",
        13 => "Power on",
        14 => "Battery",
        15 => "Low battery",
        16 => "Battery error",
        17 => "Motor over current",
        18 => "Motor over temperature",
        19 => "Radio link error",
        20 => "Field connected",
        21 => "Field disconnected",
        22 => "Program error",
        23 => "Power output",
        24 | 25 => "One or more ports are disabled",
        _ => "unknown error",
    }
}
