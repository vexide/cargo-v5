use std::io::{self, Write};
use std::time::Duration;

use vex_v5_serial::connection::{serial::SerialConnection, Connection};

use tabwriter::TabWriter;
use vex_v5_serial::packets::device::{GetDeviceStatusPacket, GetDeviceStatusReplyPacket};

use crate::errors::CliError;

pub async fn devices(connection: &mut SerialConnection) -> Result<(), CliError> {
    let mut tw = TabWriter::new(io::stdout());

    let status = connection
        .packet_handshake::<GetDeviceStatusReplyPacket>(
            Duration::from_millis(500),
            10,
            GetDeviceStatusPacket::new(()),
        )
        .await?
        .try_into_inner()?;
    write!(
        &mut tw,
        "\x1B[1mPort\tType\tStatus\tFirmware\tBootloader\x1B[0m\n"
    )
    .unwrap();

    for device in status.devices {
        write!(
            &mut tw,
            "{}\t{:?}\t{:#x}\t{}\t{}\n",
            device.port,
            device.device_type,
            device.status,
            format!(
                "{}.{}.{}.b{}",
                (u32::from(device.version) >> 14) as u8,
                ((u32::from(device.version) << 18) >> 26) as u8,
                (device.version & 0xff) as u8,
                device.beta_version
            ),
            format!(
                "{}.{}.{}",
                (u32::from(device.boot_version) >> 14) as u8,
                ((u32::from(device.boot_version) << 18) >> 26) as u8,
                (device.boot_version & 0xff) as u8
            ),
        )
        .unwrap();
    }

    tw.flush().unwrap();

    Ok(())
}
