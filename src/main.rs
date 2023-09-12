use cargo_metadata::camino::Utf8PathBuf;
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
}

#[derive(Subcommand, Debug)]
enum Commands {
    Build,
}

cargo_subcommand_metadata::description!("Builds a pros-rs project");

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

const TARGET_PATH: &str = "target/armv7a-vexos-eabi.json";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli::Pros(args) = Cli::parse();
    let target_path = args.path.join(TARGET_PATH);

    let target = include_str!("armv7a-vexos-eabi.json");
    std::fs::write(&target_path, target).unwrap();

    match args.command {
        Commands::Build => {
            let mut build_cmd = Command::new(cargo_bin());
            build_cmd.stdout(std::process::Stdio::piped());
            build_cmd.arg("build");

            build_cmd.arg("--message-format");
            build_cmd.arg("json-render-diagnostics");

            build_cmd.arg("--manifest-path");
            build_cmd.arg(args.path.join("Cargo.toml"));

            build_cmd.arg("--target");
            build_cmd.arg(&target_path);

            build_cmd.arg("-Zbuild-std=core,alloc,compiler_builtins");

            let mut out = build_cmd.spawn().unwrap();
            let reader = std::io::BufReader::new(out.stdout.take().unwrap());
            for message in cargo_metadata::Message::parse_stream(reader) {
                match message.unwrap() {
                    cargo_metadata::Message::CompilerArtifact(artifact) => {
                        if let Some(binary_path) = artifact.executable {
                            strip_binary(binary_path);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn strip_binary(bin: Utf8PathBuf) {
    println!("Stripping Binary: {}", bin.clone());
    let strip = std::process::Command::new("arm-none-eabi-objcopy")
        .args(&[
            "--strip-symbol=install_hot_table",
            "--strip-symbol=__libc_init_array",
            "--strip-symbol=_PROS_COMPILE_DIRECTORY",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP",
            "--strip-symbol=_PROS_COMPILE_TIMESTAMP_INT",
            &bin.as_str(),
            &format!("{}.stripped", bin), // We already panicked if file name contained invalid unicode, so we can unwrap.
        ])
        .spawn()
        .unwrap();
    strip.wait_with_output().unwrap();
    let elf_to_bin = std::process::Command::new("arm-none-eabi-objcopy")
        .args(&[
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
