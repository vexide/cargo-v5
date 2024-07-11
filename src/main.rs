use std::time::Duration;

use cargo_metadata::camino::Utf8PathBuf;
use cargo_v5::{
    commands::{
        build::{build, objcopy, CargoOpts},
        simulator::launch_simulator,
        upload::{upload, AfterUpload, ProgramIcon},
    },
    errors::CliError,
    metadata::Metadata,
};
use clap::{Parser, Subcommand};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{runtime::Handle, task::block_in_place};
use vex_v5_serial::connection::{serial, Connection};

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
        /// An ELF build artifact to upload.
        #[arg(long)]
        file: Option<Utf8PathBuf>,

        #[arg(long, default_value = "none")]
        after: AfterUpload,

        /// Program slot.
        #[arg(short, long)]
        slot: Option<u8>,

        /// The name of the program.
        #[arg(long)]
        name: Option<String>,

        /// The description of the program.
        #[arg(short, long)]
        description: Option<String>,

        /// The program's file icon.
        #[arg(short, long)]
        icon: Option<ProgramIcon>,

        /// Skip gzip compression before uploading. Will result in longer upload times.
        #[arg(short, long)]
        uncompressed: Option<bool>,

        /// Arguments forwarded to `cargo`.
        #[clap(flatten)]
        cargo_opts: CargoOpts,
    },
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
                            objcopy(&path).await;
                        });
                    });
                }
            })
            .await;
        }
        Command::Upload {
            file,
            after,
            slot,
            name,
            description,
            icon,
            uncompressed,
            cargo_opts,
        } => {
            // We'll use `cargo-metadata` to parse the output of `cargo metadata` and find valid `Cargo.toml`
            // files in the workspace directory.
            let cargo_metadata =
                block_in_place(|| cargo_metadata::MetadataCommand::new().no_deps().exec())
                    .map_err(CliError::CargoMetadata)?;

            // Locate packages with valid v5 metadata fields.
            let package = cargo_metadata
                .packages
                .iter()
                .find(|p| {
                    if let Some(v5_metadata) = p.metadata.get("v5") {
                        v5_metadata.is_object()
                    } else {
                        false
                    }
                })
                .or(cargo_metadata.packages.iter().next())
                .ok_or(CliError::NoManifest)?;

            // Uploading has the option to use the `package.metadata.v5` table for default configuration options.
            // Attempt to serialize `package.metadata.v5` into a [`Metadata`] struct. This will just Default::default to
            // all `None`s if it can't find a specific field, or error if the field is malformed.
            let metadata = Metadata::new(package)?;

            // Get the build artifact we'll be uploading with.
            //
            // The user either directly passed an ELF file through the `--file` argument, or they didn't and we need to run
            // `cargo build`.
            let mut artifact = None;
            if let Some(file) = file {
                // Convert ELF -> BIN using objcopy before we upload.
                artifact = Some(objcopy(&file).await)
            } else {
                // Run cargo build, then objcopy.
                build(&path, cargo_opts, false, |new_artifact| {
                    let mut bin_path = new_artifact.clone();
                    bin_path.set_extension("bin");
                    block_in_place(|| {
                        Handle::current().block_on(async move {
                            objcopy(&new_artifact).await;
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
                .or(metadata.slot)
                .or_else(|| {
                    CustomType::<u8>::new("Choose a program slot to upload to:")
                        .with_validator(|slot: &u8| {
                            Ok(if (1..=8).contains(slot) {
                                Validation::Valid
                            } else {
                                Validation::Invalid(ErrorMessage::Custom(
                                    "Slot out of range".to_string(),
                                ))
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

            // Pass information to the upload routine.
            upload(
                &artifact.ok_or(CliError::NoArtifact)?,
                after,
                slot,
                name.unwrap_or(package.name.clone()), // Fallback to crate name if no name was provided
                description
                    .or(package.description.clone())
                    .unwrap_or("Uploaded with cargo-v5.".to_string()), // Fallback to crate description if no description was provided
                icon.or(metadata.icon).unwrap_or_default(),
                "Rust".to_string(), // `program_type` hardcoded for now, maybe configurable in the future.
                match uncompressed {
                    Some(val) => !val,
                    None => metadata.compress.unwrap_or(true),
                },
            )
            .await?;
        }
        Command::Terminal => {
            // Find all vex devices on serial ports.
            let devices = serial::find_devices().map_err(CliError::ConnectionError)?;

            // Open a connection to the device.
            let mut connection = devices.first()
                .ok_or(CliError::NoDevice)?
                .connect(Duration::from_secs(5))
                .map_err(CliError::ConnectionError)?;

            loop {
                let mut output = [0; 2048];

                if let Ok(size) = connection.read_user(&mut output).await {
                    if size > 0 {
                        print!("{}", std::str::from_utf8(&output).unwrap());
                    }
                }
            }
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
