use cargo_v5::{
    commands::{
        build::{CargoOpts, build},
        cat::cat,
        devices::devices,
        dir::dir,
        key_value::{kv_get, kv_set},
        log::log,
        new::new,
        rm::rm,
        screenshot::screenshot,
        terminal::terminal,
        migrate,
        upload::{AfterUpload, UploadOpts, upload},
    },
    connection::{open_connection, switch_to_download_channel},
    errors::CliError,
    self_update::{self, SelfUpdateMode},
};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use flexi_logger::{AdaptiveFormat, FileSpec, LogfileSelector, LoggerHandle};
use std::{env, num::NonZeroU32, panic, path::PathBuf};
use vex_v5_serial::{
    Connection,
    protocol::{
        FixedString,
        cdc2::file::{FileLoadAction, FileLoadActionPacket, FileLoadActionPayload, FileVendor},
    },
    serial::{self, SerialConnection, SerialDevice},
};

#[cfg(feature = "field-control")]
use cargo_v5::commands::field_control::run_field_control_tui;
#[cfg(feature = "field-control")]
use std::time::Duration;
#[cfg(feature = "field-control")]

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
        path: PathBuf,
    },
}

/// Access a Brain's system key/value configuration.
#[derive(Subcommand, Debug)]
#[clap(name = "kv")]
enum KeyValue {
    /// Get the value of a system variable on a Brain.
    Get { key: String },

    /// Set a system variable on a Brain.
    Set { key: String, value: String },
}

/// A possible `cargo v5` subcommand.
#[derive(Subcommand, Debug)]
enum Command {
    /// Build a project for the V5 Brain.
    #[clap(visible_alias = "b")]
    Build {
        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
    
    /// Upload a project or file to a Brain.
    #[clap(visible_alias = "u")]
    Upload {
        #[arg(long, default_value = "none")]
        after: AfterUpload,

        #[clap(flatten)]
        upload_opts: UploadOpts,
    },
    
    /// Access a Brain's remote terminal I/O.
    #[clap(visible_alias = "t")]
    Terminal,
    
    /// Build, upload, and run a program on a V5 Brain, showing its output in the terminal.
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
    
    /// Create a new vexide project in the current directory.
    Init {
        #[clap(flatten)]
        download_opts: DownloadOpts,
    },
    
    /// List files on flash.
    #[clap(visible_alias = "ls")]
    Dir,
    
    /// Read a file from flash, then write its contents to stdout.
    Cat {
        file: PathBuf,
    },

    /// Erase a file from flash.
    Rm {
        file: PathBuf,
    },
    
    /// Read a Brain's event log.
    Log {
        #[arg(long, short, default_value = "1")]
        page: NonZeroU32,
    },
    
    /// List devices connected to a Brain.
    #[clap(visible_alias = "lsdev")]
    Devices,

    /// Take a screen capture of the brain, saving the file to the current directory.
    #[clap(visible_alias = "sc")]
    Screenshot,
    
    /// Access a Brain's system key/value configuration.
    #[command(subcommand, visible_alias = "kv")]
    KeyValue(KeyValue),
    
    /// Run a field control TUI.
    #[cfg(feature = "field-control")]
    #[clap(visible_aliases = ["fc", "comp-control"])]
    FieldControl,
    
    /// Update cargo-v5 to the latest version.
    #[clap(hide = matches!(*self_update::CURRENT_MODE, SelfUpdateMode::Unmanaged(_)))]
    SelfUpdate,

    /// Migrate an older project to vexide 0.8.0.
    Migrate,
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
        log::debug!("cargo-v5 is exiting due to an error: {err}");
        if let Ok(files) = logger.existing_log_files(&LogfileSelector::default()) {
            for file in files {
                eprintln!("A log file is available at {}.", file.display());
            }
        }
        return Err(err);
    }
    Ok(())
}

async fn app(command: Command, path: PathBuf, logger: &mut LoggerHandle) -> miette::Result<()> {
    match command {
        Command::Build { cargo_opts } => {
            build(&path, cargo_opts).await?;
        }
        Command::Upload { upload_opts, after } => {
            upload(&path, upload_opts, after).await?;
        }
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
                    _ = connection.send(
                        FileLoadActionPacket::new(FileLoadActionPayload {
                            vendor: FileVendor::User,
                            action: FileLoadAction::Stop,
                            file_name: FixedString::default(),
                        })
                    ).await;

                    std::process::exit(0);
                }
            }
        }
        Command::KeyValue(subcommand) => {
            let mut connection = open_connection().await?;
            match subcommand {
                KeyValue::Get { key } => {
                    println!("{}", kv_get(&mut connection, &key).await?);
                }
                KeyValue::Set { key, value } => {
                    kv_set(&mut connection, &key, &value).await?;
                    println!("{key} = {}", kv_get(&mut connection, &key).await?);
                }
            }
        }
        Command::Terminal => {
            let mut connection = open_connection().await?;
            switch_to_download_channel(&mut connection).await?;
            terminal(&mut connection, logger).await;
        }
        #[cfg(feature = "field-control")]
        Command::FieldControl => {
            // Not using open_connection since we need to filter for controllers only here.
            let mut connection = {
                let devices = serial::find_devices().map_err(CliError::SerialError)?;

                tokio::task::spawn_blocking::<_, Result<SerialConnection, CliError>>(move || {
                    devices
                        .into_iter()
                        .find(|device| {
                            matches!(device, SerialDevice::Controller { system_port: _ })
                        })
                        .ok_or(CliError::NoController)?
                        .connect(Duration::from_secs(5))
                        .map_err(CliError::SerialError)
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
        Command::SelfUpdate => {
            self_update::self_update().await?;
        }
        Command::Migrate => {
            migrate::migrate_workspace(&path).await?;
        }
    }

    Ok(())
}
