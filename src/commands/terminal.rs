use std::time::Duration;

use flexi_logger::{LogSpecification, LoggerHandle};
use log::info;
use tokio::{
    io::{stdin, AsyncReadExt},
    select,
    time::sleep,
};
use vex_v5_serial::connection::{serial::SerialConnection, Connection};

pub async fn terminal(connection: &mut SerialConnection, logger: &mut LoggerHandle) -> ! {
    info!("Started terminal.");

    logger.push_temp_spec(LogSpecification::off());

    let mut stdin = stdin();

    loop {
        let mut program_output = [0; 1024];
        let mut program_input = [0; 1024];
        select! {
            read = connection.read_user(&mut program_output) => {
                if let Ok(size) = read {
                    print!("{}", std::str::from_utf8(&program_output[..size]).unwrap());
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
