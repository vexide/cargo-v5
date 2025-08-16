use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use fs_err::PathExt;
use image::GenericImageView;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use tokio::sync::Mutex;
use vex_v5_serial::{
    commands::file::DownloadFile,
    connection::{Connection, serial::SerialConnection},
    packets::{
        capture::{ScreenCapturePacket, ScreenCaptureReplyPacket},
        file::{FileTransferTarget, FileVendor},
    },
    string::FixedString,
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
        .packet_handshake::<ScreenCaptureReplyPacket>(
            Duration::from_millis(100),
            5,
            ScreenCapturePacket::new(()),
        )
        .await?
        .try_into_inner()?;

    // Grab the image data
    let cap = connection
        .execute_command(DownloadFile {
            file_name: FixedString::new("screen".to_string()).unwrap(),
            vendor: FileVendor::Sys,
            target: Some(FileTransferTarget::Cbuf),
            load_addr: 0,
            size: 512 * 272 * 4,
            progress_callback: Some({
                let progress = progress.clone();
                let timestamp = timestamp.clone();

                Box::new(move |percent| {
                    let progress = progress.try_lock().unwrap();
                    let mut timestamp = timestamp.try_lock().unwrap();

                    if timestamp.is_none() {
                        *timestamp = Some(Instant::now());
                    }

                    progress.set_prefix(format!("{:.2?}", timestamp.unwrap().elapsed()));
                    progress.set_position((percent * 100.0) as u64);
                })
            }),
        })
        .await
        .unwrap();

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

    info!(
        "Saved screenshot to {}",
        path.fs_err_canonicalize()?.display()
    );

    Ok(())
}
