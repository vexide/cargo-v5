use object::{Object, ObjectSegment};
use std::process::{exit, Stdio};
use tokio::{process::Command, task::block_in_place};

use cargo_metadata::{camino::{Utf8Path, Utf8PathBuf}, Message};
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

pub async fn build(path: &Utf8Path, opts: CargoOpts, for_simulator: bool) -> miette::Result<Option<Utf8PathBuf>> {
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

    Ok(block_in_place::<_, Result<Option<Utf8PathBuf>, CliError>>(|| {
        let mut out = build_cmd.spawn()?;
        let reader = std::io::BufReader::new(out.stdout.take().unwrap());

        let mut binary_path_opt = None;

        for message in Message::parse_stream(reader) {
            if let Message::CompilerArtifact(artifact) = message? {
                if let Some(elf_artifact_path) = artifact.executable {
                    let binary = objcopy(&std::fs::read(&elf_artifact_path)?)?;
                    let binary_path = elf_artifact_path.with_extension("bin");

                    // Write the binary to a file.
                    std::fs::write(&binary_path, binary)?;
                    println!("     \x1b[1;92mObjcopy\x1b[0m {}", binary_path);

                    binary_path_opt = Some(binary_path)
                }
            }
        }

        let status = out.wait()?;
        if !status.success() {
            exit(status.code().unwrap_or(1));
        }

        Ok(binary_path_opt.clone())
    })?)
}

pub fn objcopy(elf: &[u8]) -> Result<Vec<u8>, CliError> {
    // Parse the ELF file.
    let elf_data = object::File::parse(elf)?;

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

    Ok(bytes)
}
