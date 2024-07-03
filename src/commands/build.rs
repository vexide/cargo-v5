use std::process::{exit, Command, Stdio};

use cargo_metadata::{
    camino::{Utf8Path, Utf8PathBuf},
    Message,
};
use clap::Args;
use fs_err as fs;

use crate::CommandExt;

pub const TARGET_PATH: &str = "armv7a-vexos-eabi.json";

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

pub fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
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
        let target = include_str!("../targets/armv7a-vex-v5.json");
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
