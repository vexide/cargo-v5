#[cfg(feature = "clap")]
use std::ffi::OsStr;
use std::path::PathBuf;

#[cfg(feature = "clap")]
use clap_complete::engine::{CompletionCandidate, ValueCompleter};
use directories::ProjectDirs;

const CACHE_TTL_SECS: u64 = 600; // 10 minutes

fn get_ls_cache_path() -> Option<PathBuf> {
    ProjectDirs::from("", "vexide", "cargo-v5")
        .map(|dirs| dirs.cache_dir().to_owned().join("file-cache.json"))
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct FileCache {
    timestamp: u64,
    files: Vec<String>,
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> Option<Vec<String>> {
    let content = std::fs::read_to_string(get_ls_cache_path().unwrap()).ok()?;
    let cache: FileCache = serde_json::from_str(&content).ok()?;

    if current_timestamp() - cache.timestamp < CACHE_TTL_SECS {
        Some(cache.files)
    } else {
        None
    }
}

/// Writes the file list cache. Called by `cargo v5 ls`.
pub fn write_cache(files: &[String]) {
    let cache = FileCache {
        timestamp: current_timestamp(),
        files: files.to_vec(),
    };
    if let Ok(content) = serde_json::to_string(&cache) {
        let path = get_ls_cache_path().unwrap();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _ = std::fs::write(path, content);
    }
}

#[cfg(feature = "clap")]
pub struct FileCompleter;

#[cfg(feature = "clap")]
impl ValueCompleter for FileCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        let Some(files) = read_cache() else {
            return Vec::new();
        };

        let current_str = current.to_string_lossy();

        files
            .into_iter()
            .filter(|file| file.starts_with(&*current_str))
            .map(CompletionCandidate::new)
            .collect()
    }
}
