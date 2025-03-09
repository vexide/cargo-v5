use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, ValueEnum};
use flate2::{Compression, GzBuilder};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use tokio::{fs::File, io::AsyncWriteExt, spawn, sync::Mutex, task::block_in_place, time::Instant};

use std::{
    io::{ErrorKind, Write},
    sync::Arc,
    time::Duration,
};

use vex_v5_serial::{
    commands::file::{
        LinkedFile, Program, ProgramIniConfig, Project, UploadFile, USER_PROGRAM_LOAD_ADDR,
    },
    connection::{
        serial::{SerialConnection, SerialError},
        Connection,
    },
    crc::VEX_CRC32,
    packets::{
        cdc2::Cdc2Ack,
        file::{
            ExtensionType, FileExitAction, FileMetadata, FileVendor, GetFileMetadataPacket,
            GetFileMetadataPayload, GetFileMetadataReplyPacket, GetFileMetadataReplyPayload,
        },
        radio::RadioChannel,
    },
    string::FixedString,
    timestamp::j2000_timestamp,
    version::Version,
};

use crate::{
    connection::{open_connection, switch_radio_channel},
    errors::CliError,
    metadata::Metadata,
};

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

    /// Reupload entire base binary if differential uploading.
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

    /// Differential uploads (vexide only)
    Differential,
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

const DIFFERENTIAL_UPLOAD_MAX_SIZE: usize = 0x200000;

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

    let slot_file_name = format!("slot_{}.bin", slot);
    let ini_file_name = format!("slot_{}.ini", slot);

    let ini_data = serde_ini::to_vec(&ProgramIniConfig {
        program: Program {
            description,
            icon: format!("USER{:03}x.bmp", icon as u16),
            iconalt: String::new(),
            slot: slot - 1,
            name,
        },
        project: Project { ide: program_type },
    })
    .unwrap();

    let needs_ini_upload = if let Some(brain_metadata) = brain_file_metadata(
        connection,
        FixedString::new(ini_file_name.clone()).unwrap(),
        FileVendor::User,
    )
    .await?
    {
        brain_metadata.crc32 != VEX_CRC32.checksum(&ini_data)
    } else {
        true
    };

    if needs_ini_upload {
        let ini_timestamp = Arc::new(Mutex::new(None));
        // Progress bars
        let ini_progress = Arc::new(Mutex::new(
            multi_progress
                .add(ProgressBar::new(10000))
                .with_style(
                    ProgressStyle::with_template(
                        "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.green} {msg} ({prefix})",
                    )
                    .unwrap() // Okay to unwrap, since this just validates style formatting.
                    .progress_chars(PROGRESS_CHARS),
                )
                .with_message(ini_file_name.clone()),
        ));

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
                data: ini_data,
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
    }

    match upload_strategy {
        UploadStrategy::Monolith => {
            // indicatif is a little dumb with timestamp handling, so we're going to do this all custom,
            // which unfortunately requires us to juggle timestamps across threads.
            let bin_timestamp = Arc::new(Mutex::new(None));

            let bin_progress = Arc::new(Mutex::new(
                multi_progress
                    .add(ProgressBar::new(10000))
                    .with_style(
                        ProgressStyle::with_template(
                            "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.red} {msg} ({prefix})",
                        )
                        .unwrap() // Okay to unwrap, since this just validates style formatting.
                        .progress_chars(PROGRESS_CHARS),
                    )
                    .with_message(slot_file_name.clone()),
            ));

            // Upload the program.
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
                    data: tokio::fs::read(path).await?,
                    target: None,
                    load_addr: USER_PROGRAM_LOAD_ADDR,
                    linked_file: None,
                    after_upload: match after {
                        AfterUpload::None => FileExitAction::DoNothing,
                        AfterUpload::ShowScreen => FileExitAction::ShowRunScreen,
                        AfterUpload::Run => FileExitAction::RunProgram,
                    },
                    progress_callback: Some(build_progress_callback(
                        bin_progress.clone(),
                        bin_timestamp.clone(),
                    )),
                })
                .await?;

            // Tell the progressbars that we're done once uploading is complete, allowing further messages to be printed to stdout.
            bin_progress.lock().await.finish();
        }
        UploadStrategy::Differential => {
            let base_file_name = format!("slot_{}.base.bin", slot);

            let mut base = match tokio::fs::read(&path.with_file_name(&base_file_name)).await {
                Ok(contents) => Some(contents),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                _ => None,
            };

            let needs_cold_upload = cold
                || if let Some(base) = base.as_mut() {
                    if let Some(brain_metadata) = brain_file_metadata(
                        connection,
                        FixedString::new(base_file_name.clone()).unwrap(),
                        FileVendor::User,
                    )
                    .await?
                    {
                        if base.len() >= 4 {
                            let crc_metadata = u32::from_le_bytes(
                                base.split_off(base.len() - 4).try_into().unwrap(),
                            );

                            // last four bytes of base file contain the crc32 at time of upload
                            brain_metadata.crc32 != crc_metadata
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };

            if !needs_cold_upload {
                let base = base.unwrap();
                let patch_timestamp = Arc::new(Mutex::new(None));
                let patch_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "    \x1b[1;96mPatching\x1b[0m {percent_precise:>7}% {bar:40.red} {msg} ({prefix})",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(slot_file_name.clone()),
                ));

                let new = tokio::fs::read(path).await?;

                if base.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(base.len()));
                } else if new.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(new.len()));
                }

                let mut patch = build_patch(&base, &new);

                if patch.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::PatchTooLarge(patch.len()));
                }

                gzip_compress(&mut patch);

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
                let base_timestamp = Arc::new(Mutex::new(None));

                let base_progress = Arc::new(Mutex::new(
                    multi_progress
                        .add(ProgressBar::new(10000))
                        .with_style(
                            ProgressStyle::with_template(
                                "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.blue} {msg} ({prefix})",
                            )
                            .unwrap() // Okay to unwrap, since this just validates style formatting.
                            .progress_chars(PROGRESS_CHARS),
                        )
                        .with_message(base_file_name.clone()),
                ));

                let mut base_data = tokio::fs::read(path).await?;

                if base_data.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(base_data.len()));
                }

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
                            let mut base_file =
                                File::create(path.with_file_name(&base_file_name)).await?;
                            base_file.write_all(&base_data).await?;

                            if compress {
                                gzip_compress(&mut base_data);
                            }

                            base_file
                                .write_all(&VEX_CRC32.checksum(&base_data).to_le_bytes())
                                .await?;

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
                        progress_callback: None,
                    })
                    .await?;
            };
        }
    }

    if after == AfterUpload::Run {
        println!("     \x1b[1;92mRunning\x1b[0m `{}`", slot_file_name);
    }

    Ok(())
}

fn build_patch(old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut patch = Vec::new();

    bidiff::simple_diff(old, new, &mut patch).unwrap();

    // Insert important metadata for the patcher to use when constructing a new binary
    patch.reserve(12);
    patch.splice(8..8, ((patch.len() + 12) as u32).to_le_bytes());
    patch.splice(12..12, (old.len() as u32).to_le_bytes());
    patch.splice(16..16, (new.len() as u32).to_le_bytes());

    patch
}

async fn brain_file_metadata(
    connection: &mut SerialConnection,
    file_name: FixedString<23>,
    vendor: FileVendor,
) -> Result<Option<GetFileMetadataReplyPayload>, SerialError> {
    let reply = connection
        .packet_handshake::<GetFileMetadataReplyPacket>(
            Duration::from_millis(1000),
            2,
            GetFileMetadataPacket::new(GetFileMetadataPayload {
                vendor,
                option: 0,
                file_name,
            }),
        )
        .await?;
    match reply.ack {
        Cdc2Ack::NackProgramFile => Ok(None),
        Cdc2Ack::Ack => Ok(Some(if let Some(data) = reply.try_into_inner()? {
            data
        } else {
            return Ok(None);
        })),
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
    let mut encoder = GzBuilder::new().write(Vec::new(), Compression::best());
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
) -> miette::Result<SerialConnection> {
    // Try to open a serialport in the background while we build.
    let connection_task = spawn(open_connection());

    // Get the build artifact we'll be uploading with.
    //
    // The user either directly passed an file through the `--file` argument, or they didn't and we need to run
    // `cargo build`.
    let (artifact, package_id) = if let Some(file) = file {
        if file.extension() == Some("bin") {
            (file, None)
        } else {
            // If a BIN file wasn't provided, we'll attempt to objcopy it as if it were an ELF.
            let binary = objcopy(
                &tokio::fs::read(&file)
                    .await
                    .map_err(|e| CliError::IoError(e))?,
            )?;
            let binary_path = file.with_extension("bin");

            // Write the binary to a file.
            tokio::fs::write(&binary_path, binary)
                .await
                .map_err(|e| CliError::IoError(e))?;
            println!("     \x1b[1;92mObjcopy\x1b[0m {}", binary_path);

            (binary_path, None)
        }
    } else {
        // Run cargo build, then objcopy.
        build(path, cargo_opts, false)
            .await?
            .map(|output| (output.bin_artifact, Some(output.package_id)))
            .ok_or(CliError::NoArtifact)?
    };

    // We'll use `cargo-metadata` to parse the output of `cargo metadata` and find valid `Cargo.toml`
    // files in the workspace directory.
    let cargo_metadata =
        block_in_place(|| cargo_metadata::MetadataCommand::new().no_deps().exec()).ok();

    // Find which package we're being built from, if we're being built from a package at all.
    let package = cargo_metadata.and_then(|metadata| {
        metadata
            .packages
            .iter()
            .find(|p| {
                if let Some(package_id) = package_id.as_ref() {
                    &p.id == package_id
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

    // Wait for the serial port to finish opening.
    let mut connection = connection_task.await.unwrap()?;

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
    switch_radio_channel(&mut connection, RadioChannel::Download).await?;

    // Pass information to the upload routine.
    upload_program(
        &mut connection,
        &artifact,
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

    Ok(connection)
}
