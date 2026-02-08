use clap::{Args, ValueEnum};
use flate2::{Compression, GzBuilder};
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use inquire::{
    CustomType,
    validator::{ErrorMessage, Validation},
};
use tokio::{fs::File, io::AsyncWriteExt, task::block_in_place};

use std::{
    ffi::OsStr,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use vex_v5_serial::{
    Connection,
    commands::file::{LinkedFile, USER_PROGRAM_LOAD_ADDR, j2000_timestamp, upload_file},
    protocol::{
        FixedString, VEX_CRC32, Version,
        cdc2::{
            Cdc2Ack,
            file::{
                ExtensionType, FileExitAction, FileMetadata, FileMetadataPacket,
                FileMetadataReplyPacket, FileTransferTarget, FileVendor,
            },
        },
    },
    serial::{SerialConnection, SerialError},
};

use crate::{
    connection::{open_connection, switch_to_download_channel},
    errors::CliError,
    metadata::Metadata,
};

use super::build::{CargoOpts, build, objcopy};

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
    pub file: Option<PathBuf>,

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
    path: &Path,
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
    let mut multi_progress = MultiProgress::new();

    // Filenames for the program in the device's filesystem. These are the same ones used by
    // VEXcode/vexcom by convention, and strange things will happen if we have conflicting catalog
    // entries, so we keep the `slot_n` naming to ensure that old files will be properly
    // overwritten.
    let slot_file_name = format!("slot_{slot}.bin");
    let ini_file_name = format!("slot_{slot}.ini");

    // MARK: Metadata upload
    //
    // Every program binary uploaded to the brain is accompanied by an INI file describing the
    // program's metadata. This is where we communicate to VEXos what name, slot, icon, etc... the
    // program should have.
    //
    // For example, the program binary `slot_1.bin` will have an accompanying `slot_1.ini` that
    // VEXos will consult to build a catalog entry that's accessible in the UI.
    let ini = format!(
        "[project]
ide={}
[program]
name={}
slot={}
icon=USER{:03}x.bmp
iconalt=
description={}",
        program_type,
        name,
        slot - 1,
        icon as u16,
        description
    );

    // Determine if the program metadata file needs to be reuploaded.
    //
    // We only reupload slot_n.ini if there's a checksum mismatch between the INI generated from the
    // current arguments to this function and the INI on the brain, indicating that something has
    // changed. Otherwise, we save the overhead of another file transfer on every upload.
    let needs_ini_upload = if let Some(brain_metadata) = brain_file_metadata(
        connection,
        FixedString::new(ini_file_name.clone()).unwrap(),
        FileVendor::User,
    )
    .await?
    {
        brain_metadata.crc32 != VEX_CRC32.checksum(ini.as_bytes())
    } else {
        true
    };

    if needs_ini_upload {
        let (progress, callback) = make_progress_callback(
            &mut multi_progress,
            ProgressStyle::with_template(
                "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.green} {msg} ({elapsed_duration})",
            ).unwrap(),
            ini_file_name.clone()
        );

        upload_file(
            connection,
            FixedString::new(ini_file_name).unwrap(),
            FileMetadata {
                extension: FixedString::new("ini").unwrap(),
                extension_type: ExtensionType::default(),
                timestamp: j2000_timestamp(),
                version: Version {
                    major: 1,
                    minor: 0,
                    build: 0,
                    beta: 0,
                },
            },
            FileVendor::User,
            ini.as_bytes(),
            FileTransferTarget::Qspi,
            USER_PROGRAM_LOAD_ADDR,
            None,
            FileExitAction::DoNothing,
            Some(callback),
        )
        .await?;

        progress.finish();
    }

    // Logic for uploading the actual program binaries.
    match upload_strategy {
        // MARK: Monolith upload
        //
        // This one is really simple, we just upload the binary in full.
        UploadStrategy::Monolith => {
            let (progress, callback) = make_progress_callback(
                &mut multi_progress,
                ProgressStyle::with_template(
                    "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.red} {msg} ({elapsed_duration})",
                ).unwrap(),
                slot_file_name.clone(),
            );

            // Upload the program.
            upload_file(
                connection,
                FixedString::new(slot_file_name.clone()).unwrap(),
                FileMetadata {
                    extension: FixedString::new("bin").unwrap(),
                    extension_type: ExtensionType::default(),
                    timestamp: j2000_timestamp(),
                    version: Version {
                        major: 1,
                        minor: 0,
                        build: 0,
                        beta: 0,
                    },
                },
                FileVendor::User,
                &{
                    let mut data = tokio::fs::read(path).await?;

                    if compress {
                        // <https://media1.tenor.com/m/cjSTJh8J3QcAAAAd/cat-cat-sink.gif>
                        gzip_compress(&mut data);
                    }

                    data
                },
                FileTransferTarget::Qspi,
                USER_PROGRAM_LOAD_ADDR,
                None,
                after.into(),
                Some(callback),
            )
            .await?;

            // Tell the progressbars that we're done once uploading is complete, allowing further
            // messages to be printed to stdout.
            progress.finish();
        }

        // Before you try to dissect this, please go and read
        // <https://github.com/vexide/vexide/pull/269> and also maybe
        // <https://github.com/vexide/vexide/blob/main/packages/vexide-startup/src/patcher/mod.rs>
        // (which is probably more up to date if the implementation changes).
        //
        // This code is commented to the best of my ability, but there's a lot of pretty complex
        // logic and guardrails here to ensure that the Brain will never get de-synced with local
        // copies of base file and whatnot.
        UploadStrategy::Differential => {
            let base_file_name = format!("slot_{slot}.base.bin");

            // cargo-v5 stores the last cold uploaded binary locally (called the "base binary"),
            // which is used as a reference point for building patches off of.
            //
            // essentially: `patch = diff(base, new)`
            //
            // This base file MUST exist on both the brain and the local machine for a patch upload
            // to take place, but it obviously won't be on either if this is our first time
            // uploading, so this is optional and we'll handle that case in a sec.
            let mut base = match tokio::fs::read(&path.with_file_name(&base_file_name)).await {
                Ok(contents) => Some(contents),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                _ => None, // TODO: maybe throw an error here. that's better than a fallthrough.
            };

            // Next we need to determine if a cold upload is required.
            //
            // A "cold upload" is a case where a new base binary must be uploaded in its entirety to
            // the brain. This should happen in three cases:
            //
            // - A base binary doesn't exist, either on the local machine or on the Brain's
            //   filesystem.
            // - The base binary on the local filesystem differs from the base binary on the Brain
            //   (can happen when switching projects or computers).
            // - The user explicitly requested a cold reupload with `cargo v5 upload --cold ...`.
            let needs_cold_upload = cold
                || 'check: {
                    // If the base doesn't exist on the local machine, a cold upload is needed.
                    let Some(base) = base.as_mut() else {
                        break 'check true;
                    };

                    // Attempt to get metadata for the base binary off the Brain's internal flash.
                    let Some(brain_metadata) = brain_file_metadata(
                        connection,
                        FixedString::new(base_file_name.clone()).unwrap(),
                        FileVendor::User,
                    )
                    .await?
                    else {
                        // This usually means that the file doesn't exist on the Brain.
                        break 'check true;
                    };

                    // Compare the CRC32 of our local file with the one on the Brain. If they don't
                    // match, then the brain has different base file and we need to cold upload.
                    if base.len() >= 4 {
                        // When we store the base binary to the local machine, we append its crc32
                        // to the last four bytes of the file, which saves the trouble of us
                        // recomputing it on every upload. This is kinda a gross hack, but whatevs.
                        let crc_metadata =
                            u32::from_le_bytes(base.split_off(base.len() - 4).try_into().unwrap());

                        brain_metadata.crc32 != crc_metadata
                    } else {
                        true
                    }
                };

            // MARK: Patch upload
            if !needs_cold_upload {
                let (progress, callback) = make_progress_callback(
                    &mut multi_progress,
                    ProgressStyle::with_template(
                        "    \x1b[1;96mPatching\x1b[0m {percent_precise:>7}% {bar:40.red} {msg} ({elapsed_duration})",
                    ).unwrap(),
                    slot_file_name.clone()
                );

                // The "new" file is the file that the user requested to upload, as opposed to the
                // "base" file which is the program that the brain already has.
                let new = tokio::fs::read(path).await?;
                let base = base.unwrap();

                // Some sanity checks to make sure that the patch and base file fit inside the 2mb
                // subregions that we allocate in program memory before compression. This also
                // ensures that VEXos won't data abort on CPU0, since there's a buffer overflow that
                // can occur in the file transfer logic when uploading very large compressed
                // programs.
                if base.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(base.len()));
                } else if new.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(new.len()));
                }

                // Generate a patch (binary diff) using the base binary and the new file.
                let mut patch = build_patch(&base, &new);
                if patch.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::PatchTooLarge(patch.len()));
                }

                // We ignore compression preferences and always gzip here, since bidiff NEEDS a
                // compression algorithm to gain any advantage on size at all. Uncompressed patches
                // in memory are larger than both the base and new file. We have plenty of memory to
                // go around, and only care about upload bandwidth.
                gzip_compress(&mut patch);

                // Upload the patch
                upload_file(
                    connection,
                    FixedString::new(slot_file_name.clone()).unwrap(),
                    FileMetadata {
                        extension: FixedString::new("bin").unwrap(),
                        extension_type: ExtensionType::default(),
                        timestamp: j2000_timestamp(),
                        version: Version {
                            major: 1,
                            minor: 0,
                            build: 0,
                            beta: 0,
                        },
                    },
                    FileVendor::User,
                    &patch,
                    FileTransferTarget::Qspi,
                    // See <https://github.com/vexide/vexide/blob/main/packages/vexide-startup/src/patcher/mod.rs#L38>
                    // and <https://github.com/vexide/vexide/blob/main/packages/vexide-startup/link/vexide.ld#L41>
                    // if you're confused about why we're loading this file to that address.
                    0x07A00000,
                    // This patch file is the binary that has the actual catalog entry (it's the
                    // file that VEXos actually treats as "a program"). We do it this way since it's
                    // impossible to edit a file link after a program has been uploaded. So, the
                    // base file at 0x03800000 is what's linked to the patch rather than the other
                    // way around.
                    Some(LinkedFile {
                        file_name: FixedString::new(base_file_name.clone()).unwrap(),
                        vendor: FileVendor::User,
                    }),
                    after.into(),
                    Some(callback),
                )
                .await?;

                progress.finish();
            } else {
                // MARK: Cold upload
                let (progress, callback) = make_progress_callback(
                    &mut multi_progress,
                    ProgressStyle::with_template(
                        "   \x1b[1;96mUploading\x1b[0m {percent_precise:>7}% {bar:40.blue} {msg} ({elapsed_duration})",
                    ).unwrap(),
                    base_file_name.clone()
                );

                let mut base_data = tokio::fs::read(path).await?;

                if base_data.len() > DIFFERENTIAL_UPLOAD_MAX_SIZE {
                    return Err(CliError::ProgramTooLarge(base_data.len()));
                }

                // Upload the entire base file during a cold upload. This is pretty much the same as
                // a normal monolith upload, but the file is named `slot_n.base.bin` rather than
                // `slot_n.bin` and will be linked by the dummy patch to correct load address below.
                upload_file(
                    connection,
                    FixedString::new(base_file_name.clone()).unwrap(),
                    FileMetadata {
                        extension: FixedString::new("bin").unwrap(),
                        extension_type: ExtensionType::default(),
                        timestamp: j2000_timestamp(),
                        version: Version {
                            major: 1,
                            minor: 0,
                            build: 0,
                            beta: 0,
                        },
                    },
                    FileVendor::User,
                    {
                        // Save the base file to the local machine.
                        let mut base_file =
                            File::create(path.with_file_name(&base_file_name)).await?;
                        base_file.write_all(&base_data).await?;

                        if compress {
                            // <https://media1.tenor.com/m/cjSTJh8J3QcAAAAd/cat-cat-sink.gif>
                            gzip_compress(&mut base_data);
                        }

                        // If you've been reading these comments, you should already know why we do
                        // this :3
                        base_file
                            .write_all(&VEX_CRC32.checksum(&base_data).to_le_bytes())
                            .await?;

                        &base_data
                    },
                    FileTransferTarget::Qspi,
                    USER_PROGRAM_LOAD_ADDR,
                    None,
                    FileExitAction::DoNothing,
                    Some(callback),
                )
                .await?;

                // We have to get rid of any old patch files that may or may not be there after a
                // cold upload, so we overwrite them with a placeholder patch with an intentionally
                // invalid header so that vexide-startup skips it.
                //
                // We also need this because the patch binary is the one that VEXos actually treats
                // as "a program", and therefore is the one with the file link (as discussed above).
                upload_file(
                    connection,
                    FixedString::new(slot_file_name.clone()).unwrap(),
                    FileMetadata {
                        extension: FixedString::new("bin").unwrap(),
                        extension_type: ExtensionType::default(),
                        timestamp: j2000_timestamp(),
                        version: Version {
                            major: 1,
                            minor: 0,
                            build: 0,
                            beta: 0,
                        },
                    },
                    FileVendor::User,
                    // An obviously invalid patch header that'll bail out early (valid would be
                    // 0xB1DF). We use 0xB2DF to indicate that the patch is already applied on the
                    // second _vexide_boot round, so we can just reuse that here.
                    &u32::to_le_bytes(0xB2DF),
                    FileTransferTarget::Qspi,
                    0x07A00000,
                    Some(LinkedFile {
                        file_name: FixedString::new(base_file_name.clone()).unwrap(),
                        vendor: FileVendor::User,
                    }),
                    after.into(),
                    None::<fn(f32)>,
                )
                .await?;

                progress.finish();
            }
        }
    }

    if after == AfterUpload::Run {
        eprintln!("     \x1b[1;92mRunning\x1b[0m `{slot_file_name}`");
    }

    Ok(())
}

fn build_patch(old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut patch = Vec::new();

    bidiff::simple_diff(old, new, &mut patch).unwrap();

    // Insert some important metadata for the patcher to use when constructing a new binary.
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
) -> Result<Option<FileMetadataReplyPacket>, SerialError> {
    let reply = connection
        .handshake(
            FileMetadataPacket {
                vendor,
                reserved: 0,
                file_name,
            },
            Duration::from_millis(1000),
            2,
        )
        .await?;

    match reply {
        Ok(payload) => Ok(payload),
        Err(Cdc2Ack::NackProgramFile) => Ok(None),
        Err(nack) => Err(SerialError::Nack(nack)),
    }
}

fn make_progress_callback(
    multi: &mut MultiProgress,
    style: ProgressStyle,
    file_name: String,
) -> (ProgressBar, impl FnMut(f32)) {
    let bar = multi
        .add(ProgressBar::new(10000))
        .with_style(
            style
                .with_key(
                    "elapsed_duration",
                    |state: &ProgressState, buf: &mut dyn std::fmt::Write| {
                        write!(buf, "{:?}", state.elapsed()).unwrap()
                    },
                )
                .progress_chars(PROGRESS_CHARS),
        )
        .with_message(file_name);

    (bar.clone(), {
        move |percent| {
            bar.set_position((percent * 100.0) as u64);
        }
    })
}

/// Apply gzip compression to the given data
fn gzip_compress(data: &mut Vec<u8>) {
    let mut encoder = GzBuilder::new().write(Vec::new(), Compression::best());
    encoder.write_all(data).unwrap();
    *data = encoder.finish().unwrap();
}

pub async fn upload(
    path: &Path,
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
    let (mut connection, (artifact, package_id)) = tokio::try_join!(
        async {
            let mut connection = open_connection().await?;

            // Switch the radio to the download channel if the controller is wireless.
            switch_to_download_channel(&mut connection).await?;

            Ok::<SerialConnection, CliError>(connection)
        },
        async {
            // Get the build artifact we'll be uploading with.
            //
            // The user either directly passed an file through the `--file` argument, or they didn't and we need to run
            // `cargo build`.
            Ok(if let Some(file) = file {
                if file.extension() == Some(OsStr::new("bin")) {
                    (file, None)
                } else {
                    // If a BIN file wasn't provided, we'll attempt to objcopy it as if it were an ELF.
                    let binary =
                        objcopy(&tokio::fs::read(&file).await.map_err(CliError::IoError)?)?;
                    let binary_path = file.with_extension("bin");

                    // Write the binary to a file.
                    tokio::fs::write(&binary_path, binary)
                        .await
                        .map_err(CliError::IoError)?;
                    eprintln!("     \x1b[1;92mObjcopy\x1b[0m {}", binary_path.display());

                    (binary_path, None)
                }
            } else {
                // Run cargo build, then objcopy.
                build(path, cargo_opts)
                    .await?
                    .map(|output| (output.bin_artifact, Some(output.package_id)))
                    .ok_or(CliError::NoArtifact)?
            })
        }
    )?;

    // We'll use `cargo-metadata` to parse the output of `cargo metadata` and find valid `Cargo.toml`
    // files in the workspace directory.
    let cargo_metadata =
        block_in_place(|| cargo_metadata::MetadataCommand::new().no_deps().exec()).ok();

    // Find which package we're being built from, if we're being built from a package at all.
    let package = cargo_metadata.and_then(|metadata| {
        package_id
            .as_ref()
            .and_then(|id| metadata.packages.iter().find(|p| &p.id == id))
            .or_else(|| metadata.packages.first())
            .cloned()
    });

    // Uploading has the option to use the `package.metadata.v5` table for default configuration options.
    // Attempt to serialize `package.metadata.v5` into a [`Metadata`] struct. This will just Default::default to
    // all `None`s if it can't find a specific field, or error if the field is malformed.
    let metadata = package.as_ref().map(Metadata::new).transpose()?;

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

    // Pass information to the upload routine.
    upload_program(
        &mut connection,
        &artifact,
        after,
        slot,
        name.or(package.as_ref().map(|pkg| pkg.name.to_string()))
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
