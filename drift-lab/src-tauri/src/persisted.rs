//! Persist the LLM backend selection to `app_local_data_dir/backend.json`.
//!
//! Threat model is dev-tool: API keys live in plaintext but the file is mode
//! 0600 on Unix. A future `KeychainSecretStore` can split secrets out without
//! touching commands or UI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager, Runtime};

use crate::model_config::ModelBackend;

const CONFIG_FILENAME: &str = "backend.json";

pub fn config_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_local_data_dir()
        .context("resolving app_local_data_dir")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {dir:?}"))?;
    Ok(dir.join(CONFIG_FILENAME))
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<Option<ModelBackend>> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {path:?}"))?;
    let config: ModelBackend =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {path:?}"))?;
    Ok(Some(config))
}

pub fn save<R: Runtime>(app: &AppHandle<R>, config: &ModelBackend) -> Result<()> {
    let path = config_path(app)?;
    let bytes = serde_json::to_vec_pretty(config).context("serialising backend config")?;
    write_locked(&path, &bytes)
}

pub fn clear<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let path = config_path(app)?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {path:?}"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_locked(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {path:?} for write"))?;
    std::io::Write::write_all(&mut file, bytes)
        .with_context(|| format!("writing {path:?}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_locked(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {path:?}"))?;
    Ok(())
}
