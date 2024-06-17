use anyhow::Context;
use cargo_metadata::{
    camino::{Utf8Path, Utf8PathBuf},
    Message,
};
use cfg_if::cfg_if;
use clap::Args;
use config::Config;
use fs::PathExt;
use fs_err as fs;
use inquire::{
    validator::{ErrorMessage, Validation},
    CustomType,
};
use std::{
    io::{self, ErrorKind},
    path::Path,
    process::{exit, Child, Command, Stdio},
};

pub mod config;

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

pub trait CommandExt {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child>;
}

impl CommandExt for Command {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child> {
        let command_name = self.get_program().to_string_lossy().to_string();
        self.spawn().map_err(|err| match err.kind() {
            ErrorKind::NotFound => {
                eprintln!("error: command `{}` not found", command_name);
                #[cfg(feature = "legacy-pros-rs-support")]
                {
                    eprintln!(
                        "Please refer to the documentation for installing pros-rs' dependencies on your platform."
                    );
                    eprintln!("> https://github.com/vexide/pros-rs#compiling");
                }
                #[cfg(not(feature = "legacy-pros-rs-support"))]
                {
                    eprintln!("Please refer to the documentation for installing vexide's dependencies on your platform.");
                    eprintln!("> https://github.com/vexide/vexide#compiling");
                }
                exit(1);
            }
            _ => err,
        })
    }
}

const TARGET_PATH: &str = "armv7a-vexos-eabi.json";

/// Common Cargo options to forward.
#[derive(Args, Debug)]
pub struct BuildOpts {
    #[clap(long, short)]
    release: bool,
    #[clap(long, short)]
    example: Option<String>,
    #[clap(long, short)]
    features: Vec<String>,
    #[clap(long, short)]
    all_features: bool,
    #[clap(long, short)]
    no_default_features: bool,
    #[clap(last = true)]
    args: Vec<String>,
}

pub fn build(
    path: &Utf8Path,
    opts: BuildOpts,
    for_simulator: bool,
    mut handle_executable: impl FnMut(Utf8PathBuf),
) {
    let target_path = path.join(TARGET_PATH);
    let mut build_cmd = Command::new(cargo_bin());
    build_cmd
        .current_dir(path)
        .arg("build")
        .arg("--message-format")
        .arg("json-render-diagnostics")
        .arg("--manifest-path")
        .arg(path.join("Cargo.toml").as_str());

    if !is_nightly_toolchain() {
        eprintln!("ERROR: pros-rs requires Nightly Rust features, but you're using stable.");
        eprintln!(" hint: this can be fixed by running `rustup override set nightly`");
        exit(1);
    }

    if for_simulator {
        if !has_wasm_target() {
            eprintln!(
                "ERROR: simulation requires the wasm32-unknown-unknown target to be installed"
            );
            eprintln!(
                " hint: this can be fixed by running `rustup target add wasm32-unknown-unknown`"
            );
            exit(1);
        }

        build_cmd
            .arg("--target")
            .arg("wasm32-unknown-unknown")
            .arg("-Zbuild-std=std,panic_abort")
            .arg("--config=build.rustflags=['-Ctarget-feature=+atomics,+bulk-memory,+mutable-globals','-Clink-arg=--shared-memory','-Clink-arg=--export-table']")
            .stdout(Stdio::piped());
    } else {
        #[cfg(feature = "legacy-pros-rs-support")]
        let target = include_str!("targets/pros-rs.json");
        #[cfg(not(feature = "legacy-pros-rs-support"))]
        let target = include_str!("targets/vexide.json");
        if !target_path.exists() {
            fs::create_dir_all(target_path.parent().unwrap()).unwrap();
            fs::write(&target_path, target).unwrap();
        }
        build_cmd.arg("--target");
        build_cmd.arg(&target_path);

        build_cmd
            .arg("-Zbuild-std=core,alloc,compiler_builtins")
            .stdout(Stdio::piped());
    }

    if opts.release {
        build_cmd.arg("--release");
    }

    if let Some(example) = opts.example {
        build_cmd.arg("--example").arg(example);
    }

    if !opts.features.is_empty() {
        build_cmd.arg("--features").arg(opts.features.join(","));
    }

    if opts.all_features {
        build_cmd.arg("--all-features");
    }

    if opts.no_default_features {
        build_cmd.arg("--no-default-features");
    }

    build_cmd.args(opts.args);

    let mut out = build_cmd.spawn_handling_not_found().unwrap();
    let reader = std::io::BufReader::new(out.stdout.take().unwrap());
    for message in Message::parse_stream(reader) {
        if let Message::CompilerArtifact(artifact) = message.unwrap() {
            if let Some(binary_path) = artifact.executable {
                handle_executable(binary_path);
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct UploadOpts {
    #[clap(long, short)]
    slot: Option<u8>,
    #[clap(long, short)]
    file: Option<Utf8PathBuf>,
    /// Convert the program to a stripped binary before uploading it.
    /// This is necessary for uploading an ELF that has not yet
    /// been processed.
    #[clap(long, short)]
    strip: bool,
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
        if opts.strip {
            artifact = Some(finish_binary(&path));
        } else {
            artifact = Some(path);
        }
    } else {
        build(path, opts.build_opts, false, |new_artifact| {
            let mut bin_path = new_artifact.clone();
            bin_path.set_extension("bin");
            artifact = Some(bin_path);
            finish_binary(&new_artifact);
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
        .spawn_handling_not_found()?
        .wait()?;
    Ok(())
}

#[cfg(all(target_os = "windows", feature = "legacy-pros-rs-support"))]
fn find_objcopy_path_windows() -> Option<String> {
    use std::path::PathBuf;
    let arm_install_path =
        PathBuf::from("C:\\Program Files (x86)\\Arm GNU Toolchain arm-none-eabi");
    let mut versions = fs::read_dir(arm_install_path).ok()?;
    let install = versions.next()?.ok()?.path();
    let path = install.join("bin").join("arm-none-eabi-objcopy.exe");
    Some(path.to_string_lossy().to_string())
}

#[cfg(feature = "legacy-pros-rs-support")]
fn objcopy_path() -> String {
    #[cfg(target_os = "windows")]
    let objcopy_path = find_objcopy_path_windows();

    #[cfg(not(target_os = "windows"))]
    let objcopy_path = None;

    objcopy_path.unwrap_or_else(|| "arm-none-eabi-objcopy".to_owned())
}

#[cfg(feature = "legacy-pros-rs-support")]
pub fn finish_binary(bin: &Utf8Path) -> Utf8PathBuf {
    println!("Stripping Binary: {}", bin);
    let objcopy = objcopy_path();
    let strip = std::process::Command::new(&objcopy)
        .args([
            "--strip-symbol=install_hot_table",
            "--strip-symbol=__libc_init_array",
            "--strip-symbol=_PROS_COMPILE_DIRECTORY",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP_INT",
            bin.as_str(),
            &format!("{}.stripped", bin),
        ])
        .spawn_handling_not_found()
        .unwrap();
    strip.wait_with_output().unwrap();
    let out = bin.with_extension("bin");
    let elf_to_bin = std::process::Command::new(&objcopy)
        .args([
            "-O",
            "binary",
            "-R",
            ".hot_init",
            &format!("{}.stripped", bin),
            out.as_str(),
        ])
        .spawn_handling_not_found()
        .unwrap();
    elf_to_bin.wait_with_output().unwrap();
    println!("Output binary: {}", out);
    out
}

#[cfg(not(feature = "legacy-pros-rs-support"))]
pub fn finish_binary(bin: &Utf8Path) -> Utf8PathBuf {
    println!("Stripping Binary: {}", bin);
    let out = bin.with_extension("bin");
    Command::new("rust-objcopy")
        .args(["-O", "binary", bin.as_str(), out.as_str()])
        .spawn_handling_not_found()
        .unwrap();
    println!("Output binary: {}", out);
    out
}

fn is_nightly_toolchain() -> bool {
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .unwrap();
    let rustc = String::from_utf8(rustc.stdout).unwrap();
    rustc.contains("nightly")
}

fn has_wasm_target() -> bool {
    let Ok(rustup) = std::process::Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .output()
    else {
        return true;
    };
    let rustup = String::from_utf8(rustup.stdout).unwrap();
    rustup.contains("wasm32-unknown-unknown")
}

#[cfg(target_os = "windows")]
fn find_simulator_path_windows() -> Option<String> {
    use std::path::PathBuf;
    let wix_path = PathBuf::from(r#"C:\Program Files\PROS Simulator\PROS Simulator.exe"#);
    if wix_path.exists() {
        return Some(wix_path.to_string_lossy().to_string());
    }
    // C:\Users\USER\AppData\Local\PROS Simulator
    let nsis_path = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
        .join("PROS Simulator")
        .join("PROS Simulator.exe");
    if nsis_path.exists() {
        return Some(nsis_path.to_string_lossy().to_string());
    }
    None
}

fn find_simulator() -> Command {
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            let mut cmd = Command::new("open");
            cmd.args(["-nWb", "rs.pros.simulator", "--args"]);
            cmd
        } else if #[cfg(target_os = "windows")] {
            Command::new(find_simulator_path_windows().expect("Simulator install not found"))
        } else {
            Command::new("pros-simulator")
        }
    }
}

pub fn launch_simulator(ui: Option<String>, workspace_dir: &Path, binary_path: &Path) {
    let mut command = if let Some(ui) = ui {
        Command::new(ui)
    } else {
        find_simulator()
    };
    command
        .arg("--code")
        .arg(binary_path.fs_err_canonicalize().unwrap())
        .arg(workspace_dir.fs_err_canonicalize().unwrap());

    let command_name = command.get_program().to_string_lossy().to_string();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    eprintln!("$ {} {}", command_name, args.join(" "));

    let res = command
        .spawn()
        .map_err(|err| match err.kind() {
            ErrorKind::NotFound => {
                eprintln!("Failed to start simulator:");
                eprintln!("error: command `{command_name}` not found");
                eprintln!();
                eprintln!("Please install PROS Simulator using the link below.");
                eprintln!("> https://github.com/pros-rs/pros-simulator-gui/releases");
                exit(1);
            }
            _ => err,
        })
        .unwrap()
        .wait();
    if let Err(err) = res {
        eprintln!("Failed to launch simulator: {}", err);
    }
}
