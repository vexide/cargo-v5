use std::{
    io::{Read, Write},
    sync::mpsc::{self, TryRecvError},
    time::Duration,
};

use flexi_logger::LoggerHandle;
use log::info;
use vex_serial::{
    Connection,
    protocol::{
        FixedString,
        cdc2::file::{FileLoadAction, FileLoadActionPacket, FileVendor},
    },
    serial::{SerialConnection, SerialError},
};

use crate::errors::CliError;

pub fn terminal(
    connection: &mut SerialConnection,
    _logger: &mut LoggerHandle,
) -> Result<(), CliError> {
    info!("Started terminal.");

    // register a signal handler for Ctrl+C interrupts that notifies the main thread through a channel when something comes in.
    let (s, ctrl_c) = mpsc::channel();
    _ = ctrlc::set_handler(move || {
        s.send(()).ok();

        // give a 1s hard deadline for quitting
        std::thread::sleep(Duration::from_secs(1));
        log::error!("ctrlc handler timed out!");
        std::process::exit(1);
    });

    // need to poll stdin on a separate thread since it'd otherwise block the terminal loop
    let (stdin_tx, stdin_rx) = mpsc::channel::<std::io::Result<Vec<u8>>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break, // eof
                Ok(size) => {
                    if stdin_tx.send(Ok(buf[..size].to_vec())).is_err() {
                        break; // exited
                    }
                }
                Err(e) => {
                    let _ = stdin_tx.send(Err(e));
                    break;
                }
            }
        }
    });

    let mut stdout = std::io::stdout().lock();
    let mut program_output = [0; 2048];

    loop {
        if ctrl_c.try_recv().is_ok() {
            // Try to quit program.
            //
            // Don't bother waiting for a response, since the brain could
            // be locked up and prevent the program from exiting.
            _ = connection.send(FileLoadActionPacket {
                vendor: FileVendor::User,
                action: FileLoadAction::Stop,
                file_name: FixedString::default(),
            });

            std::process::exit(0);
        }

        // pipe user port output to stdout
        let size = match connection.read_user(&mut program_output) {
            Ok(size) => size,
            Err(SerialError::IoError(e)) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };

        if size > 0 {
            stdout
                .write_all(&program_output[..size])
                .and_then(|_| stdout.flush())
                .map_err(CliError::IoError)?;
        }

        // pipe stdin to user port input
        loop {
            match stdin_rx.try_recv() {
                Ok(Ok(data)) => match connection.write_user(&data) {
                    Ok(_) => {}
                    Err(SerialError::IoError(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                },
                Ok(Err(e)) => return Err(e.into()),
                Err(TryRecvError::Empty) => break,
                // Stdin hit EOF or the reader thread died.
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // hopefully this stops controller firmware from killing itself
        // std::thread::sleep(Duration::from_millis(10));
    }
}
