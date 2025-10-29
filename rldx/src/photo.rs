use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::BaseDirs;

const CACHE_SUBDIR: &str = "rldx/img";

pub fn cache_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().context("unable to determine cache directory")?;
    let dir = base.cache_dir().join(CACHE_SUBDIR);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn cached_image_path(uuid: &str) -> Result<PathBuf> {
    Ok(cache_dir()?.join(format!("{uuid}.png")))
}

pub fn load_cached_image(uuid: &str) -> Result<Option<Vec<u8>>> {
    let path = cached_image_path(uuid)?;
    if path.exists() {
        let data = fs::read(&path)?;
        Ok(Some(data))
    } else {
        Ok(None)
    }
}

pub fn save_cached_image(uuid: &str, data: &[u8]) -> Result<PathBuf> {
    let path = cached_image_path(uuid)?;
    fs::write(&path, data)?;
    Ok(path)
}