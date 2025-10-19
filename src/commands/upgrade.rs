use std::{io::ErrorKind, path::Path};

use miette::Diagnostic;
use supports_color::Stream;
use thiserror::Error;
use toml_edit::{DocumentMut, Value, table, value};

use crate::errors::CliError;

mod vfs;

#[derive(Debug, Error, Diagnostic)]
pub enum UpgradeError {
    #[error("failed to parse toml file")]
    #[diagnostic(code(cargo_v5::upgrade::invalid_toml_file))]
    TomlParse(#[from] toml_edit::TomlError),
}

/// Applies all available upgrades to the workspace.
pub async fn upgrade_workspace(root: &Path) -> Result<(), CliError> {
    let mut files = vfs::FileOperationStore::default();

    update_cargo_config(root, &mut files).await?;
    update_vexide(root, &mut files).await?;

    // Print pending changes - in the future we will apply them too.
    let highlight = supports_color::on_cached(Stream::Stdout).is_some();

    println!();
    println!("{}", files.display(true, highlight).await);

    Ok(())
}

async fn open_toml(
    path: &Path,
    files: &mut vfs::FileOperationStore,
) -> Result<DocumentMut, CliError> {
    let file = files.read_to_string(&path).await;

    // If the config file is missing, make a new one.
    let doc = match file {
        Ok(contents) => contents
            .parse::<DocumentMut>()
            .map_err(UpgradeError::from)?,
        Err(err) if err.kind() == ErrorKind::NotFound => DocumentMut::new(),
        Err(other) => return Err(other)?,
    };

    Ok(doc)
}

/// Updates the user's Cargo config to use the Rust `armv7a-vex-v5` target
/// and deletes their old target JSON file.
async fn update_cargo_config(
    root: &Path,
    files: &mut vfs::FileOperationStore,
) -> Result<(), CliError> {
    let cargo_config = root.join(".cargo").join("config.toml");
    let mut document = open_toml(&cargo_config, files).await?;

    let mut build = table();
    let rustflags = Value::from_iter(vec!["-Clink-arg=-Tvexide.ld"]);
    build["rustflags"] = value(rustflags);

    document["build"] = build;

    let mut unstable = table();

    let build_std = Value::from_iter(vec!["std", "panic_abort"]);
    let build_std_features = Value::from_iter(vec!["compiler-builtins-mem"]);

    unstable["build-std"] = value(build_std);
    unstable["build-std-features"] = value(build_std_features);

    document["unstable"] = unstable;

    files.write(cargo_config, document.to_string()).await?;

    files
        .delete_if_exists(root.join("armv7a-vex-v5.json"))
        .await?;

    Ok(())
}

async fn update_vexide(root: &Path, files: &mut vfs::FileOperationStore) -> Result<(), CliError> {
    let cargo_toml = root.join("Cargo.toml");
    let mut document = open_toml(&cargo_toml, files).await?;

    let old_entry = document.get("dependencies").and_then(|d| d.get("vexide"));

    let old_features_array = old_entry
        .and_then(|v| v.get("features"))
        .and_then(|d| d.as_array());

    let default_features = old_entry
        .and_then(|v| v.get("default-features"))
        .and_then(|d| d.as_bool())
        .unwrap_or(true);

    let mut features = vec![];

    if default_features {
        features.push("full");
    }

    // Add features that were already enabled so the user doesn't have to
    // turn them back on manually.
    if let Some(old_features_array) = old_features_array {
        for value in old_features_array {
            let Some(mut feature) = value.as_str() else {
                continue;
            };

            // Apply renames.
            feature = match feature {
                "dangerous_motor_tuning" => "dangerous-motor-tuning",
                "backtraces" => "backtrace",
                "force_rust_libm" => continue, // Removed
                other => other,
            };

            features.push(feature);
        }
    }

    if default_features || features.contains(&"startup") {
        features.push("vex-sdk-jumptable");
        features.push("vex-sdk-mock");
    }

    let mut dependencies = table();

    let mut vexide = table();

    vexide["version"] = value("v0.8.0-alpha.2");
    vexide["features"] = value(Value::from_iter(features));
    if !default_features {
        vexide["default-features"] = value(default_features);
    }

    dependencies["vexide"] = vexide;

    document["dependencies"] = dependencies;

    files.write(cargo_toml, document.to_string()).await?;

    Ok(())
}
