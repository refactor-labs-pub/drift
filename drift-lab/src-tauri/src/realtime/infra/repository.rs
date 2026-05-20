//! [`SettingsRepository`] backed by the existing `AppConfig` store.
//!
//! Owns no state of its own — it reaches into the shared
//! `AppState.app_config` mutex (via `Arc<Mutex<AppConfig>>`) and writes
//! through to disk via [`crate::app_config::save`]. Kept thin on purpose:
//! a future SQLite-backed repo replaces only this adapter, nothing else.

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;

use crate::app_config::{self, AppConfig};
use crate::realtime::domain::{RealtimeConfig, RealtimeError};
use crate::realtime::ports::SettingsRepository;

/// Adapter that loads/saves the realtime slice of the shared `AppConfig`.
/// One instance per Tauri command invocation is fine — construction is
/// cheap (just clones a couple of `Arc`s).
pub struct AppConfigSettingsRepository<R: Runtime> {
    handle: AppHandle<R>,
    config: Arc<Mutex<AppConfig>>,
}

impl<R: Runtime> AppConfigSettingsRepository<R> {
    pub fn new(handle: AppHandle<R>, config: Arc<Mutex<AppConfig>>) -> Self {
        Self { handle, config }
    }
}

#[async_trait]
impl<R: Runtime> SettingsRepository for AppConfigSettingsRepository<R> {
    async fn load(&self) -> Result<RealtimeConfig, RealtimeError> {
        Ok(self.config.lock().await.realtime.clone())
    }

    async fn save(&self, settings: &RealtimeConfig) -> Result<(), RealtimeError> {
        let mut cfg = self.config.lock().await;
        cfg.realtime = settings.clone();
        // Persist while still holding the in-memory lock so a concurrent
        // reader can't observe a half-applied state (in-memory updated,
        // disk not yet).
        app_config::save(&self.handle, &cfg)
            .map_err(|e| RealtimeError::Io(format!("persist app config: {e}")))?;
        Ok(())
    }
}
