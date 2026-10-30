use std::{path::Path, time::Duration};

use image::GenericImageView;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use log::info;
use vex_serial::{
    Connection,
    protocol::{
        FixedString,
        cdc2::{
            file::{FileTransferTarget, FileVendor},
            system::ScreenCapturePacket,
        },
    },
    serial::SerialConnection,
};
use vex_serial_util::file::blocking::download_file;

use crate::errors::CliError;

use super::upload::PROGRESS_CHARS;

pub fn screenshot(connection: &mut SerialConnection) -> Result<(), CliError> {
    let progress = ProgressBar::new(10000)
        .with_style(
            ProgressStyle::with_template("{msg:4} {percent_precise:>7}% {bar:40.blue} {prefix}")
                .unwrap() // Okay to unwrap, since this just validates style formatting.
                .with_key(
                    "elapsed_duration",
                    |state: &ProgressState, buf: &mut dyn std::fmt::Write| {
                        write!(buf, "{:?}", state.elapsed()).unwrap()
                    },
                )
                .progress_chars(PROGRESS_CHARS),
        )
        .with_message("CBUF");

    // Tell the brain we want to take a screenshot
    connection.handshake(
        ScreenCapturePacket { layer: None },
        Duration::from_millis(100),
        5,
    )??;

    // Grab the image data
    let cap = download_file(
        connection,
        FixedString::new("screen").unwrap(),
        512 * 272 * 4,
        FileVendor::Sys,
        FileTransferTarget::Cbuf,
        0x0,
        Some({
            let bar = progress.clone();
            move |percent| {
                bar.set_position((percent * 100.0) as u64);
            }
        }),
    )?;

    progress.finish();

    info!("Creating image file...");

    let colors = cap
        .chunks(4)
        .filter_map(|p| {
            if p.len() == 4 {
                // little endian
                let color = [p[2], p[1], p[0]];
                Some(color)
            } else {
                None
            }
        })
        .flatten()
        .collect::<Vec<_>>();

    let image = image::RgbImage::from_vec(512, 272, colors).unwrap();

    let path = Path::new("./screen.png");
    GenericImageView::view(&image, 0, 0, 480, 272)
        .to_image()
        .save(path)?;

    info!("Saved screenshot to {}", path.canonicalize()?.display());

    Ok(())
}
