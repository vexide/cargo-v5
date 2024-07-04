use anyhow::Context;
use cargo_metadata::Package;
use clap::ValueEnum;

use crate::commands::upload::ProgramIcon;

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct Metadata {
    pub slot: Option<u8>,
    pub icon: Option<ProgramIcon>,
    pub compress: Option<bool>,
}

impl Metadata {
    pub fn new(pkg: &Package) -> anyhow::Result<Self> {
        if let Some(metadata) = pkg.metadata.as_object() {
            if let Some(v5_metadata) = metadata.get("v5").and_then(|m| m.as_object()) {
                return Ok(Self {
                    slot: if let Some(slot) = &v5_metadata.get("slot") {
                        Some(
                            slot.as_u64()
                                .context("The provided slot must be in the range [1, 8].")?
                                as u8,
                        )
                    } else {
                        None
                    },
                    icon: if let Some(icon) = &v5_metadata.get("icon") {
                        Some(
                            ProgramIcon::from_str(icon.as_str().context("`icon` field should be a string.")?, false)
                                .expect("Invalid icon"),
                        )
                    } else {
                        None
                    },
                    compress: if let Some(compress) = &v5_metadata.get("compress") {
                        Some(
                            compress
                                .as_bool()
                                .context("`compress` field should be a boolean.")?,
                        )
                    } else {
                        None
                    },
                });
            }
        }
        
        Ok(Self::default())
    }
}
