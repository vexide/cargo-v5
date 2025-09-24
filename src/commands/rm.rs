use std::{path::PathBuf, str::FromStr, time::Duration};

use vex_v5_serial::{
    Connection,
    protocol::{
        FixedString,
        cdc2::file::{
            FileErasePacket, FileErasePayload, FileEraseReplyPacket, FileExitAction,
            FileTransferExitPacket, FileTransferExitReplyPacket,
        },
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
        .handshake::<FileEraseReplyPacket>(
            Duration::from_millis(500),
            1,
            FileErasePacket::new(FileErasePayload {
                vendor,
                reserved: 0,
                file_name,
            }),
        )
        .await?
        .payload?;

    connection
        .handshake::<FileTransferExitReplyPacket>(
            Duration::from_millis(500),
            1,
            FileTransferExitPacket::new(FileExitAction::DoNothing),
        )
        .await?
        .payload?;

    Ok(())
}
