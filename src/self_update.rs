use std::{
    borrow::Cow,
    env::{self, consts::EXE_SUFFIX},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use axoupdater::{AxoUpdater, AxoupdateError};
use miette::Diagnostic;
use thiserror::Error;
use tokio::{process::Command, sync::Mutex, task::block_in_place};

#[derive(Debug, Error, Diagnostic)]
pub enum SelfUpdateError {
    #[error("cargo-v5's updates are externally managed")]
    #[diagnostic(code(cargo_v5::self_update::unavailable))]
    SelfUpdateUnavailable {
        #[help]
        advice: &'static str,
    },

    #[error("Self-update failed")]
    #[diagnostic(code(cargo_v5::self_update::failure))]
    Axoupdate(#[from] AxoupdateError),
    #[error("Failed to run the update command")]
    #[diagnostic(code(cargo_v5::self_update::io))]
    Io(#[from] std::io::Error),
}

static AXOUPDATER: LazyLock<Mutex<AxoUpdater>> =
    LazyLock::new(|| Mutex::new(AxoUpdater::new_for("cargo-v5")));
pub static CURRENT_MODE: LazyLock<SelfUpdateMode> = LazyLock::new(SelfUpdateMode::current);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfUpdateMode {
    Axoupdate,
    Cargo,
    Unmanaged(Option<ExternalUpdateManager>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalUpdateManager {
    Homebrew,
}

fn cargo_bin_path() -> Option<PathBuf> {
    let cargo_home = env::var("CARGO_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::home_dir().map(|home| home.join(".cargo")))?;

    Some(cargo_home.join("bin"))
}

fn exe_name<'a>(string: impl Into<Cow<'a, str>>) -> Cow<'a, str> {
    if EXE_SUFFIX.is_empty() {
        string.into()
    } else {
        Cow::Owned(format!("{}{}", string.into(), EXE_SUFFIX))
    }
}

impl SelfUpdateMode {
    pub fn current() -> Self {
        if let Some(this_arg) = std::env::args().next()
            && !this_arg.is_empty()
        {
            // Check if managed by cargo
            if let Some(bin_path) = cargo_bin_path()
                && let Ok(expected_exe_path) =
                    bin_path.join(exe_name("cargo-v5").as_ref()).canonicalize()
                && let Ok(exe_path) = Path::new(&this_arg).canonicalize()
                && expected_exe_path == exe_path
            {
                return Self::Cargo;
            }

            // Check if managed by homebrew
            let homebrew_prefix =
                env::var("HOMEBREW_PREFIX").unwrap_or_else(|_| "/opt/homebrew/bin/".to_string());
            if this_arg.starts_with(&homebrew_prefix) {
                return SelfUpdateMode::Unmanaged(Some(ExternalUpdateManager::Homebrew));
            }
        }

        // Check if installed by shell script
        let mut updater = block_in_place(|| AXOUPDATER.blocking_lock());
        if updater.load_receipt().is_ok() {
            return Self::Axoupdate;
        }

        // Idk
        SelfUpdateMode::Unmanaged(None)
    }
}

pub async fn self_update() -> Result<(), SelfUpdateError> {
    eprintln!("Checking for updates...");

    let mode = *CURRENT_MODE;

    match mode {
        SelfUpdateMode::Axoupdate => {
            // This will redownload the installer shell script and run it again

            let mut updater = AXOUPDATER.lock().await;
            updater.run().await?;
            Ok(())
        }
        SelfUpdateMode::Cargo => {
            // Just spawn a cargo command to update for us

            let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

            let cargo_binstall_path =
                cargo_bin_path().map(|p| p.join(exe_name("cargo-binstall").as_ref()));

            let mut command = Command::new(cargo);

            if let Some(cargo_binstall_path) = cargo_binstall_path
                && let Ok(canonical_path) = cargo_binstall_path.canonicalize()
                && canonical_path.exists()
            {
                // Update with cargo-binstall because it's installed and faster
                command.arg("binstall");
            } else {
                command.arg("install").arg("--locked");
            }
            command.arg("cargo-v5");

            eprintln!("> {:?}", command.as_std());

            command.spawn()?.wait().await?;

            Ok(())
        }
        SelfUpdateMode::Unmanaged(manager) => Err(SelfUpdateError::SelfUpdateUnavailable {
            advice: match manager {
                Some(ExternalUpdateManager::Homebrew) => "run `brew upgrade cargo-v5`",
                None => "update cargo-v5 with your package manager or redownload the executable",
            },
        }),
    }
}
