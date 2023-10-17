use cargo_metadata::{camino::Utf8PathBuf, Message};
use clap::{Args, Parser, Subcommand};
use fs_err as fs;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

#[derive(Parser, Debug)]
#[clap(bin_name = "cargo")]
enum Cli {
    /// Manage pros-rs projects
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

#[derive(Args, Debug)]
struct BuildOpts {
    #[arg(long, short)]
    release: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Build {
        #[clap(flatten)]
        build: BuildOpts,
    },
    Simulate {
        #[clap(flatten)]
        build: BuildOpts,
    },
}

cargo_subcommand_metadata::description!("Builds a pros-rs project");

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

const TARGET_PATH: &str = "target/armv7a-vexos-eabi.json";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli::Pros(args) = Cli::parse();
    let target_path = args.path.join(TARGET_PATH);
    let mut build_cmd = Command::new(cargo_bin());
    build_cmd
        .arg("build")
        .arg("--manifest-path")
        .arg(args.path.join("Cargo.toml"));

    match args.command {
        Commands::Build { build } => {
            if build.release {
                build_cmd.arg("--release");
            }

            let target = include_str!("armv7a-vexos-eabi.json");
            fs::create_dir_all(target_path.parent().unwrap()).unwrap();
            fs::write(&target_path, target).unwrap();
            build_cmd.arg("--target");
            build_cmd.arg(&target_path);

            build_cmd
                .arg("-Zbuild-std=core,alloc,compiler_builtins")
                .arg("--message-format")
                .arg("json-render-diagnostics")
                .stdout(Stdio::piped());

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
        Commands::Simulate { build } => {
            if build.release {
                build_cmd.arg("--release");
            }

            build_cmd.arg("--target").arg("wasm32-unknown-unknown");

            let mut out = build_cmd.spawn().unwrap();
            let reader = std::io::BufReader::new(out.stdout.take().unwrap());
            let mut wasm_path = None;
            for message in Message::parse_stream(reader) {
                if let Message::CompilerArtifact(artifact) = message.unwrap() {
                    if let Some(binary_path) = artifact.executable {
                        wasm_path = Some(binary_path);
                    }
                }
            }

            let wasm_path = wasm_path.expect("pros-simulator may not run libraries");

            pros_simulator::simulate(wasm_path.as_std_path())
                .await
                .unwrap();
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
