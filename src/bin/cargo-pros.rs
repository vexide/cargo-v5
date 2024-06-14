use cargo_metadata::camino::Utf8PathBuf;
use cargo_pros::{build, finish_binary, launch_simulator, upload, UploadAction, UploadOpts};
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
        #[clap(last = true)]
        args: Vec<String>,
    },
    Upload {
        #[clap(long, short, default_value = "none")]
        action: UploadAction,

        #[command(flatten)]
        opts: UploadOpts,

        #[clap(last = true)]
        args: Vec<String>,
    },
    Sim {
        #[clap(long)]
        ui: Option<String>,
        #[clap(last = true)]
        args: Vec<String>,
    },
    /// Build, upload, run, and view the serial output of a vexide project.
    Run {
        #[command(flatten)]
        upload_opts: UploadOpts,

        #[clap(last = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "legacy-pros-rs-support")]
    println!("cargo-pros is using legacy pros-rs support. Please consider upgrading to the new vexide crate.");

    let Cli::Pros(args) = Cli::parse();
    let path = args.path;

    match args.command {
        Commands::Build { simulator, args } => {
            build(&path, &args, simulator, |path| {
                if !simulator {
                    finish_binary(&path);
                }
            });
        }
        Commands::Upload { opts, action, args } => upload(&path, opts, action, &args)?,
        Commands::Sim { ui, args } => {
            let mut artifact = None;
            build(&path, &args, true, |new_artifact| {
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
        Commands::Run { upload_opts, args } => {
            let term = thread::spawn(|| {
                sleep(Duration::from_millis(500));
                Command::new("pros")
                    .args(["terminal", "--raw"])
                    .spawn()
                    .expect("Failed to start terminal")
            });
            upload(&path, upload_opts, UploadAction::Run, &args)?;
            let mut term_child = term.join().unwrap();
            let term_res = term_child.wait()?;
            if !term_res.success() {
                eprintln!("Failed to start terminal: {:?}", term_res);
            }
        }
    }

    Ok(())
}
