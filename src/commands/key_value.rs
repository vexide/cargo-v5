use std::time::Duration;
use vex_v5_serial::Connection;
use vex_v5_serial::protocol::FixedString;
use vex_v5_serial::protocol::cdc2::system::{
    KeyValueLoadPacket, KeyValueLoadReplyPacket, KeyValueSavePacket, KeyValueSavePayload,
    KeyValueSaveReplyPacket,
};
use vex_v5_serial::serial::SerialConnection;

use crate::errors::CliError;

pub async fn kv_set(
    connection: &mut SerialConnection,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    connection
        .handshake::<KeyValueSaveReplyPacket>(
            Duration::from_millis(500),
            1,
            KeyValueSavePacket::new(KeyValueSavePayload {
                key: FixedString::new(key)?,
                value: FixedString::new(value)?,
            }),
        )
        .await?
        .payload?;

    Ok(())
}

pub async fn kv_get(connection: &mut SerialConnection, key: &str) -> Result<String, CliError> {
    Ok(connection
        .handshake::<KeyValueLoadReplyPacket>(
            Duration::from_millis(500),
            1,
            KeyValueLoadPacket::new(FixedString::new(key)?),
        )
        .await?
        .payload?
        .to_string())
}
