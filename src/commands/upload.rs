use anyhow::Context;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use std::process::Command;

use crate::manifest::Config;

use super::build::{build, objcopy, BuildOpts};

#[derive(Args, Debug)]
pub struct UploadOpts {
    #[clap(long, short)]
    slot: Option<u8>,
    #[clap(long, short)]
    file: Option<Utf8PathBuf>,
    #[clap(flatten)]
    build_opts: BuildOpts,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum UploadAction {
    Screen,
    Run,
    #[default]
    None,
}
impl std::str::FromStr for UploadAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "screen" => Ok(UploadAction::Screen),
            "run" => Ok(UploadAction::Run),
            "none" => Ok(UploadAction::None),
            _ => Err(format!(
                "Invalid upload action. Found: {}, expected one of: screen, run, or none",
                s
            )),
        }
    }
}

pub fn upload(
    path: &Utf8Path,
    opts: UploadOpts,
    action: UploadAction,
    config: &Config,
    pre_upload: impl FnOnce(&Utf8Path),
) -> anyhow::Result<()> {
    let slot = opts.slot
        .or(config.defaults.slot)
        .or_else(|| {
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
        .context("No upload slot was provided; consider using the --slot flag or setting a default in the config file")?;
    let mut artifact = None;
    if let Some(path) = opts.file {
        artifact = Some(objcopy(&path));
    } else {
        build(path, opts.build_opts, false, |new_artifact| {
            let mut bin_path = new_artifact.clone();
            bin_path.set_extension("bin");
            artifact = Some(bin_path);
            objcopy(&new_artifact);
        });
    }
    let artifact =
        artifact.expect("Binary not found! Try explicitly providing one with --path (-p)");
    pre_upload(&artifact);
    Command::new("pros")
        .args([
            "upload",
            "--target",
            "v5",
            "--slot",
            &slot.to_string(),
            "--after",
            match action {
                UploadAction::Screen => "screen",
                UploadAction::Run => "run",
                UploadAction::None => "none",
            },
            artifact.as_str(),
        ])
        .spawn()?
        .wait()?;
    Ok(())
}