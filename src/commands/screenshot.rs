use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use image::GenericImageView;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use tokio::sync::Mutex;
use vex_v5_serial::{
    Connection,
    commands::file::download_file,
    protocol::{
        FixedString,
        cdc2::{
            file::{FileTransferTarget, FileVendor},
            system::ScreenCapturePacket,
        },
    },
    serial::SerialConnection,
};

use crate::errors::CliError;

use super::upload::PROGRESS_CHARS;

pub async fn screenshot(connection: &mut SerialConnection) -> Result<(), CliError> {
    let timestamp = Arc::new(Mutex::new(None));
    let progress = Arc::new(Mutex::new(
        ProgressBar::new(10000)
            .with_style(
                ProgressStyle::with_template(
                    "{msg:4} {percent_precise:>7}% {bar:40.blue} {prefix}",
                )
                .unwrap() // Okay to unwrap, since this just validates style formatting.
                .progress_chars(PROGRESS_CHARS),
            )
            .with_message("CBUF"),
    ));

    // Tell the brain we want to take a screenshot
    connection
        .handshake(
            ScreenCapturePacket { layer: None },
            Duration::from_millis(100),
            5,
        )
        .await??;

    // Grab the image data
    let cap = download_file(
        connection,
        FixedString::new("screen").unwrap(),
        512 * 272 * 4,
        FileVendor::Sys,
        FileTransferTarget::Cbuf,
        0x0,
        Some({
            let progress = progress.clone();
            let timestamp = timestamp.clone();

            move |percent| {
                let progress = progress.try_lock().unwrap();
                let mut timestamp = timestamp.try_lock().unwrap();

                if timestamp.is_none() {
                    *timestamp = Some(Instant::now());
                }

                progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                progress.set_position((percent * 100.0) as u64);
            }
        }),
    )
    .await?;

    progress.lock().await.finish();

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
