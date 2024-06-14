use cargo_metadata::camino::Utf8PathBuf;
use cargo_pros::{
    build, finish_binary, launch_simulator, upload, BuildOpts, UploadAction, UploadOpts,
};
use clap::{Args, Parser, Subcommand};
use std::{
    process::Command,
    thread::{self, sleep},
    time::Duration,
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
    path: Utf8PathBuf,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Build {
        #[clap(long, short)]
        simulator: bool,
        #[clap(flatten)]
        opts: BuildOpts,
    },
    Upload {
        #[clap(long, short, default_value = "none")]
        action: UploadAction,

        #[command(flatten)]
        opts: UploadOpts,
    },
    Sim {
        #[clap(long)]
        ui: Option<String>,
        #[clap(flatten)]
        opts: BuildOpts,
    },
    /// Build, upload, run, and view the serial output of a vexide project.
    Run {
        #[command(flatten)]
        upload_opts: UploadOpts,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "legacy-pros-rs-support")]
    println!("cargo-pros is using legacy pros-rs support. Please consider upgrading to the new vexide crate.");

    let Cli::Pros(args) = Cli::parse();
    let path = args.path;

    match args.command {
        Commands::Build { simulator, opts } => {
            build(&path, opts, simulator, |path| {
                if !simulator {
                    finish_binary(&path);
                }
            });
        }
        Commands::Upload { opts, action } => upload(&path, opts, action)?,
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
        Commands::Run { upload_opts } => {
            let term = thread::spawn(|| {
                // Delay allows the upload process some time to get started.
                sleep(Duration::from_millis(500));
                Command::new("pros")
                    .args(["terminal", "--raw"])
                    .spawn()
                    .expect("Failed to start terminal")
            });
            upload(&path, upload_opts, UploadAction::Run)?;
            let mut term_child = term.join().unwrap();
            let term_res = term_child.wait()?;
            if !term_res.success() {
                eprintln!("Failed to start terminal: {:?}", term_res);
            }
        }
    }

    Ok(())
}
