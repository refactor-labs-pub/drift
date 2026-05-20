//! [`ProfileRepository`] backed by the shared `AppConfig` store, with a
//! one-time migration from the legacy single-record `RealtimeConfig`.
//!
//! ## Migration semantics
//!
//! On every `load()`:
//!   * If `AppConfig.realtime_settings.profiles` is non-empty → already
//!     migrated. Return as-is.
//!   * Else if `AppConfig.realtime.url` is empty → fresh user, nothing
//!     to migrate. Return empty settings.
//!   * Else (legacy data present, new field empty) → build ONE
//!     profile named `"default"` from the legacy values, set it
//!     active, persist, AND migrate the SecretStore key from
//!     `supabase_realtime_api_key` → `supabase_realtime_api_key:<id>`.
//!
//! Idempotent: after the first successful migration,
//! `realtime_settings.profiles` is non-empty so the second branch
//! returns immediately. The legacy `realtime` field is left untouched
//! so a downgrade-then-rerun still works during the transition window.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;

use crate::app_config::{self, AppConfig};
use crate::realtime::domain::{
    ProfileId, RealtimeError, RealtimeProfile, RealtimeSettings,
};
use crate::realtime::infra::vault::{
    delete_legacy_realtime_api_key, namespaced_realtime_api_key_for,
    read_legacy_realtime_api_key, LEGACY_REALTIME_API_KEY,
};
use crate::realtime::ports::ProfileRepository;
use crate::secret_store::{FileSecretStore, SecretStore};

pub struct AppConfigProfileRepository<R: Runtime> {
    handle: AppHandle<R>,
    config: Arc<Mutex<AppConfig>>,
}

impl<R: Runtime> AppConfigProfileRepository<R> {
    pub fn new(handle: AppHandle<R>, config: Arc<Mutex<AppConfig>>) -> Self {
        Self { handle, config }
    }

    /// One-time migration from the legacy `realtime: RealtimeConfig`
    /// to `realtime_settings: RealtimeSettings`. Called from `load()`
    /// while holding the config lock so two concurrent loads can't
    /// race (the second sees the migrated state).
    ///
    /// Returns the (possibly mutated) settings + a `migrated` flag so
    /// the caller can persist if needed.
    fn maybe_migrate(cfg: &mut AppConfig, handle: &AppHandle<R>) -> bool {
        if !cfg.realtime_settings.profiles.is_empty() {
            return false; // already migrated
        }
        let legacy_url = cfg.realtime.url.trim();
        if legacy_url.is_empty() {
            return false; // fresh install, nothing to do
        }

        // Build a single profile carrying the legacy values verbatim.
        // The legacy field names are `default_channel` / `default_event` /
        // `default_frame_filter`; in the new profile they drop the
        // "default_" prefix because each profile owns its own routing,
        // it's no longer a "default" for a multitude of unnamed scans.
        let now = now_secs();
        let mut profile = RealtimeProfile::new("default", legacy_url, now);
        if !cfg.realtime.default_channel.trim().is_empty() {
            profile.channel = cfg.realtime.default_channel.trim().to_string();
        }
        if !cfg.realtime.default_event.trim().is_empty() {
            profile.event_name = cfg.realtime.default_event.trim().to_string();
        }
        if !cfg.realtime.default_frame_filter.trim().is_empty() {
            profile.frame_filter = cfg.realtime.default_frame_filter.trim().to_string();
        }
        let profile_id = profile.id.clone();
        cfg.realtime_settings.profiles.push(profile);
        cfg.realtime_settings.active_profile_id = Some(profile_id.clone());

        // Migrate the secret: namespaced key gets the legacy value;
        // legacy key gets cleared. Best-effort — if the legacy key
        // doesn't exist (user never saved a JWT), no migration needed.
        // If clearing the legacy key fails, log + continue: worst case
        // the user has a phantom legacy entry that no use case reads.
        if let Ok(Some(legacy_value)) = read_legacy_realtime_api_key(handle) {
            if !legacy_value.is_empty() {
                let store = FileSecretStore::new(handle.clone());
                let namespaced = namespaced_realtime_api_key_for(&profile_id);
                if let Err(e) = store.set(&namespaced, &legacy_value) {
                    tracing::warn!(
                        "realtime migration: failed to write namespaced secret key: {e}"
                    );
                } else if let Err(e) = delete_legacy_realtime_api_key(handle) {
                    tracing::warn!(
                        "realtime migration: failed to clear legacy secret key '{LEGACY_REALTIME_API_KEY}': {e}"
                    );
                }
                let _ = namespaced; // suppress unused-binding when no log fires
            }
        }

        true
    }
}

#[async_trait]
impl<R: Runtime> ProfileRepository for AppConfigProfileRepository<R> {
    async fn load(&self) -> Result<RealtimeSettings, RealtimeError> {
        let mut cfg = self.config.lock().await;
        let migrated = Self::maybe_migrate(&mut cfg, &self.handle);
        if migrated {
            app_config::save(&self.handle, &cfg)
                .map_err(|e| RealtimeError::Io(format!("persist migrated config: {e}")))?;
        }
        Ok(cfg.realtime_settings.clone())
    }

    async fn save(
        &self,
        mut profile: RealtimeProfile,
    ) -> Result<RealtimeProfile, RealtimeError> {
        // Set `updated_at` here so callers (and use cases) don't need
        // to mind timestamps. `created_at` is only set if this is a
        // first save (the slot still carries `RealtimeProfile::new`'s
        // value when the use case constructed it).
        let now = now_secs();
        profile.updated_at = now;

        let mut cfg = self.config.lock().await;
        match cfg.realtime_settings.find_mut(&profile.id) {
            Some(existing) => {
                // Update in place but DON'T clobber `created_at` — that
                // belongs to the first save, not this one.
                let created_at = existing.created_at;
                *existing = profile.clone();
                existing.created_at = created_at;
            }
            None => {
                // New profile — leave `created_at` as the caller set it
                // (typically also `now`).
                cfg.realtime_settings.profiles.push(profile.clone());
            }
        }
        app_config::save(&self.handle, &cfg)
            .map_err(|e| RealtimeError::Io(format!("persist profile save: {e}")))?;
        Ok(profile)
    }

    async fn delete(&self, id: &ProfileId) -> Result<bool, RealtimeError> {
        let mut cfg = self.config.lock().await;
        let before = cfg.realtime_settings.profiles.len();
        cfg.realtime_settings.profiles.retain(|p| &p.id != id);
        let removed = cfg.realtime_settings.profiles.len() < before;
        if !removed {
            return Ok(false);
        }
        // If the deleted profile was active, clear the pointer. No
        // auto-promotion — the user picks the next active profile
        // explicitly.
        if cfg.realtime_settings.active_profile_id.as_ref() == Some(id) {
            cfg.realtime_settings.active_profile_id = None;
        }
        app_config::save(&self.handle, &cfg)
            .map_err(|e| RealtimeError::Io(format!("persist profile delete: {e}")))?;

        // Also wipe the namespaced secret. Best-effort — a leftover
        // entry in SecretStore for a deleted profile is harmless (no
        // use case will read it) but tidiness matters.
        let store = FileSecretStore::new(self.handle.clone());
        if let Err(e) = store.delete(&namespaced_realtime_api_key_for(id)) {
            tracing::warn!("failed to clear secret for deleted profile {id}: {e}");
        }
        Ok(true)
    }

    async fn set_active(&self, id: Option<ProfileId>) -> Result<(), RealtimeError> {
        let mut cfg = self.config.lock().await;
        if let Some(ref new_active) = id {
            if cfg.realtime_settings.find(new_active).is_none() {
                return Err(RealtimeError::Io(format!(
                    "set_active: profile id {new_active} does not exist"
                )));
            }
        }
        cfg.realtime_settings.active_profile_id = id;
        app_config::save(&self.handle, &cfg)
            .map_err(|e| RealtimeError::Io(format!("persist active profile: {e}")))?;
        Ok(())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
