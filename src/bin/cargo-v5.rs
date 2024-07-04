use cargo_metadata::camino::Utf8PathBuf;
use cargo_v5::{
    commands::{
        build::{build, objcopy, BuildOpts},
        simulator::launch_simulator,
        upload::{upload, UploadAction, UploadOpts},
    },
    manifest::Config,
};
use clap::{Args, Parser, Subcommand};
use std::{
    process::Command,
    thread::{self, sleep},
    time::Duration,
};

cargo_subcommand_metadata::description!("Manage vexide projects");

#[derive(Parser, Debug)]
#[clap(bin_name = "cargo")]
enum Cli {
    /// Manage vexide projects.
    #[clap(version)]
    V5(Opt),
}

#[derive(Args, Debug)]
struct Opt {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = ".")]
    path: Utf8PathBuf,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build a vexide project for the V5 brain.
    Build {
        #[clap(long, short)]
        simulator: bool,
        #[clap(flatten)]
        opts: BuildOpts,
    },
    /// Build and upload a vexide project to the V5 brain.
    Upload {
        #[clap(long, short, default_value = "none")]
        action: UploadAction,

        #[command(flatten)]
        opts: UploadOpts,
    },
    /// Build a vexide project and run it in the simulator.
    Sim {
        #[clap(long)]
        ui: Option<String>,
        #[clap(flatten)]
        opts: BuildOpts,
    },
    /// Build, upload, start, and view the serial output of a vexide project.
    Run {
        #[command(flatten)]
        opts: UploadOpts,
    },
    /// Manage the configuration file.
    Config {
        #[clap(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommands {
    /// Prints the path of the configuration file.
    Print,
}

fn main() -> anyhow::Result<()> {
    let Cli::V5(args) = Cli::parse();
    let path = args.path;

    match args.command {
        Commands::Build { simulator, opts } => {
            build(&path, opts, simulator, |path| {
                if !simulator {
                    objcopy(&path);
                }
            });
        }
        Commands::Upload { opts, action } => upload(&path, opts, action, &Config::load()?, |_| {})?,
        Commands::Sim { ui, opts } => {
            let mut artifact = None;
            build(&path, opts, true, |new_artifact| {
                artifact = Some(new_artifact);
            });
            launch_simulator(
                ui.clone(),
                path.as_ref(),
                artifact
                    .expect("Binary target not found (is this a library?)")
                    .as_ref(),
            );
        }
        Commands::Run { opts } => {
            let mut term = None;
            upload(&path, opts, UploadAction::Run, &Config::load()?, |_| {
                term = Some(thread::spawn(|| {
                    // Delay allows the upload process some time to get started.
                    sleep(Duration::from_millis(500));
                    Command::new("pros")
                        .args(["terminal", "--raw"])
                        .spawn()
                        .expect("Failed to start terminal")
                }));
            })?;
            if let Some(term) = term {
                let mut term_child = term.join().unwrap();
                let term_res = term_child.wait()?;
                if !term_res.success() {
                    eprintln!("Failed to start terminal: {:?}", term_res);
                }
            }
        }
        Commands::Config { command } => match command {
            ConfigCommands::Print => {
                println!("{}", Config::path()?.display());
            }
        },
    }

    Ok(())
}
