use std::{fmt::{Display, Formatter}, str::FromStr};

use arm_toolchain::toolchain::ToolchainVersion;
use cargo_metadata::{Metadata, Package, PackageId};
use clap::ValueEnum;
use serde_json::Value;
use thiserror::Error;
use tokio::task::{spawn_blocking};

use crate::{
    commands::upload::{ProgramIcon, UploadStrategy},
    errors::CliError,
};

pub async fn workspace_metadata() -> Option<Metadata> {
    spawn_blocking(|| cargo_metadata::MetadataCommand::new().no_deps().exec())
        .await
        .unwrap()
        .ok()
}

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

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub struct Settings {
    pub slot: Option<u8>,
    pub icon: Option<ProgramIcon>,
    pub compress: Option<bool>,
    pub upload_strategy: Option<UploadStrategy>,
    pub toolchain: Option<ToolchainCfg>,
}

impl Settings {
    pub fn for_root(m: Option<&Metadata>) -> Result<Option<Self>, CliError> {
        m.and_then(|m| m.root_package()).map(Self::from_pkg).transpose()
    }

    pub fn from_pkg(pkg: &Package) -> Result<Self, CliError> {
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
                toolchain: if let Some(toolchain) = v5_metadata.get("toolchain") {
                    let str = toolchain.as_str().ok_or(CliError::BadFieldType {
                        field: "toolchain".to_string(),
                        expected: "table".to_string(),
                        found: field_type(toolchain).to_string(),
                    })?;

                    Some(ToolchainCfg::from_str(str)?)
                } else {
                    None
                },
            });
        }

        Ok(Self::default())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolchainCfg {
    pub ty: ToolchainType,
    pub version: ToolchainVersion,
}

impl FromStr for ToolchainCfg {
    type Err = BadFieldDataError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((left, right)) = s.split_once('-') else {
            return Err(BadFieldDataError::ToolchainMissingDash);
        };

        let ty = ToolchainType::from_str(left)?;
        let version = ToolchainVersion::from(right);

        Ok(Self { ty, version })
    }
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub enum ToolchainType {
    #[default]
    LLVM,
}

impl FromStr for ToolchainType {
    type Err = BadFieldDataError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.to_lowercase();
        match &*s {
            "llvm" => Ok(Self::LLVM),
            _ => Err(BadFieldDataError::ToolchainTypeUnsupported { request: lower }),
        }
    }
}

impl Display for ToolchainType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LLVM => write!(f, "llvm"),
        }
    }
}

#[derive(Debug, Error)]
pub enum BadFieldDataError {
    #[error("The `toolchain` type {request:?} is not supported [allowed values: llvm]")]
    ToolchainTypeUnsupported { request: String },
    #[error("`toolchain`s must have a type and version separated by a dash (e.g. llvm-21.1.1)")]
    ToolchainMissingDash,
}
