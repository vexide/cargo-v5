use std::{path::PathBuf, str::FromStr, time::Duration};

use vex_v5_serial::{
    connection::{
        Connection,
        serial::{SerialConnection, SerialError},
    },
    packets::file::{
        EraseFilePacket, EraseFilePayload, EraseFileReplyPacket, ExitFileTransferPacket,
        ExitFileTransferReplyPacket, FileExitAction,
    },
    string::FixedString,
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
        .map_err(|err| CliError::SerialError(SerialError::EncodeError(err)))?;

    connection
        .packet_handshake::<EraseFileReplyPacket>(
            Duration::from_millis(500),
            1,
            EraseFilePacket::new(EraseFilePayload {
                vendor,
                option: 0,
                file_name,
            }),
        )
        .await?
        .try_into_inner()?;

    connection
        .packet_handshake::<ExitFileTransferReplyPacket>(
            Duration::from_millis(500),
            1,
            ExitFileTransferPacket::new(FileExitAction::DoNothing),
        )
        .await?
        .try_into_inner()?;

    Ok(())
}
