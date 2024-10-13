use std::{sync::Arc, time::Duration};

use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use cargo_v5::{
    commands::{
        build::{build, objcopy, CargoOpts},
        simulator::launch_simulator,
        upload::{upload_program, AfterUpload, UploadOpts},
    },
    errors::CliError,
    metadata::Metadata,
};
use clap::{Parser, Subcommand};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{
    io::{stdin, AsyncReadExt}, runtime::Handle, select, spawn, sync::Mutex, task::{block_in_place, spawn_blocking}, time::sleep
};
use vex_v5_serial::connection::{
    serial::{self, SerialConnection},
    Connection,
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

        #[arg(long, default_value = ".")]
        path: Utf8PathBuf,
    },
}

/// A possible `cargo v5` subcommand.
#[derive(Subcommand, Debug)]
enum Command {
    /// Build a project for the V5 brain.
    Build {
        /// Build a binary for the WASM simulator instead of the native V5 target.
        #[arg(long, short)]
        simulator: bool,

        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
    /// Build a project and upload it to the V5 brain.
    Upload {
        #[arg(long, default_value = "none")]
        after: AfterUpload,

        #[clap(flatten)]
        upload_opts: UploadOpts,
    },
    /// Build, upload, and run a program on the V5 brain, showing its output in the terminal.
    Run(UploadOpts),
    /// Access the brain's remote terminal I/O.
    Terminal,
    /// Build a project and run it in the simulator.
    Sim {
        #[arg(long)]
        ui: Option<String>,

        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Parse CLI arguments
    let Cargo::V5 { command, path } = Cargo::parse();

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
            upload(&path, upload_opts, after, false).await?;
        }
        Command::Run(opts) => {
            upload(&path, opts, AfterUpload::Run, true).await?;
        }
        Command::Terminal => {
            terminal(open_connection().await?).await;
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
    }

    Ok(())
}

async fn open_connection() -> miette::Result<SerialConnection> {
    // Find all vex devices on serial ports.
    let devices = serial::find_devices().map_err(CliError::SerialError)?;

    // Open a connection to the device.
    spawn_blocking(move || {
        Ok(devices
            .first()
            .ok_or(CliError::NoDevice)?
            .connect(Duration::from_secs(5))
            .map_err(CliError::SerialError)?)
    })
    .await
    .unwrap()
}

async fn upload(
    path: &Utf8Path,
    UploadOpts {
        file,
        slot,
        name,
        description,
        icon,
        uncompressed,
        cargo_opts,
    }: UploadOpts,
    after: AfterUpload,
    then_terminal: bool,
) -> miette::Result<()> {
    // We'll use `cargo-metadata` to parse the output of `cargo metadata` and find valid `Cargo.toml`
    // files in the workspace directory.
    let cargo_metadata =
        block_in_place(|| cargo_metadata::MetadataCommand::new().no_deps().exec()).ok();

    // Locate packages with valid v5 metadata fields.
    let package = cargo_metadata.and_then(|metadata| {
        metadata
            .packages
            .iter()
            .find(|p| {
                if let Some(v5_metadata) = p.metadata.get("v5") {
                    v5_metadata.is_object()
                } else {
                    false
                }
            })
            .cloned()
            .or(metadata.packages.first().cloned())
    });

    // Uploading has the option to use the `package.metadata.v5` table for default configuration options.
    // Attempt to serialize `package.metadata.v5` into a [`Metadata`] struct. This will just Default::default to
    // all `None`s if it can't find a specific field, or error if the field is malformed.
    let metadata = if let Some(ref package) = package {
        Some(Metadata::new(package)?)
    } else {
        None
    };

    // Get the build artifact we'll be uploading with.
    //
    // The user either directly passed an file through the `--file` argument, or they didn't and we need to run
    // `cargo build`.
    let mut artifact = None;
    if let Some(file) = file {
        if file.extension() == Some("bin") {
            artifact = Some(file);
        } else {
            // If a BIN file wasn't provided, we'll attempt to objcopy it as if it were an ELF.
            artifact = Some(objcopy(&file).await?);
        }
    } else {
        // Run cargo build, then objcopy.
        build(path, cargo_opts, false, |new_artifact| {
            let mut bin_path = new_artifact.clone();
            bin_path.set_extension("bin");
            block_in_place(|| {
                Handle::current().block_on(async move {
                    objcopy(&new_artifact).await.unwrap();
                });
            });
            artifact = Some(bin_path);
        })
        .await;
    }

    // The program's slot number is absolutely required for uploading. If the slot argument isn't directly provided:
    //
    // - Check for the `package.metadata.v5.slot` field in Cargo.toml.
    // - If that doesn't exist, directly prompt the user asking what slot to upload to.
    let slot = slot
        .or(metadata.and_then(|m| m.slot))
        .or_else(|| {
            CustomType::<u8>::new("Choose a program slot to upload to:")
                .with_validator(|slot: &u8| {
                    Ok(if (1..=8).contains(slot) {
                        Validation::Valid
                    } else {
                        Validation::Invalid(ErrorMessage::Custom("Slot out of range".to_string()))
                    })
                })
                .with_help_message("Type a slot number from 1 to 8, inclusive")
                .prompt()
                .ok()
        })
        .ok_or(CliError::NoSlot)?;

    // Ensure [1, 8] range bounds for slot number
    if !(1..8).contains(&slot) {
        Err(CliError::SlotOutOfRange)?;
    }

    let mut connection = open_connection().await?;

    // Pass information to the upload routine.
    upload_program(
        &mut connection,
        &artifact.ok_or(CliError::NoArtifact)?,
        after,
        slot,
        name.or(package.as_ref().map(|pkg| pkg.name.clone()))
            .unwrap_or("cargo-v5".to_string()),
        description
            .or(package.as_ref().and_then(|pkg| pkg.description.clone()))
            .unwrap_or("Uploaded with cargo-v5.".to_string()),
        icon.or(metadata.and_then(|metadata| metadata.icon))
            .unwrap_or_default(),
        "Rust".to_string(), // `program_type` hardcoded for now, maybe configurable in the future.
        match uncompressed {
            Some(val) => !val,
            None => metadata
                .and_then(|metadata| metadata.compress)
                .unwrap_or(true),
        },
    )
    .await?;

    if then_terminal {
        println!();
        terminal(connection).await;
    }

    Ok(())
}

async fn terminal(mut connection: SerialConnection) -> ! {
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
