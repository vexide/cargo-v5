use std::time::Duration;

use flexi_logger::{LogSpecification, LoggerHandle};
use log::info;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, stdin, stdout},
    select,
    time::sleep,
};
use vex_v5_serial::{Connection, serial::SerialConnection};

pub async fn terminal(connection: &mut SerialConnection, logger: &mut LoggerHandle) -> ! {
    info!("Started terminal.");

    logger.push_temp_spec(LogSpecification::off());

    let mut stdin = stdin();
    let mut program_output = [0; 2048];
    let mut program_input = [0; 4096];

    loop {
        select! {
            read = connection.read_user(&mut program_output) => {
                if let Ok(size) = read {
                    stdout().write_all(&program_output[..size]).await.unwrap();
                }
            },
            read = stdin.read(&mut program_input) => {
                if let Ok(size) = read {
                    connection.write_user(&program_input[..size]).await.unwrap();
                }
            }
        }

        sleep(Duration::from_millis(10)).await;
    }
}
