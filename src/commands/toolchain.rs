use arm_toolchain::{
    cli::{confirm_install, ctrl_c_cancel, install_with_progress_bar},
    toolchain::ToolchainClient,
};
use owo_colors::OwoColorize;
use std::ffi::OsString;
use std::{env, ffi::OsStr};

use crate::{
    errors::CliError,
    settings::{Settings, ToolchainCfg, ToolchainType, workspace_metadata},
};

#[derive(Debug, clap::Subcommand)]
pub enum ToolchainCmd {
    Install,
    Env,
}

/// An environment variable key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvVarKey {
    /// The `PATH` environment variable.
    Path,
    /// Other environment variable.
    Other(&'static str),
}

impl From<&'static str> for EnvVarKey {
    fn from(value: &'static str) -> Self {
        Self::Other(value)
    }
}

pub fn setup_env(
    bin_dir: &OsStr,
    toolchain_type: ToolchainType,
    mut set_env_var: impl FnMut(EnvVarKey, &OsStr),
) {
    set_env_var(EnvVarKey::Path, bin_dir);
    set_env_var("CC_armv7a_vex_v5".into(), OsStr::new("clang"));
    set_env_var("AR_armv7a_vex_v5".into(), OsStr::new("llvm-ar"));

    let base_flags = [
        "--target=arm-none-eabi",
        "-mcpu=cortex-a9",
        "-mfpu=neon",
        "-mfloat-abi=hard",
        "-fno-pic",
        "-fno-exceptions",
        "-fno-rtti",
        "-funwind-tables",
    ];

    let mut c_flags = OsString::from(base_flags.join(" "));
    if let Some(old_flags) = env::var_os("CFLAGS_armv7a_vex_v5") {
        c_flags.push(" ");
        c_flags.push(old_flags);
    }

    set_env_var("CFLAGS_armv7a_vex_v5".into(), &c_flags);

    // Configure clang's multilib: the reason we don't have to specify which
    // libc sysroot we want (in the form of /path/to/sysroot/lib and …/include)
    // is because ARM clang is shipped with a multilib.yaml file which maps
    // target, CPU, and FPU flags to one of the many sysroots it bundles.
    // The bundled sysroots and multilib.yaml file are the primary things
    // that makes ARM clang distinct from upstream clang.

    // Note that these target flags passed to the linker are for static-lib
    // resolution only, not compiling C code. We have to set those flags
    // separately.

    // We use clang as a linker because ld.lld by itself doesn't include the
    // multilib logic for resolving static libraries.
    set_env_var("CARGO_TARGET_ARMV7A_VEX_V5_LINKER".into(), OsStr::new("clang"));

    // These flags are intended for use with LLVM 21.1.1, but may work on other
    // versions.
    let link_flags = base_flags
        .into_iter()
        .chain([
            // These flags + the C flags resolve to this sysroot:
            // `arm-none-eabi/armv7a_hard_vfpv3_d16_unaligned`
            // (hard float / VFP version 3 with 16 regs / unaligned access)
            "--target=armv7a-none-eabihf",
            // Disable crt0, we have vexide-startup.
            "-nostartfiles",
            // Explicit `-lc` required because Rust calls the linker with
            // `-nodefaultlibs` which disables libc, libm, etc.
            "-lc",
        ])
        .map(|f| format!("-Clink-arg={f}"))
        .collect::<Vec<String>>();

    let mut rust_flags = link_flags;
    rust_flags.push(format!("--cfg=vexide_toolchain=\"{}\"", toolchain_type));

    set_env_var("CARGO_TARGET_ARMV7A_VEX_V5_RUSTFLAGS".into(), OsStr::new(&rust_flags.join(" ")));
}

impl ToolchainCmd {
    pub async fn run(self) -> Result<(), CliError> {
        let client = ToolchainClient::using_data_dir().await?;

        let metadata = workspace_metadata().await;
        let settings = Settings::load(metadata.as_ref(), None)?;

        match self {
            Self::Install => {
                let Some(settings) = settings else {
                    return Err(CliError::NoCargoProject);
                };
                let Some(cfg) = settings.toolchain else {
                    return Err(CliError::NoToolchainConfigured);
                };

                Self::install(&client, &cfg).await
            }
            Self::Env => {
                let Some(settings) = settings else {
                    return Err(CliError::NoCargoProject);
                };
                let Some(cfg) = settings.toolchain else {
                    return Err(CliError::NoToolchainConfigured);
                };

                Self::env(&cfg).await
            }
        }
    }

    pub async fn install(client: &ToolchainClient, cfg: &ToolchainCfg) -> Result<(), CliError> {
        let ty = cfg.ty;
        let ToolchainType::LLVM = ty;

        let version = &cfg.version;

        let already_installed = client.install_path_for(version);
        if already_installed.exists() {
            println!(
                "Toolchain already installed: {}",
                format!("{ty:?} {version}").bold(),
            );
            return Ok(());
        }

        let release = client.get_release(version).await?;

        confirm_install(version, false).await?;

        let token = ctrl_c_cancel();
        install_with_progress_bar(client, &release, token.clone()).await?;
        token.cancel();

        println!(
            "Toolchain {} is ready for use.",
            format!("{ty:?} {version}").bold()
        );

        Ok(())
    }

    pub async fn env(cfg: &ToolchainCfg) -> Result<(), CliError> {
        let ToolchainType::LLVM = cfg.ty;

        let client = ToolchainClient::using_data_dir().await?;
        let toolchain = client.toolchain(&cfg.version).await?;

        setup_env(
            toolchain.host_bin_dir().as_os_str(),
            cfg.ty,
            |k, v| match k {
                EnvVarKey::Path => println!("PATH_add '{}'", v.display()),
                EnvVarKey::Other(key) => println!("export {key}='{}'", v.display()),
            }
        );

        Ok(())
    }
}
