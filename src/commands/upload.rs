use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::{
    runtime::Handle,
    select,
    sync::Mutex,
    task::block_in_place,
    time::{sleep, Instant},
};

use std::{sync::Arc, time::Duration};

use vex_v5_serial::{
    commands::file::{ProgramData, UploadProgram},
    connection::{serial::SerialConnection, Connection},
    packets::{
        file::FileExitAction,
        radio::{
            RadioChannel, SelectRadioChannelPacket, SelectRadioChannelPayload,
            SelectRadioChannelReplyPacket,
        },
        system::{
            GetSystemFlagsPacket, GetSystemFlagsReplyPacket, GetSystemVersionPacket,
            GetSystemVersionReplyPacket, ProductFlags,
        },
    },
};

use crate::errors::CliError;

use super::build::CargoOpts;

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

async fn is_connection_wireless(connection: &mut SerialConnection) -> Result<bool, CliError> {
    let version = connection
        .packet_handshake::<GetSystemVersionReplyPacket>(
            Duration::from_millis(500),
            1,
            GetSystemVersionPacket::new(()),
        )
        .await?;
    let system_flags = connection
        .packet_handshake::<GetSystemFlagsReplyPacket>(
            Duration::from_millis(500),
            1,
            GetSystemFlagsPacket::new(()),
        )
        .await?;
    let controller = version
        .payload
        .flags
        .contains(ProductFlags::CONNECTED_WIRELESS);

    let tethered = system_flags.payload.flags & (1 << 23) != 0;
    Ok(!tethered && controller)
}

async fn switch_to_download_channel(connection: &mut SerialConnection) -> Result<(), CliError> {
    if connection.connection_type().is_controller() {
        println!("Switching radio to download channel...");

        // Tell the controller to switch to the download channel.
        connection
            .packet_handshake::<SelectRadioChannelReplyPacket>(
                Duration::from_secs(2),
                3,
                SelectRadioChannelPacket::new(SelectRadioChannelPayload {
                    channel: RadioChannel::Download,
                }),
            )
            .await?;

        // Wait for the radio to switch channels before polling the connection
        sleep(Duration::from_millis(250)).await;

        // Poll the connection of the controller to ensure the radio has switched channels.
        let timeout = Duration::from_secs(5);
        select! {
            _ = sleep(timeout) => {
                return Err(CliError::DownloadChannelTimeout)
            }
            _ = async {
                while !is_connection_wireless(connection).await.unwrap_or(false) {
                    sleep(Duration::from_millis(250)).await;
                }
            } => {
                println!("Radio successfully switched to download channel.");
            }
        }
    }

    Ok(())
}

const PROGRESS_CHARS: &str = "⣿⣦⣀";

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

    // Attempt to switch to the download channel if we're uploading from a controller.
    switch_to_download_channel(connection).await?;

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
                // Update ini file progressbar. This code is a mess, yeah.
                let ini_progres_clone = Arc::clone(&ini_progress);
                let ini_timestamp_clone = Arc::clone(&ini_timestamp);
                Some(Box::new(move |percent| {
                    let ini_progres_clone = Arc::clone(&ini_progres_clone);
                    let ini_timestamp_clone = Arc::clone(&ini_timestamp_clone);

                    block_in_place(move || {
                        Handle::current().block_on(async move {
                            let progress = ini_progres_clone.lock().await;
                            let mut timestamp = ini_timestamp_clone.lock().await;

                            if timestamp.is_none() {
                                *timestamp = Some(Instant::now());
                            }

                            progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                            progress.set_position((percent * 100.0) as u64);
                        });
                    });
                }))
            },
            monolith_callback: {
                // Update bin file progressbar. This code is a mess, yeah.
                let bin_progres_clone = Arc::clone(&bin_progress);
                let bin_timestamp_clone = Arc::clone(&bin_timestamp);
                Some(Box::new(move |percent| {
                    let bin_progres_clone = Arc::clone(&bin_progres_clone);
                    let bin_timestamp_clone = Arc::clone(&bin_timestamp_clone);

                    block_in_place(move || {
                        Handle::current().block_on(async move {
                            let progress = bin_progres_clone.lock().await;
                            let mut timestamp = bin_timestamp_clone.lock().await;

                            if timestamp.is_none() {
                                *timestamp = Some(Instant::now());
                            }
                            progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                            progress.set_position((percent * 100.0) as u64);
                        });
                    });
                }))
            },
            hot_callback: None,
            cold_callback: None,
        })
        .await?;

    // Tell the progressbars that we're done once uploading is complete, allowing further messages to be printed to stdout.
    ini_progress.lock().await.finish();
    bin_progress.lock().await.finish();

    // Switch back to the Pit channel
    connection
        .packet_handshake::<SelectRadioChannelReplyPacket>(
            Duration::from_secs(2),
            1,
            SelectRadioChannelPacket::new(SelectRadioChannelPayload {
                channel: RadioChannel::Pit,
            }),
        )
        .await?;

    Ok(())
}
