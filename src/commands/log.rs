use std::io::{self, Write};
use std::num::NonZeroU32;
use std::time::Duration;
use tabwriter::{Alignment, TabWriter};
use vex_v5_serial::{
    Connection,
    protocol::cdc2::system::{LogReadPacket, LogReadPayload, LogReadReplyPacket},
    serial::SerialConnection,
};

use crate::errors::CliError;

const MAX_LOGS_PER_PAGE: u32 = 254;

pub async fn log(connection: &mut SerialConnection, page: NonZeroU32) -> Result<(), CliError> {
    let mut tw = TabWriter::new(io::stdout())
        .tab_indent(false)
        .padding(1)
        .alignment(Alignment::Right);

    let mut entries = Vec::new();
    entries.extend(
        connection
            .handshake::<LogReadReplyPacket>(
                Duration::from_millis(500),
                10,
                LogReadPacket::new(LogReadPayload {
                    offset: MAX_LOGS_PER_PAGE * page.get(),
                    count: MAX_LOGS_PER_PAGE,
                }),
            )
            .await?
            .payload?
            .entries,
    );

    for (i, log) in entries.into_iter().enumerate() {
        let time = log.time / 1000;
        write!(
            &mut tw,
            "{}:\t[{:02}:{:02}:{:02}]\t",
            (MAX_LOGS_PER_PAGE * page.get()) - (i as u32),
            (time / 3600) % 24,
            (time / 60) % 60,
            time % 60
        )?;

        if matches!(log.log_type, 10..=0xc) {
            write!(&mut tw, "\x1B[1m")?; // Bold white
        } else if (128..u8::MAX).contains(&log.log_type) {
            write!(&mut tw, "\x1B[33m")?; // Yellow (warning)
        } else if matches!(
            log.description,
            2 | 8 | 9 | 0xf | 0x10 | 0x11 | 0x12 | 0x16 | 0x17 | 0x18 | 14
        ) {
            write!(&mut tw, "\x1B[31m")?; // Error
        } else if log.description == 13 {
            write!(&mut tw, "\x1B[32m")?; // Green (battery-related)
        } else {
            write!(&mut tw, "\x1B[34m")?; // Blue (default)
        }

        match log.log_type {
            4 if log.description == 7 => writeln!(&mut tw, "Field tether connected")?,
            9 if log.description == 7 => writeln!(&mut tw, "Radio linked")?,
            10 => {
                if log.description & 0b11000000 == 0 {
                    writeln!(
                        &mut tw,
                        "VRC-{}-{}",
                        log.description & 0b00111111,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    )?
                } else {
                    writeln!(
                        &mut tw,
                        "XXX-{}-{}",
                        log.description & 0b00111111,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    )?
                }
            }
            11 => {
                let match_round = decode_match_round(log.description);
                match log.description {
                    2..=8 => writeln!(&mut tw, "{}-{}-{}", match_round, log.code, log.spare)?,
                    9 | 99 => writeln!(
                        &mut tw,
                        "{}-{:.04}",
                        match_round,
                        u32::from(log.code) * 256 + u32::from(log.spare)
                    )?,
                    _ => writeln!(&mut tw, "Match error")?,
                }
            }
            12 => writeln!(
                &mut tw,
                "--> {:.02}:{:.02}:{:.02}",
                log.code, log.spare, log.description
            )?,
            0..=127 => {
                let device_string = decode_device_type(log.spare);
                let type_string = decode_log_type(log.log_type);
                let error_string = decode_error_message(log.description);

                match log.description {
                    2 => writeln!(&mut tw, "{type_string} {error_string}")?,
                    7 | 8 => match log.log_type {
                        3 => writeln!(
                            &mut tw,
                            "{} {} on port {}",
                            device_string, error_string, log.code
                        )?,
                        4 => writeln!(&mut tw, "Field tether disconnected")?,
                        _ => writeln!(&mut tw, "{type_string} {error_string}")?,
                    },
                    9 => writeln!(&mut tw, "{error_string}")?,
                    11 => {
                        if log.spare == 2 {
                            writeln!(&mut tw, "{} Run", decode_default_program(0))?;
                        } else if log.spare == 1 && log.code == 0 {
                            writeln!(&mut tw, "{} Run", decode_default_program(1))?;
                        } else {
                            writeln!(&mut tw, "{} slot {}", error_string, log.code)?;
                        }
                    }
                    13 => {
                        if log.code == 0 {
                            writeln!(&mut tw, "{error_string}")?;
                        } else if log.code == 0xff {
                            writeln!(&mut tw, "Power off")?;
                        } else if log.code == 0xf0 {
                            writeln!(&mut tw, "Reset")?;
                        }
                    }
                    14 => writeln!(
                        &mut tw,
                        "{} {:.2}V {}% Capacity",
                        error_string,
                        log.code as f32 * 0.064,
                        log.spare,
                    )?,
                    15 => {
                        if log.spare == 0 {
                            writeln!(&mut tw, "{error_string} Voltage")?;
                        } else {
                            writeln!(&mut tw, "{} Cell {}", error_string, log.spare)?;
                        }
                    }
                    16 => writeln!(&mut tw, "{error_string} AFE fault")?,
                    17 => writeln!(&mut tw, "Motor {} on port {}", error_string, log.code)?,
                    18 => writeln!(
                        &mut tw,
                        "Motor {} {} on port {}",
                        error_string, log.spare, log.code
                    )?,
                    22 => writeln!(&mut tw, "{error_string} Error")?,
                    23 => writeln!(&mut tw, "Motor {error_string} Error")?,
                    24 => writeln!(&mut tw, "{error_string}")?,
                    _ => {
                        if log.description < 26 {
                            writeln!(&mut tw, "{error_string}")?;
                        } else {
                            writeln!(
                                &mut tw,
                                "?: {:.02X} {:.02X} {:.02X} {:.02X}",
                                log.code, log.spare, log.description, log.log_type
                            )?;
                        }
                    }
                }
            }
            128 => match log.code {
                0x11 => writeln!(&mut tw, "Program error: Invalid")?,
                0x12 => writeln!(&mut tw, "Program error: Abort")?,
                0x13 => writeln!(&mut tw, "Program error: SDK")?,
                0x14 => writeln!(&mut tw, "Program error: SDK Mismatch")?,
                _ => writeln!(
                    &mut tw,
                    "U {:.02X}:{:.02X}:{:.02X}",
                    log.code, log.spare, log.description
                )?,
            },
            144 => writeln!(&mut tw, "Program: Tamper")?,
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
                    1 => writeln!(
                        &mut tw,
                        "FC: Cable - {}{}{}{}{}",
                        r1.unwrap_or_default(),
                        b1.unwrap_or_default(),
                        r2.unwrap_or_default(),
                        b2.unwrap_or_default(),
                        log.description
                    )?,
                    2 => writeln!(
                        &mut tw,
                        "FC: Radio - {}{}{}{}{}",
                        r1.unwrap_or_default(),
                        b1.unwrap_or_default(),
                        r2.unwrap_or_default(),
                        b2.unwrap_or_default(),
                        log.description
                    )?,
                    _ => writeln!(
                        &mut tw,
                        "FC: {:.02X}:{:.02X}:{:.02X}",
                        log.code, log.spare, log.description
                    )?,
                }
            }
            _ => writeln!(
                &mut tw,
                "X: {:.02X}:{:.02X}:{:.02X}",
                log.code, log.spare, log.description
            )?,
        }
        write!(&mut tw, "\x1B[0m")?;
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
