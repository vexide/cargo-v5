use core::panic;
use std::{env, time::Duration};

use cargo_metadata::camino::Utf8PathBuf;
#[cfg(feature = "field-control")]
use cargo_v5::{commands::field_control::run_field_control_tui, errors::CliError};
use cargo_v5::{
    commands::{
        build::{build, objcopy, CargoOpts},
        new::new,
        simulator::launch_simulator,
        terminal::terminal,
        upload::{upload, AfterUpload, UploadOpts},
    },
    connection::{open_connection, switch_radio_channel},
};
use chrono::Utc;
use clap::{Parser, Subcommand};
use flexi_logger::{AdaptiveFormat, Duplicate, FileSpec, LogfileSelector, LoggerHandle};
use tokio::{runtime::Handle, select, task::block_in_place};
#[cfg(feature = "field-control")]
use vex_v5_serial::connection::serial::{self, SerialConnection, SerialDevice};
use vex_v5_serial::{
    connection::Connection,
    packets::{
        file::{
            FileLoadAction, FileVendor, LoadFileActionPacket, LoadFileActionPayload,
            LoadFileActionReplyPacket,
        },
        radio::{
            RadioChannel, SelectRadioChannelPacket, SelectRadioChannelPayload,
            SelectRadioChannelReplyPacket,
        },
    },
    string::FixedLengthString,
};

cargo_subcommand_metadata::description!("Manage vexide projects");

/// Cargo's CLI arguments
#[derive(Parser, Debug)]
#[clap(name = "cargo", bin_name = "cargo")]
enum Cargo {
    /// Manage vexide projects.
    #[clap(version)]
    V5 {
        #[command(subcommand)]
        command: Command,

        #[arg(long, default_value = ".", global = true)]
        path: Utf8PathBuf,
    },
}

/// A possible `cargo v5` subcommand.
#[derive(Subcommand, Debug)]
enum Command {
    /// Build a project for the V5 brain.
    #[clap(visible_alias = "b")]
    Build {
        /// Build a binary for the WASM simulator instead of the native V5 target.
        #[arg(long, short)]
        simulator: bool,

        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
    /// Build a project and upload it to the V5 brain.
    #[clap(visible_alias = "u")]
    Upload {
        #[arg(long, default_value = "none")]
        after: AfterUpload,

        #[clap(flatten)]
        upload_opts: UploadOpts,
    },
    /// Build, upload, and run a program on the V5 brain, showing its output in the terminal.
    #[clap(visible_alias = "r")]
    Run(UploadOpts),
    /// Access the brain's remote terminal I/O.
    #[clap(visible_alias = "t")]
    Terminal,
    /// Build a project and run it in the simulator.
    Sim {
        #[arg(long)]
        ui: Option<String>,

        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
    /// Run a field control TUI.
    #[cfg(feature = "field-control")]
    #[clap(visible_aliases = ["fc", "comp-control"])]
    FieldControl,
    /// Create a new vexide project with a given name.
    #[clap(visible_alias = "n")]
    New {
        /// The name of the project.
        name: String,
    },
    /// Creates a new vexide project in the current directory
    Init,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Parse CLI arguments
    let Cargo::V5 { command, path } = Cargo::parse();

    let mut logger = flexi_logger::Logger::try_with_env_or_str("info")
        .unwrap()
        .log_to_file(
            FileSpec::default()
                .directory(env::temp_dir())
                .use_timestamp(false)
                .basename(format!(
                    "cargo-v5-{}",
                    Utc::now().format("%Y-%m-%d_%H-%M-%S")
                )),
        )
        .log_to_stdout()
        .duplicate_to_stderr(Duplicate::Warn)
        .adaptive_format_for_stderr(AdaptiveFormat::Default)
        .start()
        .unwrap();

    if let Err(err) = app(command, path, &mut logger).await {
        log::debug!("cargo-v5 is exiting due to an error: {}", err);
        if let Ok(files) = logger.existing_log_files(&LogfileSelector::default()) {
            for file in files {
                eprintln!("A log file is available at {}.", file.display());
            }
        }
        return Err(err);
    }
    Ok(())
}

async fn app(command: Command, path: Utf8PathBuf, logger: &mut LoggerHandle) -> miette::Result<()> {
    match command {
        Command::Build {
            simulator,
            cargo_opts,
        } => {
            build(&path, cargo_opts, simulator, |path| {
                if !simulator {
                    block_in_place(|| {
                        Handle::current().block_on(async move {
                            objcopy(&path).await.unwrap();
                        });
                    });
                }
            })
            .await;
        }
        Command::Upload { upload_opts, after } => {
            upload(&path, upload_opts, after, &mut open_connection().await?).await?;
        }
        Command::Run(opts) => {
            let mut connection = open_connection().await?;

            upload(&path, opts, AfterUpload::Run, &mut connection).await?;

            select! {
                () = terminal(&mut connection, logger) => {}
                _ = tokio::signal::ctrl_c() => {
                    // Quit program
                    _ = connection.packet_handshake::<LoadFileActionReplyPacket>(
                        Duration::from_secs(2),
                        1,
                        LoadFileActionPacket::new(LoadFileActionPayload {
                            vendor: FileVendor::User,
                            action: FileLoadAction::Stop,
                            file_name: FixedLengthString::new(Default::default()).unwrap(),
                        })
                    ).await;

                    // Switch back to pit channel
                    _ = connection
                        .packet_handshake::<SelectRadioChannelReplyPacket>(
                            Duration::from_secs(2),
                            1,
                            SelectRadioChannelPacket::new(SelectRadioChannelPayload {
                                channel: RadioChannel::Pit,
                            }),
                        )
                        .await;

                    std::process::exit(0);
                }
            }
        }
        Command::Terminal => {
            let mut connection = open_connection().await?;
            switch_radio_channel(&mut connection, RadioChannel::Download).await?;
            terminal(&mut connection, logger).await;
        }
        Command::Sim { ui, cargo_opts } => {
            let mut artifact = None;
            build(&path, cargo_opts, true, |new_artifact| {
                artifact = Some(new_artifact);
            })
            .await;
            launch_simulator(
                ui.clone(),
                path.as_ref(),
                artifact
                    .expect("Binary target not found (is this a library?)")
                    .as_ref(),
            )
            .await;
        }
        #[cfg(feature = "field-control")]
        Command::FieldControl => {
            // Not using open_connection since we need to filter for controllers only here.
            let mut connection = {
                let devices = serial::find_devices().map_err(CliError::SerialError)?;

                tokio::task::spawn_blocking::<_, Result<SerialConnection, CliError>>(move || {
                    Ok(devices
                        .into_iter()
                        .find(|device| {
                            matches!(device, SerialDevice::Controller { system_port: _ })
                        })
                        .ok_or(CliError::NoController)?
                        .connect(Duration::from_secs(5))
                        .map_err(CliError::SerialError)?)
                })
                .await
                .unwrap()?
            };

            run_field_control_tui(&mut connection).await?;
        }
        Command::New { name } => {
            new(path, Some(name)).await?;
        }
        Command::Init => {
            new(path, None).await?;
        }
    }

    Ok(())
}
