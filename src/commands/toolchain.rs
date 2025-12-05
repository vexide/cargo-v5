use arm_toolchain::{
    cli::{confirm_install, ctrl_c_cancel, install_with_progress_bar},
    toolchain::ToolchainClient,
};
use owo_colors::OwoColorize;

use crate::{
    errors::CliError,
    metadata::{Metadata, ToolchainType},
};

#[derive(Debug, clap::Subcommand)]
pub enum ToolchainCmd {
    Install,
}

impl ToolchainCmd {
    pub async fn run(self) -> Result<(), CliError> {
        let client = ToolchainClient::using_data_dir().await?;
        let metadata = Metadata::for_root().await?;

        match self {
            Self::Install => Self::install(client, metadata).await,
        }
    }

    async fn install(client: ToolchainClient, metadata: Option<Metadata>) -> Result<(), CliError> {
        let Some(metadata) = metadata else {
            return Err(CliError::NoCargoProject);
        };
        let Some(cfg) = metadata.toolchain else {
            return Err(CliError::NoToolchainConfigured);
        };

        let ty = cfg.ty;
        let ToolchainType::LLVM = ty;

        let version = cfg.version;

        let already_installed = client.install_path_for(&version);
        if already_installed.exists() {
            println!(
                "Toolchain already installed: {}",
                format!("{ty:?} {version}").bold(),
            );
            return Ok(());
        }

        let release = client.get_release(&version).await?;

        confirm_install(&version, false).await?;

        let token = ctrl_c_cancel();
        install_with_progress_bar(&client, &release, token.clone()).await?;
        token.cancel();

        println!(
            "Toolchain {} is ready for use.",
            format!("{ty:?} {version}").bold()
        );

        Ok(())
    }
}
