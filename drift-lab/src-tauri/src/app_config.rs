//! Multi-provider app config. One JSON record in tauri-plugin-store under
//! `app-config.json` → key `config`.
//!
//! Replaces the older single-config `persisted.rs`. The two coexist for one
//! release while the frontend migrates; eventually `persisted.rs` is dropped.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use uuid::Uuid;

use crate::model_config::ModelBackend;

pub const STORE_FILE: &str = "app-config.json";
const CONFIG_KEY: &str = "config";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedProvider {
    pub id: String,
    /// Human label, e.g. "My OpenAI" or "Local Llama 3.2".
    pub name: String,
    pub config: ModelBackend,
    pub created_at: u64,
}

impl SavedProvider {
    pub fn new(name: String, config: ModelBackend) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            config,
            created_at: now_secs(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub onboarding_complete: bool,
    pub active_provider_id: Option<String>,
    pub providers: Vec<SavedProvider>,
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<AppConfig> {
    let store = app.store(STORE_FILE).context("opening app-config store")?;
    Ok(store
        .get(CONFIG_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

pub fn save<R: Runtime>(app: &AppHandle<R>, cfg: &AppConfig) -> Result<()> {
    let store = app.store(STORE_FILE).context("opening app-config store")?;
    store.set(
        CONFIG_KEY,
        serde_json::to_value(cfg).context("serialising app config")?,
    );
    store.save().context("flushing app-config store")?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
