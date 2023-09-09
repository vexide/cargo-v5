use cargo::{
    core::{Shell, Workspace},
    ops::CompileOptions,
    CargoResult, Config,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = ".")]
    path: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    Build,
}

fn main() -> CargoResult<()> {
    let args = Cli::parse();

    let config = Config::new(
        Shell::new(),
        std::env::current_dir().unwrap(),
        home::cargo_home().unwrap(),
    );

    let manifest_path = if args.path.is_relative() {
        std::env::current_dir()
            .unwrap()
            .join(args.path)
            .join("Cargo.toml")
    } else {
        args.path.join("Cargo.toml")
    };

    let ws = Workspace::new(&manifest_path, &config)?;

    if ws.is_virtual() {
        panic!("Virtual workspaces are not supported")
        // for ws in ws.members() {
        //     if ws.
        // }
    }

    match args.command {
        Commands::Build => {
            let compile_options =
                CompileOptions::new(&config, cargo::util::command_prelude::CompileMode::Build)?;

            //TODO: THIS DOESN'T WORK. We need to set the target to custom target "armv7a-vexos-eabi"

            let comp = cargo::ops::compile(&ws, &compile_options)?;

            for bin in comp.binaries {
                println!("Stripping Binary: {}", bin.path.display());
                let strip = std::process::Command::new("arm-none-eabi-objcopy")
                    .args(&[
                        "--strip-symbol=install_hot_table",
                        "--strip-symbol=__libc_init_array",
                        "--strip-symbol=_PROS_COMPILE_DIRECTORY",
                        "--strip-symbol=_PROS_COMPILE_TIMESTAMP",
                        "--strip-symbol=_PROS_COMPILE_TIMESTAMP_INT",
                        bin.path.to_str().expect("Invalid binary name"),
                        &format!("{}.stripped", bin.path.to_str().unwrap()), // We already panicked if file name contained invalid unicode, so we can unwrap.
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
                        &format!("{}.stripped", bin.path.to_str().unwrap()),
                        &format!("{}.bin", bin.path.to_str().unwrap()),
                    ])
                    .spawn()
                    .unwrap();
                elf_to_bin.wait_with_output().unwrap();
            }
        }
    }

    Ok(())
}
