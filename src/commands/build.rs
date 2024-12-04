use log::info;
use object::{Object, ObjectSegment};
use std::process::{exit, Stdio};
use tokio::{process::Command, task::block_in_place};

use cargo_metadata::{
    camino::{Utf8Path, Utf8PathBuf},
    Message,
};
use clap::Args;
use fs_err::tokio as fs;

use crate::errors::CliError;

pub const TARGET_PATH: &str = "armv7a-vex-v5.json";

/// Common Cargo options to forward.
#[derive(Args, Debug)]
pub struct CargoOpts {
    /// Arguments forwarded to cargo.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "CARGO-OPTIONS"
    )]
    args: Vec<String>,
}

pub fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

async fn is_nightly_toolchain() -> bool {
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .await
        .unwrap();
    let rustc = String::from_utf8(rustc.stdout).unwrap();
    rustc.contains("nightly")
}

async fn has_wasm_target() -> bool {
    let Ok(rustup) = Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .output()
        .await
    else {
        return true;
    };
    let rustup = String::from_utf8(rustup.stdout).unwrap();
    rustup.contains("wasm32-unknown-unknown")
}

pub async fn build(
    path: &Utf8Path,
    opts: CargoOpts,
    for_simulator: bool,
    mut handle_executable: impl FnMut(Utf8PathBuf),
) {
    let target_path = path.join(TARGET_PATH);
    let mut build_cmd = std::process::Command::new(cargo_bin());
    build_cmd
        .current_dir(path)
        .arg("build")
        .arg("--message-format")
        .arg("json-render-diagnostics");

    if !is_nightly_toolchain().await {
        eprintln!("ERROR: vexide requires Nightly Rust features, but you're using stable.");
        eprintln!(" hint: this can be fixed by running `rustup override set nightly`");
        exit(1);
    }

    if for_simulator {
        if !has_wasm_target().await {
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
            fs::create_dir_all(target_path.parent().unwrap())
                .await
                .unwrap();
        }
        fs::write(&target_path, target).await.unwrap();
        build_cmd.arg("--target");
        build_cmd.arg(&target_path);

        build_cmd
            .arg("-Zbuild-std=core,alloc,compiler_builtins")
            .stdout(Stdio::piped());
    }

    build_cmd.args(opts.args);

    block_in_place(|| {
        let mut out = build_cmd.spawn().unwrap();
        let reader = std::io::BufReader::new(out.stdout.take().unwrap());
        for message in Message::parse_stream(reader) {
            if let Message::CompilerArtifact(artifact) = message.unwrap() {
                if let Some(binary_path) = artifact.executable {
                    handle_executable(binary_path);
                }
            }
        }
        let status = out.wait().unwrap();
        if !status.success() {
            exit(status.code().unwrap_or(1));
        }
    });
}

pub async fn objcopy(elf: &Utf8Path) -> Result<Utf8PathBuf, CliError> {
    info!("Creating binary: {}.bin", elf);
    // Read the ELF file built by cargo.
    let data = tokio::fs::read(elf).await?;

    // Parse the ELF file.
    let elf_data = object::File::parse(data.as_slice())?;

    // Get the loadable segments (program data) and sort them by virtual address.
    let mut program_segments: Vec<_> = elf_data.segments().collect();
    program_segments.sort_by_key(|seg| seg.address());

    // used to fill gaps between segments with zeros
    let mut last_addr = program_segments.first().unwrap().address();
    // final binary
    let mut bytes = Vec::new();

    // Concatenate all the segments into a single binary.
    for segment in program_segments {
        // Fill gaps between segments with zeros.
        let gap = segment.address() - last_addr;
        if gap > 0 {
            bytes.extend(vec![0; gap as usize]);
        }

        // Push the segment data to the binary.
        let data = segment.data()?;
        bytes.extend_from_slice(data);

        // data.len() can be different from segment.size() so we use the actual data length
        last_addr = segment.address() + data.len() as u64;
    }

    // Write the binary to a file.
    let bin = elf.with_extension("bin");
    tokio::fs::write(&bin, bytes).await?;

    Ok(bin)
}
