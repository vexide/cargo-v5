use log::info;
use std::time::Duration;
use tokio::{select, task::spawn_blocking, time::sleep};
use vex_v5_serial::{
    connection::{
        serial::{self, SerialConnection, SerialError},
        Connection,
    },
    packets::{
        radio::{
            GetRadioStatusPacket, GetRadioStatusReplyPacket, RadioChannel,
            SelectRadioChannelPacket, SelectRadioChannelPayload, SelectRadioChannelReplyPacket,
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
        .await?
        .try_into_inner()?;
    let controller = matches!(version.payload.product_type, ProductType::Controller);

    let tethered = system_flags.flags & (1 << 8) != 0;
    Ok(!tethered && controller)
}

pub async fn switch_radio_channel(
    connection: &mut SerialConnection,
    channel: RadioChannel,
) -> Result<(), CliError> {
    let radio_status = connection
        .packet_handshake::<GetRadioStatusReplyPacket>(
            Duration::from_secs(2),
            3,
            GetRadioStatusPacket::new(()),
        )
        .await?
        .try_into_inner()?;

    log::debug!("Radio channel: {}", radio_status.channel);

    // Return early if already in download channel.
    // TODO: Make this also detect the bluetooth radio channel
    if (radio_status.channel == 5 && channel == RadioChannel::Download)
        || (radio_status.channel == 31 && channel == RadioChannel::Pit)
        || (radio_status.channel == -11)
    {
        return Ok(());
    }

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
            .await?
            .try_into_inner()?;

        // Wait for the controller to disconnect by spamming it with a packet and waiting until that packet
        // doesn't go through. This indicates that the radio has actually started to switch channels.
        select! {
            _ = sleep(Duration::from_secs(8)) => {
                return Err(CliError::RadioChannelDisconnectTimeout)
            }
            _ = async {
                while connection
                    .packet_handshake::<GetRadioStatusReplyPacket>(
                        Duration::from_millis(250),
                        1,
                        GetRadioStatusPacket::new(())
                    )
                    .await
                    .is_ok()
                {
                    sleep(Duration::from_millis(250)).await;
                }
            } => {}
        }

        // Poll the connection of the controller to ensure the radio has switched channels by sending
        // test packets every 250ms for 8 seconds until we get a successful reply, indicating that the
        // controller has reconnected.
        //
        // If the controller doesn't a reply within 8 seconds, it hasn't reconnected correctly.
        connection
            .packet_handshake::<GetRadioStatusReplyPacket>(
                Duration::from_millis(250),
                32,
                GetRadioStatusPacket::new(()),
            )
            .await
            .map_err(|err| match err {
                SerialError::Timeout => CliError::RadioChannelReconnectTimeout,
                other => CliError::SerialError(other),
            })?;
    }

    Ok(())
}
