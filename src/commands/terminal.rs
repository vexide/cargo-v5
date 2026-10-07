use std::time::Duration;

use flexi_logger::{LogSpecification, LoggerHandle};
use futures_util::FutureExt;
use log::info;
use smol::{
    Timer, Unblock,
    io::{AsyncReadExt, AsyncWriteExt},
};
use vex_v5_serial::{Connection, serial::SerialConnection};

pub async fn terminal(connection: &mut SerialConnection, logger: &mut LoggerHandle) -> ! {
    info!("Started terminal.");

    logger.push_temp_spec(LogSpecification::off());

    let mut stdin = Unblock::new(std::io::stdin());
    let mut program_output = [0; 2048];
    let mut program_input = [0; 4096];

    let mut stdout = Unblock::new(std::io::stdout());

    loop {
        futures_util::select! {
            read = connection.read_user(&mut program_output).fuse() => {
                if let Ok(size) = read {
                    stdout.write_all(&program_output[..size]).await.unwrap();
                }
            },
            read = stdin.read(&mut program_input).fuse() => {
                if let Ok(size) = read {
                    connection.write_user(&program_input[..size]).await.unwrap();
                }
            }
        }

        Timer::after(Duration::from_millis(10)).await;
    }
}
