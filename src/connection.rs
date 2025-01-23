use log::info;
use std::time::Duration;
use tokio::{select, task::spawn_blocking, time::sleep};
use vex_v5_serial::{
    connection::{
        serial::{self, SerialConnection},
        Connection,
    },
    packets::{
        radio::{
            RadioChannel, SelectRadioChannelPacket, SelectRadioChannelPayload,
            SelectRadioChannelReplyPacket,
        },
        system::{
            GetSystemFlagsPacket, GetSystemFlagsReplyPacket, GetSystemVersionPacket,
            GetSystemVersionReplyPacket, ProductType,
        },
    },
};

use crate::errors::CliError;

pub async fn open_connection() -> miette::Result<SerialConnection> {
    // Find all vex devices on serial ports.
    let devices = serial::find_devices().map_err(CliError::SerialError)?;

    // Open a connection to the device.
    spawn_blocking(move || {
        Ok(devices
            .first()
            .ok_or(CliError::NoDevice)?
            .connect(Duration::from_secs(5))
            .map_err(CliError::SerialError)?)
    })
    .await
    .unwrap()
}

async fn is_connection_wireless(connection: &mut SerialConnection) -> Result<bool, CliError> {
    let version = connection
        .packet_handshake::<GetSystemVersionReplyPacket>(
            Duration::from_millis(500),
            1,
            GetSystemVersionPacket::new(()),
        )
        .await?;
    let system_flags = connection
        .packet_handshake::<GetSystemFlagsReplyPacket>(
            Duration::from_millis(500),
            1,
            GetSystemFlagsPacket::new(()),
        )
        .await?;
    let controller = matches!(version.payload.product_type, ProductType::Controller);

    let tethered = system_flags.payload.flags & (1 << 8) != 0;
    Ok(!tethered && controller)
}

pub async fn switch_radio_channel(
    connection: &mut SerialConnection,
    channel: RadioChannel,
) -> Result<(), CliError> {
    if is_connection_wireless(connection).await? {
        let channel_str = match channel {
            RadioChannel::Download => "download",
            RadioChannel::Pit => "pit",
        };

        info!("Switching radio to {channel_str} channel...");

        // Tell the controller to switch to the download channel.
        connection
            .packet_handshake::<SelectRadioChannelReplyPacket>(
                Duration::from_secs(2),
                3,
                SelectRadioChannelPacket::new(SelectRadioChannelPayload { channel }),
            )
            .await?;

        // Wait for the radio to switch channels before polling the connection
        sleep(Duration::from_millis(250)).await;

        // Poll the connection of the controller to ensure the radio has switched channels.
        let timeout = Duration::from_secs(5);
        select! {
            _ = sleep(timeout) => {
                return Err(CliError::RadioChannelTimeout)
            }
            _ = async {
                while !is_connection_wireless(connection).await.unwrap_or(false) {
                    sleep(Duration::from_millis(250)).await;
                }
            } => {
                info!("Radio successfully switched to {channel_str} channel.");
            }
        }
    }

    Ok(())
}
