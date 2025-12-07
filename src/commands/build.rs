use arm_toolchain::toolchain::{ToolchainClient, ToolchainError};
use cargo_metadata::{Message as CompileMsg, PackageId};
use clap::Args;
use object::{Object, ObjectSection, ObjectSegment};
use owo_colors::OwoColorize;
use serde::Deserialize;
use serde_json::Deserializer;
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Stdio, exit},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

use crate::{
    commands::toolchain::ToolchainCmd,
    errors::CliError,
    fs,
    settings::{Settings, ToolchainCfg, ToolchainType},
};

/// Common Cargo options to forward.
#[derive(Args, Debug)]
pub struct BuildOpts {
    #[arg(short = 'T', long)]
    toolchain: Option<ToolchainCfg>,

    /// Arguments forwarded to cargo.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "CARGO-OPTIONS"
    )]
    args: Vec<String>,
}

pub fn cargo_bin() -> std::ffi::OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

async fn check_release_channel(cargo_bin: &OsStr) -> Result<(), CliError> {
    let rustc = Command::new(cargo_bin)
        .arg("--version")
        .output()
        .await
        .unwrap();
    let rustc = String::from_utf8(rustc.stdout).unwrap();
    let supported = rustc.contains("nightly") || rustc.contains("-dev");

    if !supported {
        return Err(CliError::UnsupportedReleaseChannel)?;
    }

    Ok(())
}

pub struct BuildOutput {
    pub elf_artifact_path: PathBuf,
    pub bin_artifact: PathBuf,
    pub package_id: PackageId,
}

pub async fn build(
    workspace_dir: &Path,
    opts: BuildOpts,
    root_settings: Option<&Settings>,
) -> Result<Option<BuildOutput>, CliError> {
    let cargo = cargo_bin();
    let BuildOpts { args, toolchain } = opts;

    check_release_channel(&cargo).await?;

    // Delegate to cargo build as normal, with some different defaults for arguments.

    let mut build_cmd = Command::new(cargo);
    build_cmd
        .current_dir(workspace_dir)
        .stdout(Stdio::piped())
        .args(["build", "--message-format", "json-render-diagnostics"]);

    let explicit_target_specified = args
        .iter()
        .take_while(|&arg| *arg != "--")
        .any(|arg| arg == "--target" || arg.starts_with("--target="));

    if !explicit_target_specified {
        build_cmd.arg("--target=armv7a-vex-v5");
    }

    // If there is a toolchain enabled, we need to put it in scope so that cc builds work correctly.

    let toolchain = toolchain
        .as_ref()
        .or(root_settings.and_then(|s| s.toolchain.as_ref()));

    if let Some(toolchain_cfg) = toolchain {
        let ToolchainType::LLVM = toolchain_cfg.ty;

        let client = ToolchainClient::using_data_dir().await?;

        let mut toolchain = client.toolchain(&toolchain_cfg.version).await;

        // If no toolchain installed, ask user to install it now. If they say "no",
        // the `run()` call will return an error that we just propagate.
        if matches!(toolchain, Err(ToolchainError::ToolchainNotInstalled { .. })) {
            ToolchainCmd::install(&client, toolchain_cfg).await?;
            toolchain = client.toolchain(&toolchain_cfg.version).await;
        }
        let toolchain = toolchain?;

        let mut path = OsString::from(toolchain.host_bin_dir());
        if let Some(old_path) = env::var_os("PATH") {
            path.push(":");
            path.push(old_path);
        }

        build_cmd.env("PATH", path);
        build_cmd.env("CC_armv7a_vex_v5", "clang");
        build_cmd.env("AR_armv7a_vex_v5", "llvm-ar");

        let base_flags = [
            "--target=arm-none-eabi",
            "-mcpu=cortex-a9",
            "-mfpu=neon",
            "-mfloat-abi=hard",
            "-fno-pic",
            "-fno-exceptions",
            "-fno-rtti",
            "-funwind-tables",
        ];

        let mut c_flags = OsString::from(base_flags.join(" "));
        if let Some(old_flags) = env::var_os("CFLAGS_armv7a_vex_v5") {
            c_flags.push(" ");
            c_flags.push(old_flags);
        }

        build_cmd.env("CFLAGS_armv7a_vex_v5", c_flags);

        // Configure clang's multilib: the reason we don't have to specify which
        // libc sysroot we want (in the form of /path/to/sysroot/lib and …/include)
        // is because ARM clang is shipped with a multilib.yaml file which maps
        // target, CPU, and FPU flags to one of the many sysroots it bundles.
        // The bundled sysroots and multilib.yaml file are the primary things
        // that makes ARM clang distinct from upstream clang.

        // Note that these target flags passed to the linker are for static-lib
        // resolution only, not compiling C code. We have to set those flags
        // separately.

        // We use clang as a linker because ld.lld by itself doesn't include the
        // multilib logic for resolving static libraries.
        build_cmd.arg("--config=target.armv7a-vex-v5.linker='clang'");

        // These flags are intended for use with LLVM 21.1.1, but may work on other
        // versions.
        let link_flags = base_flags
            .into_iter()
            .chain([
                // These flags + the C flags resolve to this sysroot:
                // `arm-none-eabi/armv7a_hard_vfpv3_d16_unaligned`
                // (hard float / VFP version 3 with 16 regs / unaligned access)
                "--target=armv7a-none-eabihf",
                // Disable crt0, we have vexide-startup.
                "-nostartfiles",
                // Explicit `-lc` required because Rust calls the linker with
                // `-nodefaultlibs` which disables libc, libm, etc.
                "-lc",
            ])
            .map(|f| format!("'-Clink-arg={f}'"))
            .collect::<Vec<String>>();

        let mut rust_flags = link_flags;
        rust_flags.push(format!("'--cfg=vexide_toolchain=\"{}\"'", toolchain_cfg.ty));

        // N.B. It's okay if the `target.<cfg>.rustflags` key is a duplicate to one in
        // the cargo config, they will still merge as expected.
        let flags_config = format!(
            "--config=target.armv7a-vex-v5.rustflags=[{}]",
            rust_flags.join(",")
        );

        build_cmd.arg(flags_config);
    }

    build_cmd.args(args);

    let mut child = build_cmd.spawn()?;
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    // Search for ELF executable outputs and objcopy them to the BIN format suitable for uploading.
    // This is the primary feature that cargo v5 build has over cargo build.

    let mut build_output = None;

    let mut line = String::new();
    while reader.read_line(&mut line).await? != 0 {
        // We attempt to interpret Cargo's stdout as a JSON message, but be forgiving for normal lines of text.

        let trimmed = line.strip_suffix('\n').unwrap_or(&line);
        let mut deser = Deserializer::from_str(trimmed);
        deser.disable_recursion_limit();

        let msg = CompileMsg::deserialize(&mut deser).ok();
        line.clear();

        if let Some(CompileMsg::CompilerArtifact(artifact)) = msg
            && let Some(executable_path) = artifact.executable
        {
            let exe_path = executable_path.into_std_path_buf();
            let (path, _) = objcopy_path(&exe_path).await?;

            build_output = Some(BuildOutput {
                bin_artifact: path,
                elf_artifact_path: exe_path,
                package_id: artifact.package_id,
            });
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }

    Ok(build_output)
}

/// Objcopy a path referencing an existing file.
///
/// Contains extra logic to pass-through `bin` files and
/// print status.
///
/// The BIN file is written back to the filesystem. Its path and data is returned.
pub async fn objcopy_path(path: &Path) -> Result<(PathBuf, Vec<u8>), CliError> {
    let contents = fs::read(path).await?;

    // Bin file: skip objcopy.
    if path.extension() == Some(OsStr::new("bin")) {
        return Ok((path.to_owned(), contents));
    }

    // Non-bin (elf) file: try to objcopy it to get a bin.
    let binary = objcopy(&contents)?;
    let binary_path = path.with_extension("bin");

    fs::write(&binary_path, &binary).await?;
    eprintln!("{:>12} {}", "Objcopy".green().bold(), binary_path.display());

    Ok((binary_path, binary))
}

/// Implementation of `objcopy -O binary`.
///
/// This converts an ELF executable to a BIN file, which is a simple byte-by-byte
/// representation of what the program will look like when loaded into memory.
///
/// This function will error if the ELF data is invalid.
pub fn objcopy(elf: &[u8]) -> Result<Vec<u8>, CliError> {
    let elf = object::File::parse(elf)?; // parse ELF file

    // First we need to find the loadable sections of the program
    // (the parts of the ELF that will be actually loaded into memory)
    let mut loadable_sections = elf
        .sections() // all sections regardless of if they lie in a PT_LOAD segment
        .filter(|section| {
            let Some((section_offset, section_size)) = section.file_range() else {
                // No file range = don't include as loadable section
                return false;
            };

            // To determine if a section is loadable, we'll check if this section lies
            // within the file range of a PT_LOAD segment by comparing file ranges.
            for segment in elf.segments() {
                let (segment_offset, segment_size) = segment.file_range();

                if segment_offset <= section_offset
                    && segment_offset + segment_size >= section_offset + section_size
                {
                    return true;
                }
            }

            false
        })
        .collect::<Vec<_>>();

    // No loadable sections implies that there's nothing in the binary.
    if loadable_sections.is_empty() {
        return Ok(Vec::new());
    }

    loadable_sections.sort_by_key(|section| section.address()); // TODO: verify this is necessary

    // Start/end address of where the binary will be loaded into memory.
    // Used to calculate the total binary size and section offset.
    let start_address = loadable_sections.first().unwrap().address();
    let end_address = {
        let last_section = loadable_sections.last().unwrap();
        last_section.address() + last_section.size()
    };

    // Pre-fill the binary with zeroes for the specified binary length
    // (determined by start address of first and end address of last loadable
    // sections respectively).
    let mut binary = vec![0; (end_address - start_address) as usize];

    for section in loadable_sections {
        let address = section.address();
        let start = address - start_address;
        let end = (address - start_address) + section.size();

        // Copy the loadable section's data into the output binary.
        binary[(start as usize)..(end as usize)].copy_from_slice(section.data()?);
    }

    Ok(binary)
}
