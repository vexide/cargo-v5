use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, ValueEnum};
use flate2::{Compression, GzBuilder};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{runtime::Handle, sync::Mutex, task::block_in_place, time::Instant};

use std::{fs::exists, io::Write, sync::Arc, time::Duration};

use vex_v5_serial::{
    commands::file::{
        LinkedFile, Program, ProgramData, ProgramIniConfig, Project, UploadFile, UploadProgram,
        USER_PROGRAM_LOAD_ADDR,
    },
    connection::{
        serial::{SerialConnection, SerialError},
        Connection,
    },
    packets::{
        cdc2::Cdc2Ack,
        file::{
            ExtensionType, FileExitAction, FileMetadata, FileVendor, GetFileMetadataPacket,
            GetFileMetadataPayload, GetFileMetadataReplyPacket,
        },
        radio::RadioChannel,
    },
    string::FixedString,
    timestamp::j2000_timestamp,
    version::Version,
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

    /// Method to use when uploading binaries.
    #[arg(long)]
    pub upload_strategy: Option<UploadStrategy>,

    /// Reupload entire base binary if patch uploading.
    #[arg(long)]
    pub cold: bool,

    /// Arguments forwarded to `cargo`.
    #[clap(flatten)]
    pub cargo_opts: CargoOpts,
}

/// Method used for uploading binaries
#[derive(ValueEnum, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum UploadStrategy {
    /// Full binary is uploaded each time
    #[default]
    Monolith,

    /// Binary patch upload (vexide only)
    Patch,
}

/// An action to perform after uploading a program.
#[derive(ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    cold: bool,
    upload_strategy: UploadStrategy,
) -> Result<(), CliError> {
    let multi_progress = MultiProgress::new();

    let slot_file_name = format!("slot_{}.bin", slot - 1);
    let ini_file_name = format!("slot_{}.ini", slot - 1);

    match upload_strategy {
        UploadStrategy::Monolith => {
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
                            "   \x1b[1;96mUploading\x1b[0m {msg:10} {percent_precise:>7}% {bar:40.green} {prefix}",
                        )
                        .unwrap() // Okay to unwrap, since this just validates style formatting.
                        .progress_chars(PROGRESS_CHARS),
                    )
                    .with_message(ini_file_name),
            ));
            let bin_progress = Arc::new(Mutex::new(
                multi_progress
                    .add(ProgressBar::new(10000))
                    .with_style(
                        ProgressStyle::with_template(
                            "   \x1b[1;96mUploading\x1b[0m {msg:10} {percent_precise:>7}% {bar:40.red} {prefix}",
                        )
                        .unwrap() // Okay to unwrap, since this just validates style formatting.
                        .progress_chars(PROGRESS_CHARS),
                    )
                    .with_message(slot_file_name.clone()),
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
                    ini_callback: Some(build_progress_callback(
                        ini_progress.clone(),
                        ini_timestamp.clone(),
                    )),
                    bin_callback: Some(build_progress_callback(
                        bin_progress.clone(),
                        bin_timestamp.clone(),
                    )),
                    lib_callback: None,
                })
                .await?;

            // Tell the progressbars that we're done once uploading is complete, allowing further messages to be printed to stdout.
            ini_progress.lock().await.finish();
            bin_progress.lock().await.finish();
        }
        UploadStrategy::Patch => {
            let base_file_name = format!("slot_{}.base.bin", slot - 1);

            if exists(path.with_extension("base.bin"))?
                && brain_file_exists(
                    connection,
                    FixedString::new(base_file_name.clone()).unwrap(),
                    FileVendor::User,
                )
                .await?
                && !cold
            {
                let patch_timestamp = Arc::new(Mutex::new(None));
                let patch_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "    \x1b[1;96mPatching\x1b[0m {msg:10} {percent_precise:>7}% {bar:40.red} {prefix}",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(slot_file_name.clone()),
                ));

                let old = tokio::fs::read(path.with_extension("base.bin")).await?;
                let new = tokio::fs::read(path).await?;
                let mut patch = Vec::new();

                bidiff::simple_diff(old.as_slice(), new.as_slice(), &mut patch).unwrap();

                // Insert important metadata for the patcher to use when constructing a new binary
                patch.reserve(12);
                patch.splice(8..8, ((patch.len() + 12) as u32).to_le_bytes());
                patch.splice(12..12, (old.len() as u32).to_le_bytes());
                patch.splice(16..16, (new.len() as u32).to_le_bytes());

                gzip_compress(&mut patch);

                log::debug!(
                    "old: {}, new: {}, patch: {}",
                    old.len(),
                    new.len(),
                    patch.len()
                );

                connection
                    .execute_command(UploadFile {
                        filename: FixedString::new(slot_file_name.clone()).unwrap(),
                        metadata: FileMetadata {
                            extension: FixedString::new("bin".to_string()).unwrap(),
                            extension_type: ExtensionType::default(),
                            timestamp: j2000_timestamp(),
                            version: Version {
                                major: 1,
                                minor: 0,
                                build: 0,
                                beta: 0,
                            },
                        },
                        vendor: Some(FileVendor::User),
                        data: patch,
                        target: None,
                        load_addr: 0x07A00000,
                        linked_file: Some(LinkedFile {
                            filename: FixedString::new(base_file_name.clone()).unwrap(),
                            vendor: Some(FileVendor::User),
                        }),
                        after_upload: match after {
                            AfterUpload::None => FileExitAction::DoNothing,
                            AfterUpload::ShowScreen => FileExitAction::ShowRunScreen,
                            AfterUpload::Run => FileExitAction::RunProgram,
                        },
                        progress_callback: Some(build_progress_callback(
                            patch_progress.clone(),
                            patch_timestamp.clone(),
                        )),
                    })
                    .await?;

                patch_progress.lock().await.finish();
            } else {
                // indicatif is a little dumb with timestamp handling, so we're going to do this all custom,
                // which unfortunately requires us to juggle timestamps across threads.
                let ini_timestamp = Arc::new(Mutex::new(None));
                let base_timestamp = Arc::new(Mutex::new(None));

                let ini_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "   \x1b[1;96mUploading\x1b[0m {msg:15} {percent_precise:>7}% {bar:40.green} {prefix}",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(ini_file_name.clone()),
                ));
                let base_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "   \x1b[1;96mUploading\x1b[0m {msg:15} {percent_precise:>7}% {bar:40.blue} {prefix}",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(base_file_name.clone()),
                ));
                let patch_timestamp = Arc::new(Mutex::new(None));
                let patch_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "    \x1b[1;96mPatching\x1b[0m {msg:15} {percent_precise:>7}% {bar:40.red} {prefix}",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(slot_file_name.clone()),
                ));

                // Create a new base.bin file with the current binary.
                tokio::fs::copy(path, path.with_extension("base.bin")).await?;

                connection
                    .execute_command(UploadFile {
                        filename: FixedString::new(ini_file_name).unwrap(),
                        metadata: FileMetadata {
                            extension: FixedString::new("ini".to_string()).unwrap(),
                            extension_type: ExtensionType::default(),
                            timestamp: j2000_timestamp(),
                            version: Version {
                                major: 1,
                                minor: 0,
                                build: 0,
                                beta: 0,
                            },
                        },
                        vendor: None,
                        data: serde_ini::to_vec(&ProgramIniConfig {
                            program: Program {
                                description,
                                icon: format!("USER{:03}x.bmp", icon as u16),
                                iconalt: String::new(),
                                slot: slot - 1,
                                name,
                            },
                            project: Project { ide: program_type },
                        })
                        .unwrap(),
                        target: None,
                        load_addr: USER_PROGRAM_LOAD_ADDR,
                        linked_file: None,
                        after_upload: FileExitAction::DoNothing,
                        progress_callback: Some(build_progress_callback(
                            ini_progress.clone(),
                            ini_timestamp.clone(),
                        )),
                    })
                    .await?;
                ini_progress.lock().await.finish();

                connection
                    .execute_command(UploadFile {
                        filename: FixedString::new(base_file_name.clone()).unwrap(),
                        metadata: FileMetadata {
                            extension: FixedString::new("bin".to_string()).unwrap(),
                            extension_type: ExtensionType::default(),
                            timestamp: j2000_timestamp(),
                            version: Version {
                                major: 1,
                                minor: 0,
                                build: 0,
                                beta: 0,
                            },
                        },
                        vendor: Some(FileVendor::User),
                        data: {
                            let mut base_data =
                                tokio::fs::read(path.with_extension("base.bin")).await?;
                            if compress {
                                gzip_compress(&mut base_data);
                            }

                            base_data
                        },
                        target: None,
                        load_addr: USER_PROGRAM_LOAD_ADDR,
                        linked_file: None,
                        after_upload: FileExitAction::DoNothing,
                        progress_callback: Some(build_progress_callback(
                            base_progress.clone(),
                            base_timestamp.clone(),
                        )),
                    })
                    .await?;
                base_progress.lock().await.finish();

                connection
                    .execute_command(UploadFile {
                        filename: FixedString::new(slot_file_name.clone()).unwrap(),
                        metadata: FileMetadata {
                            extension: FixedString::new("bin".to_string()).unwrap(),
                            extension_type: ExtensionType::default(),
                            timestamp: j2000_timestamp(),
                            version: Version {
                                major: 1,
                                minor: 0,
                                build: 0,
                                beta: 0,
                            },
                        },
                        vendor: Some(FileVendor::User),
                        data: u32::to_le_bytes(0xB2DF).to_vec(),
                        target: None,
                        load_addr: 0x07A00000,
                        linked_file: Some(LinkedFile {
                            filename: FixedString::new(base_file_name).unwrap(),
                            vendor: Some(FileVendor::User),
                        }),
                        after_upload: match after {
                            AfterUpload::None => FileExitAction::DoNothing,
                            AfterUpload::ShowScreen => FileExitAction::ShowRunScreen,
                            AfterUpload::Run => FileExitAction::RunProgram,
                        },
                        progress_callback: Some(build_progress_callback(
                            patch_progress.clone(),
                            patch_timestamp.clone(),
                        )),
                    })
                    .await?;

                patch_progress.lock().await.finish();
            };
        }
    }

    if after == AfterUpload::Run {
        println!("     \x1b[1;92mRunning\x1b[0m `{}`", slot_file_name);
    }

    Ok(())
}

async fn brain_file_exists(
    connection: &mut SerialConnection,
    file_name: FixedString<23>,
    vendor: FileVendor,
) -> Result<bool, SerialError> {
    match connection
        .packet_handshake::<GetFileMetadataReplyPacket>(
            Duration::from_millis(500),
            1,
            GetFileMetadataPacket::new(GetFileMetadataPayload {
                vendor,
                option: 0,
                file_name,
            }),
        )
        .await?
        .ack
    {
        Cdc2Ack::NackProgramFile => Ok(false),
        Cdc2Ack::Ack => Ok(true),
        nack => Err(SerialError::Nack(nack)),
    }
}

fn build_progress_callback(
    progress: Arc<Mutex<ProgressBar>>,
    timestamp: Arc<Mutex<Option<Instant>>>,
) -> Box<dyn FnMut(f32) + Send> {
    Box::new(move |percent| {
        let progress = progress.try_lock().unwrap();
        let mut timestamp = timestamp.try_lock().unwrap();

        if timestamp.is_none() {
            *timestamp = Some(Instant::now());
        }
        progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
        progress.set_position((percent * 100.0) as u64);
    })
}

/// Apply gzip compression to the given data
fn gzip_compress(data: &mut Vec<u8>) {
    let mut encoder = GzBuilder::new().write(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    *data = encoder.finish().unwrap();
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
        upload_strategy,
        cold,
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
    if !(1..=8).contains(&slot) {
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
        cold,
        upload_strategy
            .or(metadata.and_then(|metadata| metadata.upload_strategy))
            .unwrap_or_default(),
    )
    .await?;

    Ok(())
}
