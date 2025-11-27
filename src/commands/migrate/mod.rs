use std::{
    env::{self, home_dir},
    fmt::Display,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use cargo_metadata::Metadata;
use fs_err::tokio as fs;
use miette::Diagnostic;
use semver::Version;
use supports_color::Stream;
use thiserror::Error;
use tokio::{process::Command, task::block_in_place};
use toml_edit::{Document, DocumentMut, Item, Table, Value, table, value};

use crate::errors::CliError;

mod source_code;
mod vfs;

/// Applies all available upgrades to the workspace.
pub async fn migrate_workspace(root: &Path) -> Result<(), CliError> {
    let metadata_task = block_in_place(|| {
        cargo_metadata::MetadataCommand::new()
            .current_dir(root)
            .exec()
            .ok()
    });

    let Some(metadata) = metadata_task else {
        return Err(MigrateError::Metadata.into());
    };

    let mut ctx = ChangesCtx::new(&metadata.workspace_root);

    update_vexide(&mut ctx, &metadata).await?;
    update_rust(&mut ctx).await?;
    update_cargo_config(&mut ctx).await?;
    source_code::update_targets(&mut ctx, &metadata).await?;

    // Print pending changes - in the future we will apply them too.
    let highlight = supports_color::on_cached(Stream::Stdout).is_some();

    println!(
        "The upgrade tool will now update your project configuration to the vexide v0.8 recommended defaults."
    );
    println!(
        "After applying these changes, make sure to check out the upgrade guide on the vexide website"
    );
    println!("for instructions on how to update your project's code!");
    println!("Changes Summary:");
    for desc in &ctx.description {
        println!("  - {desc}");
    }
    if ctx.description.is_empty() {
        println!("  - (No changes)");
        println!();
        return Ok(());
    }
    println!();

    loop {
        let confirmation: inquire::Select<'_, ConfirmOptions> = inquire::Select::new(
            "Apply changes?",
            vec![
                ConfirmOptions::Confirm,
                ConfirmOptions::ViewDiff,
                ConfirmOptions::Abort,
            ],
        );

        let reply = block_in_place(|| confirmation.prompt_skippable())?.unwrap_or_default();

        match reply {
            ConfirmOptions::Confirm => {
                ctx.apply().await?;
                break;
            }
            ConfirmOptions::ViewDiff => println!("{}", ctx.fs.display(true, highlight).await),
            ConfirmOptions::Abort => {
                break;
            }
        }
    }

    Ok(())
}

#[derive(Default)]
enum ConfirmOptions {
    Confirm,
    ViewDiff,
    #[default]
    Abort,
}

impl Display for ConfirmOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConfirmOptions::Confirm => "Confirm",
            ConfirmOptions::ViewDiff => "View Changes",
            ConfirmOptions::Abort => "Abort",
        })
    }
}

async fn update_rust(ctx: &mut ChangesCtx) -> Result<(), CliError> {
    ctx.edit_toml("rust-toolchain.toml", |document, ctx| {
        let latest = "nightly-2025-11-26";

        let toolchain = document.table("toolchain");
        let old_channel = toolchain.insert("channel", value(latest));

        if let Some(old_channel) = old_channel
            && let Some(old_channel) = old_channel.as_str()
            && old_channel != latest
        {
            ctx.describe(format!("Updated to Rust {}", latest));
        }
    })
    .await?;

    let has_override = rustup_has_override_for_path(ctx.fs.root())
        .await
        .unwrap_or(false);
    if has_override {
        ctx.will_disable_rustup_override = has_override;
        ctx.describe("Disabled the Rustup override for this project.");
    }

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
    ctx.edit_toml(".cargo/config.toml", |document, ctx| {
        // Disable forced target.

        let build = document.table("build");
        build.set_implicit(true);

        let old_target = build.remove("target");
        if old_target.is_some() {
            ctx.describe("Enabled desktop unit testing");
        }

        // Move/add all required rustflags to target config.

        let rustflags = vec!["-Clink-arg=-Tvexide.ld"];

        if let Some(old_rustflags) = build.get_mut("rustflags")
            && let Some(flag_array) = old_rustflags.as_array_mut()
        {
            // If the normal rustflags have any of these items, just remove them because
            // that's probably a mistake.

            #[rustfmt::skip]
            flag_array.retain(|item| {
                // Only keep items that aren't vexide-specific.

                let is_vexide_flag = rustflags.iter().any(|&vexide_flag| {
                    item.as_str().is_some_and(|flag| flag == vexide_flag)
                });

                !is_vexide_flag
            });

            if flag_array.is_empty() {
                build.remove("rustflags");
            }
        }

        // Now set up the target table and put the rustflags in.
        let target = document.table("target");
        target.set_implicit(true);
        target.set_position(-1);

        let this_target = target.table(r#"cfg(target_os = "vexos")"#);

        let rustflags_are_updated = toml_item_eq_strings(this_target.get("rustflags"), &rustflags);
        if !rustflags_are_updated {
            let rustflags = Value::from_iter(rustflags);
            this_target["rustflags"] = value(rustflags);
            ctx.describe("Enabled the vexide v0.8.0 memory layout");
        }

        // Build-std config.
        let unstable = document.table("unstable");

        let build_std = vec!["std", "panic_abort"];
        let build_std_features = vec!["compiler-builtins-mem"];

        let unstable_is_updated = toml_item_eq_strings(unstable.get("build-std"), &build_std)
            && toml_item_eq_strings(unstable.get("build-std-features"), &build_std_features);

        if !unstable_is_updated {
            let build_std = Value::from_iter(vec!["std", "panic_abort"]);
            unstable["build-std"] = value(build_std);

            let build_std_features = Value::from_iter(vec!["compiler-builtins-mem"]);
            unstable["build-std-features"] = value(build_std_features);

            ctx.describe("Added the Rust Standard Library as a dependency");
        }
    })
    .await?;

    ctx.fs.delete_if_exists("armv7a-vex-v5.json").await?;

    Ok(())
}

async fn update_vexide(ctx: &mut ChangesCtx, metadata: &Metadata) -> Result<(), CliError> {
    let latest = "0.8.0-alpha.2";

    let mut packages = metadata.packages.iter();

    if let Some(current_vexide) = packages.find(|p| p.name.as_str() == "vexide") {
        let supported_by_tool = Version::new(0, 7, 0);
        let latest = Version::parse(latest).unwrap();

        let is_eligible =
            current_vexide.version < latest && current_vexide.version >= supported_by_tool;
        if !is_eligible {
            log::warn!("vexide {} not eligible for upgrade", current_vexide.version);
            return Ok(());
        }
    }

    ctx.edit_toml("Cargo.toml", |document, ctx| {
        // Update to Rust 2024 edition (required by 0.8.0).
        _ = document
            .table("package")
            .insert("edition", "2024".to_string().into());
        ctx.describe("Updated to Rust 2024 edition");

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

        dependencies.remove("vexide");

        let mut vexide = Table::new();

        vexide["version"] = value(latest);
        vexide["features"] = value(Value::from_iter(features));
        if !default_features {
            vexide["default-features"] = value(default_features);
        }

        dependencies["vexide"] = vexide.into_inline_table().into();

        ctx.describe(format!("Updated to vexide v{latest}"));
    })
    .await
}

#[derive(Debug, Error, Diagnostic)]
pub enum MigrateError {
    #[error("failed to parse toml file")]
    #[diagnostic(code(cargo_v5::upgrade::invalid_toml_file))]
    TomlParse(#[from] toml_edit::TomlError),
    #[error("Cannot determine the current Cargo workspace")]
    #[diagnostic(code(cargo_v5::upgrade::no_metadata))]
    Metadata,
}

struct ChangesCtx {
    fs: vfs::FileOperationStore,
    will_disable_rustup_override: bool,
    description: Vec<String>,
}

impl ChangesCtx {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            fs: vfs::FileOperationStore::new(root),
            will_disable_rustup_override: false,
            description: vec![],
        }
    }

    pub async fn edit_toml(
        &mut self,
        path: impl AsRef<Path>,
        editor: impl FnOnce(&mut DocumentMut, &mut Self),
    ) -> Result<(), CliError> {
        let path = path.as_ref();
        let (mut doc, old_contents) = open_or_create_toml(&mut self.fs, path).await?;
        editor(&mut doc, self);

        let new_file = doc.to_string();
        if old_contents.as_ref() == Some(&new_file) {
            return Ok(()); // Avoid marking file as changed; hides diff.
        }

        self.fs.write(path, new_file).await?;

        Ok(())
    }

    pub fn describe(&mut self, change: impl Into<String>) {
        self.description.push(change.into());
    }

    pub async fn apply(&mut self) -> Result<(), CliError> {
        self.fs.apply().await?;

        if self.will_disable_rustup_override {
            let mut cmd = Command::new("rustup");

            cmd.arg("override")
                .arg("unset")
                .arg("--path")
                .arg(self.fs.root());

            let status = cmd.spawn()?.wait().await?;
            if !status.success() {
                log::warn!(
                    "Disabling the rustup override for the project directory was unsuccessful"
                );
            }
        }

        Ok(())
    }
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

async fn open_or_create_toml(
    files: &mut vfs::FileOperationStore,
    path: &Path,
) -> Result<(DocumentMut, Option<String>), CliError> {
    let file = files.read_to_string(&path).await;

    // If the config file is missing, make a new one.
    let doc = match file {
        Ok(contents) => {
            let toml = contents
                .parse::<DocumentMut>()
                .map_err(MigrateError::from)?;
            (toml, Some(contents))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => (DocumentMut::new(), None),
        Err(other) => return Err(other)?,
    };

    Ok(doc)
}

fn toml_item_eq_strings(toml: Option<&Item>, strings: &[&str]) -> bool {
    toml.and_then(|f| f.as_array())
        .map(|array| {
            array
                .into_iter()
                .map(|f| f.as_str().unwrap_or_default())
                .eq(strings.iter().cloned())
        })
        .unwrap_or_default()
}
