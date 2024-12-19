use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{runtime::Handle, sync::Mutex, task::block_in_place, time::Instant};

use std::sync::Arc;

use vex_v5_serial::{
    commands::file::{ProgramData, UploadProgram},
    connection::{serial::SerialConnection, Connection},
    packets::{file::FileExitAction, radio::RadioChannel},
};

use crate::{connection::switch_radio_channel, errors::CliError, metadata::Metadata};

use super::build::{build, objcopy, CargoOpts};

/// Options used to control the behavior of a program upload
#[derive(Args, Debug)]
pub struct UploadOpts {
    /// Program slot.
    #[arg(short, long)]
    pub slot: Option<u8>,

    /// The name of the program.
    #[arg(long)]
    pub name: Option<String>,

    /// The description of the program.
    #[arg(short, long)]
    pub description: Option<String>,

    /// The program's file icon.
    #[arg(short, long)]
    pub icon: Option<ProgramIcon>,

    /// Skip gzip compression before uploading. Will result in longer upload times.
    #[arg(short, long)]
    pub uncompressed: Option<bool>,

    /// An build artifact to upload (either an ELF or BIN).
    #[arg(long)]
    pub file: Option<Utf8PathBuf>,

    /// Arguments forwarded to `cargo`.
    #[clap(flatten)]
    pub cargo_opts: CargoOpts,
}

/// An action to perform after uploading a program.
#[derive(ValueEnum, Debug, Clone, Copy, Default)]
pub enum AfterUpload {
    /// Do nothing.
    #[default]
    None,

    /// Execute the program.
    Run,

    /// Show the program's "run" screen on the brain
    #[clap(name = "screen")]
    ShowScreen,
}

impl From<AfterUpload> for FileExitAction {
    fn from(value: AfterUpload) -> Self {
        match value {
            AfterUpload::None => FileExitAction::DoNothing,
            AfterUpload::Run => FileExitAction::RunProgram,
            AfterUpload::ShowScreen => FileExitAction::ShowRunScreen,
        }
    }
}

/// A prograShow the program's "Run"m file icon.
#[derive(ValueEnum, Default, Debug, Clone, Copy, Eq, PartialEq)]
#[repr(u16)]
pub enum ProgramIcon {
    VexCodingStudio = 0,
    CoolX = 1,
    // This is the icon that appears when you provide a missing icon name.
    // 2 is one such icon that doesn't exist.
    #[default]
    QuestionMark = 2,
    Pizza = 3,
    Clawbot = 10,
    Robot = 11,
    PowerButton = 12,
    Planets = 13,
    Alien = 27,
    AlienInUfo = 29,
    CupInField = 50,
    CupAndBall = 51,
    Matlab = 901,
    Pros = 902,
    RobotMesh = 903,
    RobotMeshCpp = 911,
    RobotMeshBlockly = 912,
    RobotMeshFlowol = 913,
    RobotMeshJS = 914,
    RobotMeshPy = 915,
    // This icon is duplicated several times and has many file names.
    CodeFile = 920,
    VexcodeBrackets = 921,
    VexcodeBlocks = 922,
    VexcodePython = 925,
    VexcodeCpp = 926,
}

pub const PROGRESS_CHARS: &str = "⣿⣦⣀";

/// Upload a program to the brain.
pub async fn upload_program(
    connection: &mut SerialConnection,
    path: &Utf8Path,
    after: AfterUpload,
    slot: u8,
    name: String,
    description: String,
    icon: ProgramIcon,
    program_type: String,
    compress: bool,
) -> Result<(), CliError> {
    let multi_progress = MultiProgress::new();

    // indicatif is a little dumb with timestamp handling, so we're going to do this all custom,
    // which unfortunately requires us to juggle timestamps across threads.
    let ini_timestamp = Arc::new(Mutex::new(None));
    let bin_timestamp = Arc::new(Mutex::new(None));

    // Progress bars
    let ini_progress = Arc::new(Mutex::new(
        multi_progress
            .add(ProgressBar::new(10000))
            .with_style(
                ProgressStyle::with_template(
                    "{msg:4} {percent_precise:>7}% {bar:40.green} {prefix}",
                )
                .unwrap() // Okay to unwrap, since this just validates style formatting.
                .progress_chars(PROGRESS_CHARS),
            )
            .with_message("INI"),
    ));
    let bin_progress = Arc::new(Mutex::new(
        multi_progress
            .add(ProgressBar::new(10000))
            .with_style(
                ProgressStyle::with_template("{msg:4} {percent_precise:>7}% {bar:40.red} {prefix}")
                    .unwrap() // Okay to unwrap, since this just validates style formatting.
                    .progress_chars(PROGRESS_CHARS),
            )
            .with_message("BIN"),
    ));

    // Read our program file into a buffer.
    //
    // We're uploading a monolith (single-bin, no hot/cold linking).
    let data = ProgramData::Monolith(tokio::fs::read(path).await?);

    // Upload the program.
    connection
        .execute_command(UploadProgram {
            name,
            description,
            icon: format!("USER{:03}x.bmp", icon as u16),
            program_type,
            slot: slot - 1,
            compress_program: compress,
            data,
            after_upload: after.into(),
            ini_callback: {
                Some({
                    let ini_progres = ini_progress.clone();
                    let ini_timestamp = ini_timestamp.clone();

                    Box::new(move |percent| {
                        let progress = ini_progres.try_lock().unwrap();
                        let mut timestamp = ini_timestamp.try_lock().unwrap();

                        if timestamp.is_none() {
                            *timestamp = Some(Instant::now());
                        }

                        progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                        progress.set_position((percent * 100.0) as u64);
                    })
                })
            },
            bin_callback: {
                Some({
                    let bin_progress = bin_progress.clone();
                    let bin_timestamp = bin_timestamp.clone();
                    
                    Box::new(move |percent| {
                        let progress = bin_progress.try_lock().unwrap();
                        let mut timestamp = bin_timestamp.try_lock().unwrap();

                        if timestamp.is_none() {
                            *timestamp = Some(Instant::now());
                        }
                        progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                        progress.set_position((percent * 100.0) as u64);
                    })
                })
            },
            lib_callback: None,
        })
        .await?;

    // Tell the progressbars that we're done once uploading is complete, allowing further messages to be printed to stdout.
    ini_progress.lock().await.finish();
    bin_progress.lock().await.finish();

    Ok(())
}

pub async fn upload(
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
    connection: &mut SerialConnection,
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

    // Switch the radio to the download channel if the controller is wireless.
    switch_radio_channel(connection, RadioChannel::Download).await?;

    // Pass information to the upload routine.
    upload_program(
        connection,
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

    Ok(())
}
