use cfg_if::cfg_if;
use fs::PathExt;
use fs_err as fs;
use std::{
    io::ErrorKind,
    path::Path,
    process::{exit, Command},
};

#[cfg(target_os = "windows")]
fn find_simulator_path_windows() -> Option<String> {
    use std::path::PathBuf;
    let wix_path = PathBuf::from(r#"C:\Program Files\PROS Simulator\PROS Simulator.exe"#);
    if wix_path.exists() {
        return Some(wix_path.to_string_lossy().to_string());
    }
    // C:\Users\USER\AppData\Local\PROS Simulator
    let nsis_path = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
        .join("PROS Simulator")
        .join("PROS Simulator.exe");
    if nsis_path.exists() {
        return Some(nsis_path.to_string_lossy().to_string());
    }
    None
}

fn find_simulator() -> Command {
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            let mut cmd = Command::new("open");
            cmd.args(["-nWb", "rs.pros.simulator", "--args"]);
            cmd
        } else if #[cfg(target_os = "windows")] {
            Command::new(find_simulator_path_windows().expect("Simulator install not found"))
        } else {
            Command::new("pros-simulator")
        }
    }
}

pub fn launch_simulator(ui: Option<String>, workspace_dir: &Path, binary_path: &Path) {
    let mut command = if let Some(ui) = ui {
        Command::new(ui)
    } else {
        find_simulator()
    };
    command
        .arg("--code")
        .arg(binary_path.fs_err_canonicalize().unwrap())
        .arg(workspace_dir.fs_err_canonicalize().unwrap());

    let command_name = command.get_program().to_string_lossy().to_string();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    eprintln!("$ {} {}", command_name, args.join(" "));

    let res = command
        .spawn()
        .map_err(|err| match err.kind() {
            ErrorKind::NotFound => {
                eprintln!("Failed to start simulator:");
                eprintln!("error: command `{command_name}` not found");
                eprintln!();
                eprintln!("Please install PROS Simulator using the link below.");
                eprintln!("> https://github.com/pros-rs/pros-simulator-gui/releases");
                exit(1);
            }
            _ => err,
        })
        .unwrap()
        .wait();
    if let Err(err) = res {
        eprintln!("Failed to launch simulator: {}", err);
    }
}
