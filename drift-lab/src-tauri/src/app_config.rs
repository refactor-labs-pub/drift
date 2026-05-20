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
// Canonical definition lives in the realtime domain; we re-export it from
// this module so existing call sites (`commands.rs`, etc.) keep their
// `app_config::RealtimeConfig` import path unchanged through PR-1.
pub use crate::realtime::domain::RealtimeConfig;

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
    /// User preferences for static-scan filtering. `#[serde(default)]` so
    /// older config files (pre-scan-filters era) load with the sane defaults
    /// instead of erroring out, and the new filter takes effect even for
    /// users who never visit Settings after upgrade.
    #[serde(default)]
    pub scan_filters: ScanFilters,
    /// **Legacy** single-record realtime config (Phase B). Retained as the
    /// migration source: on first load after upgrading to PR-2a we copy
    /// non-empty values from here into `realtime_settings` as a single
    /// auto-named profile. New code reads/writes [`realtime_settings`]
    /// instead. Stays until a follow-up release confirms every user has
    /// migrated.
    #[serde(default)]
    pub realtime: RealtimeConfig,
    /// Multi-profile realtime settings (PR-2a+). One profile per
    /// project/service; one is "active" at a time. Empty by default;
    /// `AppConfigProfileRepository::load` performs a one-time migration
    /// from [`realtime`] when this field is empty and the legacy one
    /// has data.
    #[serde(default)]
    pub realtime_settings: crate::realtime::domain::RealtimeSettings,
}

/// User-toggleable scan-walker filters. Mirrors the relevant subset of
/// `drift_static_profiler::AnalyzeOptions` so the Settings UI doesn't have
/// to know the profiler's internal types — and so the profiler crate
/// stays free of `serde-with-defaults`-style backwards-compat baggage.
///
/// Add a field here when:
///   1. The profiler has a `WalkOpts` / `AnalyzeOptions` flag that
///      meaningfully changes which files surface in entry-point picking.
///   2. The flag has a sensible default for the 90% case and an
///      "I know what I'm doing" override case worth a UI toggle.
///
/// Don't add: dev-only knobs, experimental flags, anything the user can't
/// reason about without reading source code.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFilters {
    /// Skip directories named `static` / `assets` (see
    /// `drift_static_profiler::walker::STATIC_ASSET_DIRS`). Default true:
    /// these dirs nearly always hold vendored minified bundles that
    /// hijack the entry-point ranking with synthetic top callers.
    pub exclude_static_assets: bool,
    /// Drop test/spec/mock files at the walker stage. Default **true** —
    /// matches the CLI's `make scan-prompt` behavior. Test bundles
    /// (e.g. a 10MB `web/public/main.test.js` from a vite build) can
    /// otherwise flip the linguist breakdown to favor the wrong
    /// language and leave the picker with zero application roots.
    /// `#[serde(default = "default_true")]` so configs written before
    /// this field existed load with the safe (filtering-on) default
    /// instead of the bool-default `false`.
    #[serde(default = "default_true")]
    pub exclude_tests: bool,
}

impl Default for ScanFilters {
    fn default() -> Self {
        Self {
            exclude_static_assets: true,
            exclude_tests: true,
        }
    }
}

fn default_true() -> bool {
    true
}

// `RealtimeConfig` lives in `crate::realtime::domain` and is re-exported
// at the top of this module so persisting it stays a one-liner. Its
// `#[serde(default)]` + `Default` impl over there is what makes
// pre-realtime-era app-config blobs deserialize cleanly.

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
