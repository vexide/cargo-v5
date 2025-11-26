use core::fmt;
use inquire::Select;
use log::info;
use std::time::Duration;
use tokio::{task::spawn_blocking, time::sleep};
use vex_v5_serial::{
    Connection,
    protocol::{
        cdc::{ProductType, SystemVersionPacket, SystemVersionReplyPacket},
        cdc2::{
            file::{FileControlGroup, FileControlPacket, FileControlReplyPacket, RadioChannel},
            system::{
                RadioStatusPacket, RadioStatusReplyPacket, SystemFlagsPacket,
                SystemFlagsReplyPacket,
            },
        },
    },
    serial::{self, SerialConnection, SerialDevice},
};

use crate::errors::CliError;

pub async fn open_connection() -> Result<SerialConnection, CliError> {
    // Find all vex devices on serial ports.
    let devices = serial::find_devices().map_err(CliError::SerialError)?;

    let device = match devices.len() {
        // No devices connected
        0 => return Err(CliError::NoDevice),

        // Exactly one device connected. Choose that one automatically.
        1 => devices.into_iter().next().unwrap(),

        // Multiple devices connected at once. Prompt the user asking which one they want.
        _ => {
            /// Wrapper around SerialDevice to provide a Display implementation for the prompt choices.
            struct SerialDeviceChoice {
                inner: SerialDevice,
            }

            impl fmt::Display for SerialDeviceChoice {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match &self.inner {
                        SerialDevice::Brain {
                            user_port,
                            system_port,
                        } => {
                            write!(f, "Brain on {user_port}, {system_port}")
                        }
                        SerialDevice::Controller { system_port } => {
                            write!(f, "Controller on {system_port}")
                        }
                        SerialDevice::Unknown { system_port } => {
                            write!(f, "<unknown> on {system_port}")
                        }
                    }
                }
            }

            Select::new(
                "Choose a device to connect to",
                devices
                    .into_iter()
                    .map(|device| SerialDeviceChoice { inner: device })
                    .collect::<Vec<_>>(),
            )
            .prompt()?
            .inner
        }
    };

    // Open a connection to the device.
    spawn_blocking(move || {
        device
            .connect(Duration::from_secs(5))
            .map_err(CliError::SerialError)
    })
    .await
    .unwrap()
}

async fn is_connection_wireless(connection: &mut SerialConnection) -> Result<bool, CliError> {
    let version = connection
        .handshake::<SystemVersionReplyPacket>(
            Duration::from_millis(500),
            1,
            SystemVersionPacket::new(()),
        )
        .await?;
    let system_flags = connection
        .handshake::<SystemFlagsReplyPacket>(
            Duration::from_millis(500),
            1,
            SystemFlagsPacket::new(()),
        )
        .await?
        .payload?;
    let controller = matches!(version.payload.product_type, ProductType::Controller);

    let tethered = system_flags.flags & (1 << 8) != 0;
    Ok(!tethered && controller)
}

pub async fn switch_to_download_channel(connection: &mut SerialConnection) -> Result<(), CliError> {
    let radio_status = connection
        .handshake::<RadioStatusReplyPacket>(Duration::from_secs(2), 3, RadioStatusPacket::new(()))
        .await?
        .payload?;

    log::debug!("Radio channel: {}", radio_status.channel);

    match radio_status.channel {
        // 9 = Repairing/stuck.
        //
        // Usually happens when a CDC connection is established while the controller is
        // still trying to pair with the brain. In this state, the controller is stuck
        // and won't respond to FILE_CTRL packets, so we return an error and instruct the
        // user to power cycle.
        9 => return Err(CliError::RadioChannelStuck),

        // 5: Already in download.
        // 245: Bluetooth (there is no download channel).
        5 | 245 => return Ok(()),

        // Pit has a wide variety of channel identifiers that we really don't care about.
        _ => {}
    }

    if is_connection_wireless(connection).await? {
        info!("Switching radio to download channel...");

        // Tell the controller to switch to the download channel.
        connection
            .handshake::<FileControlReplyPacket>(
                Duration::from_secs(2),
                3,
                FileControlPacket::new(FileControlGroup::Radio(RadioChannel::Download)),
            )
            .await?
            .payload?;

        // Wait for the controller to disconnect by spamming it with a packet and waiting until that packet
        // doesn't go through. This indicates that the radio has actually started to switch channels.
        tokio::time::timeout(Duration::from_secs(8), async {
            while connection
                .handshake::<RadioStatusReplyPacket>(
                    Duration::from_millis(250),
                    0,
                    RadioStatusPacket::new(()),
                )
                .await
                .is_ok()
            {
                sleep(Duration::from_millis(250)).await;
            }
        })
        .await
        .map_err(|_| CliError::RadioChannelReconnectTimeout)?;

        // Poll the connection of the controller to ensure the radio has switched channels by sending
        // test packets every 250ms for 8 seconds until we get a successful reply, indicating that the
        // controller has reconnected.
        //
        // If the controller doesn't a reply within 8 seconds, it's probably frozen and hasn't reconnected
        // correctly.
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let Ok(pkt) = connection
                    .handshake::<RadioStatusReplyPacket>(
                        Duration::from_millis(250),
                        0,
                        RadioStatusPacket::new(()),
                    )
                    .await
                else {
                    continue;
                };

                match pkt.payload {
                    // We have successfully switched to the download channel.
                    Ok(payload) if payload.channel == 5 => return Ok(()),

                    // The radio/controller reconnected, but failed to report its status.
                    Err(error) => return Err(CliError::Nack(error)),

                    // Still reconnecting.
                    _ => {
                        sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                }
            }
        })
        .await
        .map_err(|_| CliError::RadioChannelReconnectTimeout)??;
    }

    Ok(())
}
