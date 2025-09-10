use object::{Object, ObjectSection, ObjectSegment};
use rustc_version::Channel;
use std::process::{Stdio, exit};
use tokio::task::block_in_place;

use cargo_metadata::{
    Message, PackageId,
    camino::{Utf8Path, Utf8PathBuf},
};
use clap::Args;
use fs_err::tokio as fs;

use crate::errors::CliError;

pub const TARGET_PATH: &str = "armv7a-vex-v5.json";

/// Common Cargo options to forward.
#[derive(Args, Debug)]
pub struct CargoOpts {
    /// Arguments forwarded to cargo.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "CARGO-OPTIONS"
    )]
    args: Vec<String>,
}

pub fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".to_owned().into())
}

pub struct BuildOutput {
    pub elf_artifact: Utf8PathBuf,
    pub bin_artifact: Utf8PathBuf,
    pub package_id: PackageId,
}

pub async fn build(path: &Utf8Path, opts: CargoOpts) -> miette::Result<Option<BuildOutput>> {
    let rustc_version_meta =
        rustc_version::version_meta().map_err(|e| CliError::RustcVersionError(e))?;
    let target_path = path.join(TARGET_PATH);
    let mut build_cmd = std::process::Command::new(cargo_bin());
    build_cmd
        .current_dir(path)
        .arg("build")
        .arg("--message-format")
        .arg("json-render-diagnostics");

    if !matches!(rustc_version_meta.channel, Channel::Nightly | Channel::Dev) {
        eprintln!("ERROR: vexide requires Nightly Rust features, but you're using stable.");
        eprintln!(" hint: this can be fixed by running `rustup override set nightly`");
        exit(1);
    }

    if !target_path.exists() {
        fs::create_dir_all(target_path.parent().unwrap())
            .await
            .unwrap();
    }

    // rustc 1.91.0 made a breaking change to the target spec format.
    //
    // NOTE: 1.91.0 nightlies before 2025-08-31 will still break with this check,
    // however my ass is not about to parse dates to determine that so I don't care.
    let target = if rustc_version_meta.semver.major >= 1 && rustc_version_meta.semver.minor >= 91 {
        include_str!("../targets/armv7a-vex-v5.json")
    } else {
        include_str!("../targets/armv7a-vex-v5-old.json")
    };

    fs::write(&target_path, target).await.unwrap();

    build_cmd
        .arg("--target")
        .arg(&target_path)
        .arg("-Zbuild-std=core,alloc,compiler_builtins")
        .arg("-Zbuild-std-features=compiler-builtins-mem")
        .stdout(Stdio::piped());

    build_cmd.args(opts.args);

    Ok(block_in_place::<_, Result<Option<BuildOutput>, CliError>>(
        || {
            let mut out = build_cmd.spawn()?;
            let reader = std::io::BufReader::new(out.stdout.take().unwrap());

            let mut output = None;

            for message in Message::parse_stream(reader) {
                match message? {
                    Message::CompilerArtifact(artifact) => {
                        if let Some(elf_artifact_path) = artifact.executable {
                            let binary = objcopy(&std::fs::read(&elf_artifact_path)?)?;
                            let binary_path = elf_artifact_path.with_extension("bin");

                            // Write the binary to a file.
                            std::fs::write(&binary_path, binary)?;
                            println!("     \x1b[1;92mObjcopy\x1b[0m {}", binary_path);

                            output = Some(BuildOutput {
                                bin_artifact: binary_path,
                                elf_artifact: elf_artifact_path,
                                package_id: artifact.package_id,
                            });
                        }
                    }
                    _ => {}
                }
            }

            let status = out.wait()?;
            if !status.success() {
                exit(status.code().unwrap_or(1));
            }

            Ok(output)
        },
    )?)
}

/// Implementation of `objcopy -O binary`.
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
