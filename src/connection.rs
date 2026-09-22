use core::fmt;
use inquire::Select;
use log::info;
use std::time::Duration;
use tokio::{task::spawn_blocking, time::sleep};
use vex_v5_serial::{
    Connection,
    protocol::{
        cdc::{ProductType, SystemVersionPacket},
        cdc2::{
            file::{FileControlGroup, FileControlPacket, RadioChannel},
            system::{RadioStatusPacket, SystemFlagsPacket},
        },
    },
    serial::{
        self, AIM_USB_PID, AIR_CONTROLLER_USB_PID, AIR_HORNET_USB_PID, EXP_BRAIN_USB_PID,
        SerialConnection, SerialDevice, V5_BRAIN_USB_PID, V5_CONTROLLER_USB_PID,
    },
};

use crate::errors::CliError;

fn pid_to_product_name(pid: u16) -> &'static str {
    match pid {
        V5_BRAIN_USB_PID => "V5 Brain",
        EXP_BRAIN_USB_PID => "EXP Brain",
        V5_CONTROLLER_USB_PID => "V5 Controller",
        AIR_HORNET_USB_PID => "AIR Hornet",
        AIR_CONTROLLER_USB_PID => "AIR Controller",
        AIM_USB_PID => "AIM Coding Robot",
        _ => "<unknown>",
    }
}

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
                    let serial::SerialPortType::UsbPort(usb_info) =
                        &self.inner.system_port().port_info.port_type
                    else {
                        unreachable!(); // vex-v5-serial already filters for USB ports only.
                    };

                    write!(
                        f,
                        "{} on {}",
                        pid_to_product_name(usb_info.pid),
                        self.inner.system_port().port_info.port_name
                    )?;
                    if let Some(user) = self.inner.user_port() {
                        write!(f, ", {}", user.port_info.port_name)?;
                    }

                    Ok(())
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
        .handshake(SystemVersionPacket {}, Duration::from_millis(500), 1)
        .await?;
    let system_flags = connection
        .handshake(SystemFlagsPacket {}, Duration::from_millis(500), 1)
        .await??;
    let is_controller = matches!(
        version.product_type,
        ProductType::V5Controller | ProductType::ExpController | ProductType::ExpControllerVariant // | ProductType::AirController
    );

    let tethered = system_flags.flags & (1 << 8) != 0;
    Ok(!tethered && is_controller)
}

pub async fn switch_to_download_channel(connection: &mut SerialConnection) -> Result<(), CliError> {
    let Ok(radio_status) = connection
        .handshake(RadioStatusPacket {}, Duration::from_secs(2), 3)
        .await?
    else {
        return Ok(()); // likely unsupported
    };

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
            .handshake(
                FileControlPacket {
                    group: FileControlGroup::Radio(RadioChannel::Download),
                },
                Duration::from_secs(2),
                3,
            )
            .await??;

        // Wait for the controller to disconnect by spamming it with a packet and waiting until that packet
        // doesn't go through. This indicates that the radio has actually started to switch channels.
        tokio::time::timeout(Duration::from_secs(8), async {
            while connection
                .handshake(RadioStatusPacket {}, Duration::from_millis(250), 0)
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
                    .handshake(RadioStatusPacket {}, Duration::from_millis(250), 0)
                    .await
                else {
                    continue;
                };

                match pkt {
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
