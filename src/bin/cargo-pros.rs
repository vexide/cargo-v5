use cargo_pros::{build, launch_simulator, strip_binary};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

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
        #[clap(long, short)]
        simulator: bool,
        #[clap(last = true)]
        args: Vec<String>,
    },
    Sim {
        #[clap(long)]
        ui: Option<String>,
        #[clap(last = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli::Pros(args) = Cli::parse();
    let path = args.path;

    match args.command {
        Commands::Build { simulator, args } => {
            build(path, args, simulator, |path| {
                if !simulator {
                    strip_binary(path);
                }
            });
        }
        Commands::Sim { ui, args } => {
            let mut artifact = None;
            build(path.clone(), args, true, |new_artifact| {
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
    }

    Ok(())
}
