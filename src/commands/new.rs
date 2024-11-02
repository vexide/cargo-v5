use cargo_metadata::camino::Utf8PathBuf;

use crate::errors::CliError;
use std::{io, path::{Path, PathBuf}};

#[cfg(feature = "fetch-template")]
async fn fetch_template() -> reqwest::Result<Vec<u8>> {
    println!("Fetching template...");
    let response =
        reqwest::get("https://github.com/vexide/vexide-template/archive/refs/heads/main.tar.gz")
            .await?;
    let bytes = response.bytes().await?;
    println!("Successfully fetched template.");
    Ok(bytes.to_vec())
}

fn baked_in_template() -> Vec<u8> {
    include_bytes!("./vexide-template.tar.gz").to_vec()
}

fn unpack_template(template: Vec<u8>, dir: &Utf8PathBuf) -> io::Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&template[..]));
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

pub async fn new(path: Utf8PathBuf, name: Option<String>) -> Result<(), CliError> {
    let dir = if let Some(name) = name {
        let dir = path.join(&name);
        if std::fs::metadata(&dir).is_ok() {
            return Err(CliError::ProjectExists(name))
        }
        std::fs::create_dir_all(&path).unwrap();
        dir
    } else {
        path
    };

    println!("Creating new project at {:?}", dir);

    #[cfg(feature = "fetch-template")]
    let template = match fetch_template().await {
        Ok(bytes) => bytes,
        Err(_) => {
            println!("Failed to fetch template, using baked-in template.");
            baked_in_template()
        },
    };
    #[cfg(not(feature = "fetch-template"))]
    let template = baked_in_template();

    println!("Unpacking template...");
    unpack_template(template, &dir)?;
    println!("Successfully created a new vexide project!");

    Ok(())
}
