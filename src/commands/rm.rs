use std::{path::PathBuf, str::FromStr, time::Duration};

use vex_v5_serial::{
    Connection,
    protocol::{
        FixedString,
        cdc2::file::{FileErasePacket, FileExitAction, FileTransferExitPacket},
    },
    serial::{SerialConnection, SerialError},
};

use crate::errors::CliError;

use super::cat::vendor_from_prefix;

pub async fn rm(connection: &mut SerialConnection, file: PathBuf) -> Result<(), CliError> {
    let vendor = vendor_from_prefix(if let Some(parent) = file.parent() {
        parent.to_str().unwrap()
    } else {
        ""
    });

    let file_name = FixedString::from_str(file.file_name().unwrap_or_default().to_str().unwrap())
        .map_err(|err| CliError::SerialError(SerialError::FixedStringSizeError(err)))?;

    connection
        .handshake(
            FileErasePacket {
                vendor,
                reserved: 0,
                file_name,
            },
            Duration::from_millis(500),
            1,
        )
        .await??;

    connection
        .handshake(
            FileTransferExitPacket {
                action: FileExitAction::DoNothing,
            },
            Duration::from_millis(500),
            1,
        )
        .await??;

    Ok(())
}
