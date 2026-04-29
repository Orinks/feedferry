use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;

pub fn data_dir(override_dir: Option<&Path>) -> Result<PathBuf> {
    let dir = if let Some(path) = override_dir {
        path.to_path_buf()
    } else if let Ok(value) = std::env::var("FEEDFERRY_DATA_DIR") {
        PathBuf::from(value)
    } else {
        ProjectDirs::from("dev", "FeedFerry", "FeedFerry")
            .ok_or_else(|| anyhow!("could not determine a platform data directory"))?
            .data_dir()
            .to_path_buf()
    };

    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create data directory {}", dir.display()))?;
    Ok(dir)
}

pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("feedferry.sqlite3")
}

pub fn default_download_dir(data_dir: &Path) -> Result<PathBuf> {
    let dir = data_dir.join("downloads");
    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create download directory {}", dir.display()))?;
    Ok(dir)
}
