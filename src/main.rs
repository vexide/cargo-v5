use cargo_metadata::{camino::Utf8PathBuf, Message};
use clap::{Args, Parser, Subcommand};
use std::{path::PathBuf, process::Command};

#[derive(Parser, Debug)]
#[clap(bin_name = "cargo")]
enum Cli {
    /// A cargo subcommand for generating flamegraphs, using inferno
    #[clap(version)]
    Pros(Opt),
}

#[derive(Args, Debug)]
struct Opt {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = ".")]
    path: PathBuf,

    #[arg(long, short)]
    release: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Build,
    Simulate,
}

cargo_subcommand_metadata::description!("Builds a pros-rs project");

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

const TARGET_PATH: &str = "target/armv7a-vexos-eabi.json";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli::Pros(args) = Cli::parse();
    let target_path = args.path.join(TARGET_PATH);
    let mut build_cmd = Command::new(cargo_bin());
    build_cmd.stdout(std::process::Stdio::piped());
    build_cmd
        .arg("build")
        .arg("--message-format")
        .arg("json-render-diagnostics")
        .arg("--manifest-path")
        .arg(args.path.join("Cargo.toml"));

    if args.release {
        build_cmd.arg("--release");
    }

    match args.command {
        Commands::Build => {
            let target = include_str!("armv7a-vexos-eabi.json");
            std::fs::write(&target_path, target).unwrap();
            build_cmd.arg("--target");
            build_cmd.arg(&target_path);

            build_cmd.arg("-Zbuild-std=core,alloc,compiler_builtins");

            // Add macOS headers to the include path.
            // This is required to build pros-sys because it uses headers
            // like <errno.h>.
            if cfg!(target_os = "macos") {
                let sdk_commad = std::process::Command::new("xcrun")
                    .args(["--sdk", "macosx", "--show-sdk-path"])
                    .output()
                    .expect("macOS sdk should be installed (try `xcode-select --install`)");
                let sdk_path = String::from_utf8(sdk_commad.stdout).unwrap();
                build_cmd.env("CPATH", sdk_path.trim());
            }

            let mut out = build_cmd.spawn().unwrap();
            let reader = std::io::BufReader::new(out.stdout.take().unwrap());
            for message in Message::parse_stream(reader) {
                if let Message::CompilerArtifact(artifact) = message.unwrap() {
                    if let Some(binary_path) = artifact.executable {
                        strip_binary(binary_path);
                    }
                }
            }
        }
        Commands::Simulate => {
            build_cmd.arg("--target").arg("wasm32-unknown-unknown");

            build_cmd.spawn().unwrap();
        }
    }

    Ok(())
}

fn strip_binary(bin: Utf8PathBuf) {
    println!("Stripping Binary: {}", bin.clone());
    let strip = std::process::Command::new("arm-none-eabi-objcopy")
        .args([
            "--strip-symbol=install_hot_table",
            "--strip-symbol=__libc_init_array",
            "--strip-symbol=_PROS_COMPILE_DIRECTORY",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP_INT",
            bin.as_str(),
            &format!("{}.stripped", bin),
        ])
        .spawn()
        .unwrap();
    strip.wait_with_output().unwrap();
    let elf_to_bin = std::process::Command::new("arm-none-eabi-objcopy")
        .args([
            "-O",
            "binary",
            "-R",
            ".hot_init",
            &format!("{}.stripped", bin),
            &format!("{}.bin", bin),
        ])
        .spawn()
        .unwrap();
    elf_to_bin.wait_with_output().unwrap();
}
