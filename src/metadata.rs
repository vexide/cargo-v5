//! Workspace metadata schema & parsing
//!
//! This module deserializes metadata inside of Cargo.toml files for cargo-v5
//! to read when determining uploading parameters. This data is parsed from the
//! `cargo metadata` command's JSON output. Currently, we only bother with a subset
//! of the scheme containing the data we actually use (otherwise we'd just be
//! reinventing the `cargo-metadata` crate, which is massive).

use std::path::PathBuf;

use clap::ValueEnum;
use serde_json::Value;

use crate::{
    commands::upload::{ProgramIcon, UploadStrategy},
    errors::CliError,
};

fn field_type(field: &Value) -> &'static str {
    match field {
        Value::Array(_) => "array",
        Value::Bool(_) => "bool",
        Value::Null => "null",
        Value::Object(_) => "object",
        Value::String(_) => "string",
        Value::Number(_) => "number",
    }
}

/// Root output of `cargo metadata`. We only care about enumerating packages.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CargoMetadata {
    pub packages: Vec<Package>,
}

/// The stuff we actually care about from `cargo metadata`'s package entries.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Package {
    pub name: String,
    pub description: Option<String>,
    pub id: String,
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub metadata: Value,
}

/// The stuff specified in a Cargo.toml's `[package.metadata.v5]` fields.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct Metadata {
    pub slot: Option<u8>,
    pub icon: Option<ProgramIcon>,
    pub compress: Option<bool>,
    pub upload_strategy: Option<UploadStrategy>,
}

impl Metadata {
    pub fn new(pkg: &Package) -> Result<Self, CliError> {
        if let Some(metadata) = pkg.metadata.as_object()
            && let Some(v5_metadata) = metadata.get("v5").and_then(|m| m.as_object())
        {
            return Ok(Self {
                slot: if let Some(field) = v5_metadata.get("slot") {
                    let slot = field.as_u64().ok_or(CliError::BadFieldType {
                        field: "slot".to_string(),
                        expected: "string".to_string(),
                        found: field_type(field).to_string(),
                    })?;

                    Some(slot as u8) // NOTE: range validation is done at a later step
                } else {
                    None
                },
                icon: if let Some(field) = v5_metadata.get("icon") {
                    let icon = field.as_str().ok_or(CliError::BadFieldType {
                        field: "icon".to_string(),
                        expected: "string".to_string(),
                        found: field_type(field).to_string(),
                    })?;

                    Some(
                        ProgramIcon::from_str(icon, false)
                            .map_err(|_| CliError::InvalidIcon(icon.to_string()))?,
                    )
                } else {
                    None
                },
                compress: if let Some(compress) = v5_metadata.get("compress") {
                    let compress = compress.as_bool().ok_or(CliError::BadFieldType {
                        field: "compress".to_string(),
                        expected: "bool".to_string(),
                        found: field_type(compress).to_string(),
                    })?;

                    Some(compress)
                } else {
                    None
                },
                upload_strategy: if let Some(upload_strategy) = v5_metadata.get("upload-strategy") {
                    let strategy = upload_strategy.as_str().ok_or(CliError::BadFieldType {
                        field: "compress".to_string(),
                        expected: "bool".to_string(),
                        found: field_type(upload_strategy).to_string(),
                    })?;

                    Some(
                        UploadStrategy::from_str(strategy, false)
                            .map_err(|_| CliError::InvalidUploadStrategy(strategy.to_string()))?,
                    )
                } else {
                    None
                },
            });
        }

        Ok(Self::default())
    }
}
