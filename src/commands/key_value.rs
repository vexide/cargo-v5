use std::time::Duration;
use vex_v5_serial::Connection;
use vex_v5_serial::protocol::FixedString;
use vex_v5_serial::protocol::cdc2::system::{KeyValueLoadPacket, KeyValueSavePacket};
use vex_v5_serial::serial::SerialConnection;

use crate::errors::CliError;

pub async fn kv_set(
    connection: &mut SerialConnection,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    connection
        .handshake(
            KeyValueSavePacket {
                key: FixedString::new(key)?,
                value: FixedString::new(value)?,
            },
            Duration::from_millis(500),
            1,
        )
        .await??;

    Ok(())
}

pub async fn kv_get(connection: &mut SerialConnection, key: &str) -> Result<String, CliError> {
    Ok(connection
        .handshake(
            KeyValueLoadPacket {
                key: FixedString::new(key)?,
            },
            Duration::from_millis(500),
            1,
        )
        .await??
        .value
        .to_string())
}
