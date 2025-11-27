use std::path::PathBuf;

use humansize::{BINARY, format_size};
use image::ImageError;
use inquire::InquireError;
use miette::Diagnostic;
use thiserror::Error;
use vex_v5_serial::protocol::{FixedStringSizeError, cdc2::Cdc2Ack};

use crate::commands::migrate::MigrateError;

#[non_exhaustive]
#[derive(Error, Diagnostic, Debug)]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(code(cargo_v5::io_error))]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(code(cargo_v5::serial_error))]
    SerialError(#[from] vex_v5_serial::serial::SerialError),

    #[error(transparent)]
    #[diagnostic(code(cargo_v5::cdc2_nack))]
    Nack(#[from] Cdc2Ack),

    #[error(transparent)]
    #[diagnostic(transparent)]
    MigrateError(#[from] MigrateError),

    #[cfg(feature = "fetch-template")]
    #[error(transparent)]
    #[diagnostic(code(cargo_v5::bad_response))]
    ReqwestError(#[from] reqwest::Error),

    #[cfg(feature = "fetch-template")]
    #[error("Received a malformed HTTP response")]
    #[diagnostic(code(cargo_v5::malformed_response))]
    MalformedResponse,

    #[error(transparent)]
    #[diagnostic(code(cargo_v5::image_error))]
    ImageError(#[from] ImageError),

    #[error(transparent)]
    #[diagnostic(code(cargo_v5::inquire))]
    Inquire(#[from] InquireError),

    #[error(transparent)]
    #[diagnostic(code(cargo_v5::fixed_string_size_error))]
    FixedStringSizeError(#[from] FixedStringSizeError),

    // TODO: Add source spans.
    #[error("Incorrect type for field `{field}` (expected {expected}, found {found}).")]
    #[diagnostic(
        code(cargo_v5::bad_field_type),
        help("The `{field}` field should be of type {expected}.")
    )]
    BadFieldType {
        /// Field name
        field: String,

        /// Expected type
        expected: String,

        /// Actual type
        found: String,
    },

    // TODO: Add optional source spans.
    #[error("The provided slot should be in the range [1, 8] inclusive.")]
    #[diagnostic(
        code(cargo_v5::slot_out_of_range),
        help(
            "The V5 Brain only has eight program slots. Adjust the `slot` field or argument to be a number from 1-8."
        )
    )]
    SlotOutOfRange,

    // TODO: Add source spans.
    #[error("{0} is not a valid icon.")]
    #[diagnostic(
        code(cargo_v5::invalid_icon),
        help("See `cargo v5 upload --help` for a list of valid icon identifiers.")
    )]
    InvalidIcon(String),

    #[error("{0} is not a valid upload strategy.")]
    #[diagnostic(
        code(cargo_v5::invalid_upload_strategy),
        help("See `cargo v5 upload --help` for a list of valid upload strategies.")
    )]
    InvalidUploadStrategy(String),

    #[error("No slot number was provided.")]
    #[diagnostic(
        code(cargo_v5::no_slot),
        help(
            "A slot number is required to upload programs. Try passing in a slot using the `--slot` argument, or setting the `package.v5.metadata.slot` field in your Cargo.toml."
        )
    )]
    NoSlot,

    #[error("ELF build artifact not found. Is this a binary crate?")]
    #[diagnostic(
        code(cargo_v5::no_artifact),
        help(
            "`cargo v5 build` should generate an ELF file in your project's `target` folder unless this is a library crate. You can explicitly supply a file to upload with the `--file` (`-f`) argument."
        )
    )]
    NoArtifact,

    #[error("No V5 devices found.")]
    #[diagnostic(
        code(cargo_v5::no_device),
        help(
            "Ensure that a V5 Brain or controller is plugged in and powered on with a stable USB connection, then try again."
        )
    )]
    NoDevice,

    #[error("cargo-v5 requires Nightly Rust features, but you're using stable.")]
    #[diagnostic(
        code(cargo_v5::unsupported_release_channel),
        help("Try switching to a nightly release channel with `rustup override set nightly`.")
    )]
    UnsupportedReleaseChannel,

    #[error("Output ELF file could not be parsed.")]
    #[diagnostic(code(cargo_v5::elf_parse_error))]
    ElfParseError(#[from] object::Error),

    #[error("Controller is stuck in radio channel 9.")]
    #[diagnostic(
        code(cargo_v5::radio_channel_stuck),
        help(
            "This is a bug in the controller's firmware. Please power cycle the controller to fix this."
        )
    )]
    RadioChannelStuck,

    #[error("Controller never switched radio channels.")]
    #[diagnostic(
        code(cargo_v5::radio_channel_disconnect_timeout),
        help(
            "Try running `cargo v5 upload` again. If the problem persists, power cycle your controller and Brain."
        )
    )]
    RadioChannelDisconnectTimeout,

    #[error("Controller never reconnected after switching radio channels.")]
    #[diagnostic(
        code(cargo_v5::radio_channel_reconnect_timeout),
        help(
            "Try running `cargo v5 upload` again. If the problem persists, power cycle your controller and Brain."
        )
    )]
    RadioChannelReconnectTimeout,

    #[cfg(feature = "field-control")]
    #[error("No V5 controllers found.")]
    #[diagnostic(
        code(cargo_v5::no_controller),
        help(
            "`cargo v5 fc` can only be ran over a controller connection. Make sure you have a controller plugged into USB, then try again."
        )
    )]
    NoController,

    #[cfg(feature = "field-control")]
    #[error("Attempted to change the match mode over a direct Brain connection.")]
    #[diagnostic(
        code(cargo_v5::brain_connection_set_match_mode),
        help(
            "This state should not be reachable and is a bug if encountered. Please report it to https://github.com/vexide/cargo-v5"
        )
    )]
    BrainConnectionSetMatchMode,

    #[error("Attempted to create a new project at {0}, but the directory is not empty.")]
    #[diagnostic(
        code(cargo_v5::project_dir_full),
        help("Try creating the project in a different directory or with a different name.")
    )]
    ProjectDirFull(PathBuf),

    #[error("Program exceeded the maximum differential upload size of 2MiB (program was {}).", format_size(*.0, BINARY))]
    #[diagnostic(
        code(cargo_v5::program_too_large),
        help(
            "This size limitation may change in the future. To upload larger binaries, switch to a monolith upload by specifying `--upload-strategy=monolith`."
        )
    )]
    ProgramTooLarge(usize),

    #[error("Patch exceeded the maximum size of 2MiB (patch was {}).", format_size(*.0, BINARY))]
    #[diagnostic(
        code(cargo_v5::patch_too_large),
        help("Try running a cold upload using `cargo v5 upload --cold`.")
    )]
    PatchTooLarge(usize),
}
