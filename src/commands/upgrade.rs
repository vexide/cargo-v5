use std::{
    env::{self, home_dir},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use fs_err::tokio as fs;
use miette::Diagnostic;
use supports_color::Stream;
use thiserror::Error;
use toml_edit::{Document, DocumentMut, Item, Table, Value, table, value};

use crate::errors::CliError;

mod vfs;

#[derive(Debug, Error, Diagnostic)]
pub enum UpgradeError {
    #[error("failed to parse toml file")]
    #[diagnostic(code(cargo_v5::upgrade::invalid_toml_file))]
    TomlParse(#[from] toml_edit::TomlError),
}

struct ChangesCtx {
    fs: vfs::FileOperationStore,
    will_disable_rustup_override: bool,
}

impl ChangesCtx {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            fs: vfs::FileOperationStore::new(root),
            will_disable_rustup_override: false,
        }
    }
}

/// Applies all available upgrades to the workspace.
pub async fn upgrade_workspace(root: &Path) -> Result<(), CliError> {
    let mut ctx = ChangesCtx::new(root);

    update_cargo_config(&mut ctx).await?;
    update_vexide(&mut ctx).await?;
    update_rust(&mut ctx).await?;

    // Print pending changes - in the future we will apply them too.
    let highlight = supports_color::on_cached(Stream::Stdout).is_some();

    println!();
    println!("{}", ctx.fs.display(true, highlight).await);
    println!("- Will disable Rustup override: {}", ctx.will_disable_rustup_override);

    Ok(())
}

async fn update_rust(ctx: &mut ChangesCtx) -> Result<(), CliError> {
    let files = &mut ctx.fs;

    files
        .edit_toml("rust-toolchain.toml", |document| {
            let toolchain = document.table("toolchain");
            toolchain["channel"] = value("nightly-2025-09-26");
        })
        .await?;

    let has_override = rustup_has_override_for_path(ctx.fs.root())
        .await
        .unwrap_or(false);
    ctx.will_disable_rustup_override = has_override;

    Ok(())
}

async fn rustup_has_override_for_path(path: &Path) -> Option<bool> {
    let absolute_path = fs::canonicalize(path).await.ok()?;

    let mut rustup_home = env::var("RUSTUP_HOME").ok().map(PathBuf::from);
    if rustup_home.is_none() {
        rustup_home = home_dir().map(|dir| dir.join(".rustup"));
    }

    let settings_path = rustup_home?.join("settings.toml");
    let contents = fs::read_to_string(settings_path).await.ok()?;

    let settings = Document::parse(contents).ok()?;

    let overrides = settings.get("overrides")?.as_table()?;

    let has_override_for_path = overrides.contains_key(absolute_path.to_str()?);

    Some(has_override_for_path)
}

/// Updates the user's Cargo config to use the Rust `armv7a-vex-v5` target
/// and deletes their old target JSON file.
async fn update_cargo_config(ctx: &mut ChangesCtx) -> Result<(), CliError> {
    let fs = &mut ctx.fs;

    fs.edit_toml(".cargo/config.toml", |document| {
        let build = document.table("build");

        let rustflags = Value::from_iter(vec!["-Clink-arg=-Tvexide.ld"]);
        build["rustflags"] = value(rustflags);

        let unstable = document.table("unstable");

        let build_std = Value::from_iter(vec!["std", "panic_abort"]);
        let build_std_features = Value::from_iter(vec!["compiler-builtins-mem"]);

        unstable["build-std"] = value(build_std);
        unstable["build-std-features"] = value(build_std_features);
    })
    .await?;

    fs.delete_if_exists("armv7a-vex-v5.json").await?;

    Ok(())
}

async fn update_vexide(ctx: &mut ChangesCtx) -> Result<(), CliError> {
    let fs = &mut ctx.fs;

    fs.edit_toml("Cargo.toml", |document| {
        let old_entry = document.get("dependencies").and_then(|d| d.get("vexide"));

        let old_features_array = old_entry
            .and_then(|v| v.get("features"))
            .and_then(|d| d.as_array());

        let default_features = old_entry
            .and_then(|v| v.get("default-features"))
            .and_then(|d| d.as_bool())
            .unwrap_or(true);

        let mut features = Vec::<Value>::new();
        let mut include_sdk_features = default_features;

        if default_features {
            features.push("full".into());
        }

        // Add features that were already enabled so the user doesn't have to
        // turn them back on manually.
        if let Some(old_features_array) = old_features_array {
            for item in old_features_array {
                let Some(mut feature) = item.as_str() else {
                    continue;
                };

                // Apply renames.
                feature = match feature {
                    "dangerous_motor_tuning" => "dangerous-motor-tuning",
                    "backtraces" => "backtrace",
                    "force_rust_libm" => continue, // Removed
                    other => other,
                };

                if feature == "startup" {
                    include_sdk_features = true;
                }

                features.push(feature.into());
            }
        }

        if include_sdk_features {
            features.push("vex-sdk-jumptable".into());
            features.push("vex-sdk-mock".into());
        }

        let dependencies = document.table("dependencies");

        let mut vexide = table();

        vexide["version"] = value("v0.8.0-alpha.2");
        vexide["features"] = value(Value::from_iter(features));
        if !default_features {
            vexide["default-features"] = value(default_features);
        }

        dependencies["vexide"] = vexide;
    })
    .await
}

trait TableExt {
    fn table(&mut self, key: &str) -> &mut Table;
}

impl TableExt for Table {
    fn table(&mut self, key: &str) -> &mut Table {
        let value = self.entry(key).or_insert_with(table);

        // Cast to table
        *value = std::mem::take(value)
            .into_table()
            .unwrap_or_default()
            .into();

        value.as_table_mut().unwrap()
    }
}
