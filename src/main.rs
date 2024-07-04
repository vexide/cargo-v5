use anyhow::Context;
use cargo_metadata::camino::Utf8PathBuf;
use cargo_v5::{
    commands::{
        build::{build, objcopy, CargoOpts},
        simulator::launch_simulator,
        upload::{upload, AfterUpload, ProgramIcon},
    },
    manifest::Manifest,
};
use clap::{Parser, Subcommand};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{runtime::Handle, task::block_in_place};

cargo_subcommand_metadata::description!("Manage vexide projects");

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

#[derive(Subcommand, Debug)]
enum Command {
    /// Build a project for the V5 brain.
    Build {
        /// Build a binary for the WASM simulator instead of the native V5 target.
        #[arg(long, short)]
        simulator: bool,
        #[clap(flatten)]
        opts: CargoOpts,
    },
    /// Build and upload a vexide project to the V5 brain.
    Upload {
        /// An ELF file to upload.
        #[arg(long)]
        file: Option<Utf8PathBuf>,

        #[arg(long, default_value = "none")]
        after: AfterUpload,

        /// Program slot
        #[arg(short, long)]
        slot: Option<u8>,

        /// The name of the program
        #[arg(long)]
        name: Option<String>,

        /// The description of the program
        #[arg(short, long)]
        description: Option<String>,

        /// The icon to appear on the program
        #[arg(short, long)]
        icon: Option<ProgramIcon>,

        /// Whether or not the program should be compressed before uploading
        #[arg(short, long)]
        uncompressed: Option<bool>,

        #[clap(flatten)]
        build_opts: CargoOpts,
    },
    /// Build a project and run it in the simulator.
    Sim {
        #[arg(long)]
        ui: Option<String>,
        #[clap(flatten)]
        opts: CargoOpts,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Prints the path of the configuration file.
    Print,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cargo::V5 { command, path } = Cargo::parse();

    match command {
        Command::Build { simulator, opts } => {
            build(&path, opts, simulator, |path| {
                if !simulator {
                    block_in_place(|| {
                        Handle::current().block_on(async move {
                            objcopy(&path).await;
                        });
                    });
                }
            }).await;
        }
        Command::Upload {
            file,
            after,
            slot,
            name,
            description,
            icon,
            uncompressed,
            build_opts,
        } => {
            let metadata = block_in_place(|| {
                cargo_metadata::MetadataCommand::new().no_deps().exec()
            })?;

            let package = metadata
                .packages
                .iter()
                .find(|p| {
                    if let Some(v5_metadata) = p.metadata.get("v5") {
                        v5_metadata.is_object()                        
                    } else {
                        false
                    }
                })
                .or(metadata.packages.first())
                .context("Could not locate a valid Cargo package. Is this a Rust project?")?;
            let manifest = Manifest::new(package)?;

            let mut artifact = None;

            if let Some(file) = file {
                artifact = Some(objcopy(&file).await)
            } else {
                build(&path, build_opts, false, |new_artifact| {
                    let mut bin_path = new_artifact.clone();
                    bin_path.set_extension("bin");
                    block_in_place(|| {
                        Handle::current().block_on(async move {
                            objcopy(&new_artifact).await;
                        });
                    });
                    artifact = Some(bin_path);
                }).await;
            }

            let slot = slot.or(manifest.slot).or_else(|| {
                CustomType::<u8>::new("Choose a program slot to upload to:")
                    .with_validator(|slot: &u8| Ok(if (1..=8).contains(slot) {
                        Validation::Valid
                    } else {
                        Validation::Invalid(ErrorMessage::Custom("Slot out of range".to_string()))
                    }))
                    .with_help_message("Type a slot number from 1 to 8, inclusive")
                    .prompt()
                    .ok()
            })
            .context("No upload slot was provided; consider using the --slot flag or using the `package.metadata.v5.slot` field in Cargo.toml.")?;

            upload(
                &artifact.expect("ELF artifact not found! Try explicitly providing one with `--file` (`-f`)."),
                after,
                slot,
                name.unwrap_or(package.name.clone()),
                description.or(package.description.clone()).unwrap_or("Uploaded with cargo-v5.".to_string()),
                icon.or(manifest.icon).unwrap_or_default(),
                "Rust".to_string(),
                match uncompressed {
                    Some(val) => !val,
                    None => manifest.compress.unwrap_or(true)
                },
            ).await?;
        }
        Command::Sim { ui, opts } => {
            let mut artifact = None;
            build(&path, opts, true, |new_artifact| {
                artifact = Some(new_artifact);
            }).await;
            launch_simulator(
                ui.clone(),
                path.as_ref(),
                artifact
                    .expect("Binary target not found (is this a library?)")
                    .as_ref(),
            ).await;
        }
    }

    Ok(())
}
