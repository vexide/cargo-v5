use anyhow::{bail, Context};
use directories::ProjectDirs;
use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
}

impl Config {
    pub fn path() -> anyhow::Result<std::path::PathBuf> {
        if let Some(proj_dirs) = ProjectDirs::from("dev", "vexide", "cargo-pros") {
            Ok(proj_dirs.preference_dir().join("config.toml"))
        } else {
            bail!("Could not find user home directory")
        }
    }

    pub fn load() -> anyhow::Result<Self> {
        let Ok(config_path) = Self::path() else {
            return Ok(Config::default());
        };

        if config_path.exists() {
            let config =
                fs_err::read_to_string(&config_path).context("Reading cargo-pros config file")?;
            Ok(toml::from_str(&config)
                .with_context(|| format!("Parsing config file at {:?}", config_path))?)
        } else {
            Ok(Config::default())
        }
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(default)]
pub struct Defaults {
    pub slot: Option<u8>,
}
