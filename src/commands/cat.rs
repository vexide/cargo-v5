use std::{io::Write, path::PathBuf, str::FromStr};

use vex_serial::{
    protocol::{
        FixedString,
        cdc2::file::{FileTransferTarget, FileVendor},
    },
    serial::{SerialConnection, SerialError},
};

use vex_serial_util::file::blocking::download_file;

use crate::errors::CliError;

pub const KITTY: &str = r#"/\_/\
>^,^< ♡ meow!
 / \
(___)_/
"#;

pub fn vendor_from_prefix(prefix: &str) -> FileVendor {
    match prefix {
        "user" | "/user" => FileVendor::User,
        "sys_" | "/sys_" => FileVendor::Sys,
        "rmsh" | "/rmsh" => FileVendor::Dev1,
        "pros" | "/pros" => FileVendor::Dev2,
        "mwrk" | "/mwrk" => FileVendor::Dev3,
        "deva" | "/deva" => FileVendor::Dev4,
        "devb" | "/devb" => FileVendor::Dev5,
        "devc" | "/devc" => FileVendor::Dev6,
        "vxvm" | "/vxvm" => FileVendor::VexVm,
        "vex_" | "/vex_" => FileVendor::Vex,
        _ => FileVendor::Undefined,
    }
}

pub fn cat(connection: &mut SerialConnection, file: PathBuf) -> Result<(), CliError> {
    let vendor = if let Some(parent) = file.parent() {
        vendor_from_prefix(parent.to_str().unwrap())
    } else {
        FileVendor::Undefined
    };

    let file_name = FixedString::from_str(file.file_name().unwrap_or_default().to_str().unwrap())
        .map_err(|err| CliError::SerialError(SerialError::FixedStringSizeError(err)))?;

    std::io::stdout().write_all(&download_file(
        connection,
        file_name,
        // This field just sets a cap on how many chunks the file transfer will
        // return, so we just use the largest possible transfer size rather than
        // the exact size of the file.
        u32::MAX,
        vendor,
        FileTransferTarget::Qspi,
        0x0,
        None::<fn(f32)>,
    )?)?;

    Ok(())
}
