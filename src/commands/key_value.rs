use std::time::Duration;
use vex_serial::Connection;
use vex_serial::protocol::FixedString;
use vex_serial::protocol::cdc2::system::{KeyValueLoadPacket, KeyValueSavePacket};
use vex_serial::serial::SerialConnection;

use crate::errors::CliError;

pub fn kv_set(connection: &mut SerialConnection, key: &str, value: &str) -> Result<(), CliError> {
    connection.handshake(
        KeyValueSavePacket {
            key: FixedString::new(key)?,
            value: FixedString::new(value)?,
        },
        Duration::from_millis(500),
        1,
    )??;

    Ok(())
}

pub fn kv_get(connection: &mut SerialConnection, key: &str) -> Result<String, CliError> {
    Ok(connection
        .handshake(
            KeyValueLoadPacket {
                key: FixedString::new(key)?,
            },
            Duration::from_millis(500),
            1,
        )??
        .value
        .to_string())
}
