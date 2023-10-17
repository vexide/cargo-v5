use cargo_metadata::{camino::Utf8PathBuf, Message};
use clap::{Args, Parser, Subcommand};
use fs_err as fs;
use std::{
    path::PathBuf,
    process::{Command, Stdio, Child, exit}, io::{self, ErrorKind},
};

cargo_subcommand_metadata::description!("Manage pros-rs projects");

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

#[derive(Subcommand, Debug)]
enum Commands {
    Build {
        #[clap(last = true)]
        args: Vec<String>,
    },
    Simulate {
        #[clap(last = true)]
        args: Vec<String>,
    },
}

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

trait CommandExt {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child>;
}

impl CommandExt for Command {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child> {
        let command_name = self.get_program().to_string_lossy().to_string();
        self.spawn().map_err(|err| match err.kind() {
            ErrorKind::NotFound => {
                eprintln!("error: command `{}` not found", command_name);
                eprintln!("Please refer to the documentation for installing pros-rs on your platform.");
                eprintln!("> https://github.com/pros-rs/pros-rs#compiling");
                exit(1);
            }
            _ => err,
        })
    }
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

    if !is_nightly_toolchain() {
        eprintln!("warn: pros-rs currently requires Nightly Rust features.");
        eprintln!("Switch project to nightly?");
        eprint!("(rustup override set nightly) [Y/n]: ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();
        if input == "y" || input.is_empty() {
            std::process::Command::new("rustup")
                .arg("override")
                .arg("set")
                .arg("nightly")
                .output()
                .unwrap();
            assert!(is_nightly_toolchain());
        }
    }

    match args.command {
        Commands::Build { args } => {
            build_cmd.args(args);

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

            let mut out = build_cmd.spawn_handling_not_found().unwrap();
            let reader = std::io::BufReader::new(out.stdout.take().unwrap());
            for message in Message::parse_stream(reader) {
                if let Message::CompilerArtifact(artifact) = message.unwrap() {
                    if let Some(binary_path) = artifact.executable {
                        strip_binary(binary_path);
                    }
                }
            }
        }
        Commands::Simulate { args } => {
            if !has_wasm_target() {
                eprintln!("warn: simulation requires the wasm32-unknown-unknown target to be installed");
                eprintln!("Install using rustup?");
                eprint!("(rustup target add wasm32-unknown-unknown) [Y/n]: ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                let input = input.trim().to_lowercase();
                if input == "y" || input.is_empty() {
                    std::process::Command::new("rustup")
                        .arg("override")
                        .arg("set")
                        .arg("nightly")
                        .output()
                        .unwrap();
                    assert!(has_wasm_target());
                }
            }

            build_cmd.args(args);

            build_cmd
            .arg("--target")
            .arg("wasm32-unknown-unknown")
            .stdout(Stdio::piped());

            let mut out = build_cmd.spawn_handling_not_found().unwrap();
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
        .spawn_handling_not_found()
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
        .spawn_handling_not_found()
        .unwrap();
    elf_to_bin.wait_with_output().unwrap();
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
        .output() else {
        return false;
    };
    let rustup = String::from_utf8(rustup.stdout).unwrap();
    rustup.contains("wasm32-unknown-unknown")
}
