use cargo_metadata::camino::Utf8PathBuf;
#[cfg(feature = "fetch-template")]
use directories::ProjectDirs;
use log::{debug, info, warn};
use serde_json::Value;

use crate::errors::CliError;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
struct Template {
    pub data: Vec<u8>,
    pub sha: Option<String>,
}

#[cfg(feature = "fetch-template")]
async fn get_current_sha() -> Result<String, CliError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://api.github.com/repos/vexide/vexide-template/commits/main?per-page=1")
        .header("User-Agent", "vexide/cargo-v5")
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(err) => return Err(CliError::ReqwestError(err)),
    };
    let response_text = response.text().await.ok().unwrap_or("{}".to_string());
    match &serde_json::from_str::<Value>(&response_text).unwrap_or_default()["sha"] {
        Value::String(str) => Ok(str.clone()),
        _ => unreachable!("Internal error: GitHub API broken"),
    }
}

#[cfg(feature = "fetch-template")]
async fn fetch_template() -> Result<Template, CliError> {
    debug!("Fetching template...");
    let response =
        reqwest::get("https://github.com/vexide/vexide-template/archive/refs/heads/main.tar.gz")
            .await;
    let response = match response {
        Ok(response) => response,
        Err(err) => return Err(CliError::ReqwestError(err)),
    };
    let bytes = response.bytes().await?;

    debug!("Successfully fetched template.");
    let template = Template {
        data: bytes.to_vec(),
        sha: get_current_sha().await.ok(),
    };
    store_cached_template(template.clone());
    Ok(template)
}

#[cfg(feature = "fetch-template")]
fn get_cached_template() -> Option<Template> {
    let sha = cached_template_dir()
        .and_then(|path| fs::read_to_string(path.with_file_name("cache-id.txt")).ok());
    cached_template_dir()
        .map(|path| path.with_file_name("vexide-template.tar.gz"))
        .and_then(|cache_file| fs::read(cache_file).ok())
        .map(|data: Vec<u8>| Template { data, sha })
        .inspect(|template| {log::debug!("Found cached template with sha: {:?}", template.sha)})
}

#[cfg(feature = "fetch-template")]
fn store_cached_template(template: Template) -> () {
    cached_template_dir()
        .map(|path| path.with_file_name("vexide-template.tar.gz"))
        .map(|cache_file| fs::write(cache_file, &template.data));
    cached_template_dir()
        .map(|path| path.with_file_name("cache-id.txt"))
        .map(|sha_file| {
            if let Some(sha) = template.sha {
                fs::write(sha_file, sha);
            }
        });
    
    }

#[cfg(feature = "fetch-template")]
fn cached_template_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "vexide", "cargo-v5")
        .and_then(|dirs| dirs.cache_dir().canonicalize().ok())
}

fn baked_in_template() -> Template {
    Template {
        data: include_bytes!("./vexide-template.tar.gz").to_vec(),
        sha: None,
    }
}

fn unpack_template(template: Vec<u8>, dir: &Utf8PathBuf) -> io::Result<()> {
    let mut archive: tar::Archive<flate2::read::GzDecoder<&[u8]>> =
        tar::Archive::new(flate2::read::GzDecoder::new(&template[..]));
    for entry in archive.entries()? {
        let mut entry = entry?;

        let path = entry.path()?;
        let stripped_path = path.iter().skip(1).collect::<PathBuf>();

        if let Some(stripped_path) = stripped_path.to_str() {
            let output_path = Path::new(dir).join(stripped_path);

            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            entry.unpack(output_path)?;
        }
    }
    Ok(())
}

pub async fn new(
    path: Utf8PathBuf,
    name: Option<String>,
    download_template: bool,
) -> Result<(), CliError> {
    let dir = if let Some(name) = &name {
        let dir = path.join(name);
        std::fs::create_dir_all(&path).unwrap();
        dir
    } else {
        path
    };

    if std::fs::read_dir(&dir).is_ok_and(|e| e.count() > 0) {
        return Err(CliError::ProjectDirFull(dir.into_string()));
    }

    let name = name.unwrap_or_else(|| dir.file_name().unwrap().to_string());
    info!("Creating new project at {:?}", dir);

    #[cfg(feature = "fetch-template")]
    let template = get_cached_template();

    #[cfg(feature = "fetch-template")]
    let template = match (
        template.clone().and_then(|t| t.sha),
        get_current_sha().await,
    ) {
        _ if !download_template => template,
        (Some(cached_sha), Ok(current_sha)) if cached_sha == current_sha => {
            debug!("Cached template is current, skipping download.");
            template
        }
        _ => {
            debug!("Cached template is out of date.");
            let fetched_template = fetch_template().await.ok();
            fetched_template.or_else(|| {
                warn!("Could not fetch template, falling back to cache.");
                template
            })
        }
    };

    #[cfg(feature = "fetch-template")]
    let template = template.unwrap_or_else(|| {
        debug!("No template found in cache, using builtin template.");
        baked_in_template()
    });

    #[cfg(not(feature = "fetch-template"))]
    let template = baked_in_template();

    debug!("Unpacking template...");
    unpack_template(template.data, &dir)?;
    debug!("Successfully unpacked vexide-template!");

    debug!("Renaming project to {}...", &name);
    let manifest_path = dir.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)?;
    let manifest = manifest.replace("vexide-template", &name);
    std::fs::write(manifest_path, manifest)?;

    info!("Successfully created new project at {:?}", dir);
    Ok(())
}
