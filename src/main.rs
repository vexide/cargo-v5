use core::panic;
use std::{env, num::NonZeroU32, path::PathBuf, time::Duration};

use cargo_metadata::camino::Utf8PathBuf;
#[cfg(feature = "field-control")]
use cargo_v5::{commands::field_control::run_field_control_tui, errors::CliError};
use cargo_v5::{
    commands::{
        build::{build, CargoOpts},
        cat::cat,
        devices::devices,
        dir::dir,
        log::log,
        new::new,
        rm::rm,
        screenshot::screenshot,
        terminal::terminal,
        upload::{upload, AfterUpload, UploadOpts},
    },
    connection::{open_connection, switch_radio_channel},
};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use flexi_logger::{AdaptiveFormat, FileSpec, LogfileSelector, LoggerHandle};
#[cfg(feature = "field-control")]
use vex_v5_serial::connection::serial::{self, SerialConnection, SerialDevice};
use vex_v5_serial::{
    connection::Connection,
    packets::{
        file::{
            FileLoadAction, FileVendor, LoadFileActionPacket, LoadFileActionPayload,
            LoadFileActionReplyPacket,
        },
        radio::RadioChannel,
    },
    string::FixedString,
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
    /// Access the brain's remote terminal I/O.
    #[clap(visible_alias = "t")]
    Terminal,
    /// Build, upload, and run a program on the V5 brain, showing its output in the terminal.
    #[clap(visible_alias = "r")]
    Run(UploadOpts),
    /// Create a new vexide project with a given name.
    #[clap(visible_alias = "n")]
    New {
        /// The name of the project.
        name: String,

        #[clap(flatten)]
        download_opts: DownloadOpts,
    },
    /// Creates a new vexide project in the current directory
    Init {
        #[clap(flatten)]
        download_opts: DownloadOpts,
    },
    /// List files on flash.
    #[clap(visible_alias = "ls")]
    Dir,
    /// Read a file from flash, then write its contents to stdout.
    Cat { file: PathBuf },
    /// Erase a file from flash.
    Rm { file: PathBuf },
    /// Read event log.
    Log {
        #[arg(long, short, default_value = "1")]
        page: NonZeroU32,
    },
    /// List devices connected to a brain.
    #[clap(visible_alias = "lsdev")]
    Devices,
    /// Take a screen capture of the brain, saving the file to the current directory.
    #[clap(visible_alias = "sc")]
    Screenshot,
    /// Run a field control TUI.
    #[cfg(feature = "field-control")]
    #[clap(visible_aliases = ["fc", "comp-control"])]
    FieldControl,
}

#[derive(Args, Debug)]
struct DownloadOpts {
    /// Do not download the latest template online.
    #[cfg_attr(feature = "fetch-template", arg(long, default_value = "false"))]
    #[cfg_attr(not(feature = "fetch-template"), arg(skip = false))]
    offline: bool,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Parse CLI arguments
    let Cargo::V5 { command, path } = Cargo::parse();

    let mut logger = flexi_logger::Logger::try_with_env()
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
        .log_to_stderr()
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
        Command::Build { cargo_opts } => build(&path, cargo_opts).await?,
        Command::Upload { upload_opts, after } => upload(&path, upload_opts, after).await?,
        Command::Dir => dir(&mut open_connection().await?).await?,
        Command::Devices => devices(&mut open_connection().await?).await?,
        Command::Cat { file } => cat(&mut open_connection().await?, file).await?,
        Command::Rm { file } => rm(&mut open_connection().await?, file).await?,
        Command::Log { page } => log(&mut open_connection().await?, page).await?,
        Command::Screenshot => screenshot(&mut open_connection().await?).await?,
        Command::Run(opts) => {
            let mut connection = upload(&path, opts, AfterUpload::Run).await?;

            tokio::select! {
                () = terminal(&mut connection, logger) => {}
                _ = tokio::signal::ctrl_c() => {
                    // Try to quit program.
                    //
                    // Don't bother waiting for a response, since the brain could
                    // be locked up and prevent the program from exiting.
                    _ = connection.send_packet(
                        LoadFileActionPacket::new(LoadFileActionPayload {
                            vendor: FileVendor::User,
                            action: FileLoadAction::Stop,
                            file_name: FixedString::new(Default::default()).unwrap(),
                        })
                    ).await;

                    std::process::exit(0);
                }
            }
        }
        Command::Terminal => {
            let mut connection = open_connection().await?;
            switch_radio_channel(&mut connection, RadioChannel::Download).await?;
            terminal(&mut connection, logger).await;
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
        Command::New {
            name,
            download_opts,
        } => {
            new(path, Some(name), !download_opts.offline).await?;
        }
        Command::Init { download_opts } => {
            new(path, None, !download_opts.offline).await?;
        }
    }

    Ok(())
}
