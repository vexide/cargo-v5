use std::time::Duration;

use flexi_logger::LoggerHandle;
use futures_util::FutureExt;
use log::info;
use smol::{
    Timer, Unblock,
    io::{AsyncReadExt, AsyncWriteExt},
};
use vex_serial::{Connection, serial::SerialConnection};

use crate::errors::CliError;

pub fn terminal(
    connection: &mut SerialConnection,
    _logger: &mut LoggerHandle,
) -> Result<(), CliError> {
    info!("Started terminal.");

    // logger.push_temp_spec(LogSpecification::off());

    let mut stdin = Unblock::new(std::io::stdin());
    let mut stdout = Unblock::new(std::io::stdout());
    let mut program_input = [0; 4096];
    let mut program_output = [0; 2048];

    loop {
        futures_util::select! {
            read = connection.read_user(&mut program_output).fuse() => {
                match read {
                    Ok(size) => {
                        stdout.write_all(&program_output[..size]).await.unwrap();
                        stdout.flush().await.unwrap();
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            read = stdin.read(&mut program_input).fuse() => {
                match read {
                    Ok(size) => match connection.write_user(&program_input[..size]).await {
                        Ok(_) => {}
                        Err(e) => return Err(e.into()),
                    },
                    Err(e) => return Err(e.into()),
                }
            }
        }
        // // done so controller firmware (hopefully) doesn't kill itself
        // Timer::after(Duration::from_millis(10)).await;
    }
}
