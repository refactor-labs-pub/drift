//! Pluggable secret storage. Today the file-backed implementation persists to
//! a dedicated `secrets.json` via `tauri-plugin-store` — separate file from
//! `app-config.json` so a "dump my config" feature can never accidentally
//! leak secrets, and so the file mode can later be hardened independently.
//!
//! The trait is the seam: a future `KeychainSecretStore` (OS keyring on macOS
//! Keychain / Windows Credential Manager / `libsecret` on Linux) drops in
//! behind the same trait without touching any caller.
//!
//! ## Design rules
//! * **The renderer never reads values.** Tauri commands expose `set_secret`
//!   (write) and `secret_status` (presence-only). There is intentionally no
//!   `get_secret` command — if a UI XSS bug ever exists, the JWT isn't in
//!   reach because the value never crosses to JS in the first place.
//! * **Server-side reads only.** Rust background tasks (the Supabase
//!   subscriber, etc.) read values directly via `SecretStore::get` from a
//!   freshly-instantiated `FileSecretStore`.
//! * **Each command instantiates its own store.** `FileSecretStore` is
//!   cheap to construct (just wraps the `AppHandle`). Avoids plumbing
//!   `Arc<dyn SecretStore>` through `AppState`.

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

/// Dedicated tauri-plugin-store file. Separate from `app-config.json` so the
/// two files have independent failure / read / dump surfaces.
pub const STORE_FILE: &str = "secrets.json";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    #[allow(dead_code)] // Reserved for the eventual "clear key" Settings button.
    fn delete(&self, key: &str) -> Result<()>;
}

/// File-backed store via `tauri-plugin-store`. Persists to
/// `<AppData>/secrets.json`. The plugin's default file mode is user-only on
/// Unix; a future hardening pass can tighten to 0600 explicitly.
pub struct FileSecretStore<R: Runtime> {
    handle: AppHandle<R>,
}

impl<R: Runtime> FileSecretStore<R> {
    pub fn new(handle: AppHandle<R>) -> Self {
        Self { handle }
    }
}

impl<R: Runtime> SecretStore for FileSecretStore<R> {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let store = self
            .handle
            .store(STORE_FILE)
            .context("opening secrets store")?;
        Ok(store
            .get(key)
            .and_then(|v| v.as_str().map(String::from)))
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        let store = self
            .handle
            .store(STORE_FILE)
            .context("opening secrets store")?;
        store.set(key, serde_json::Value::String(value.to_string()));
        store.save().context("flushing secrets store")?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let store = self
            .handle
            .store(STORE_FILE)
            .context("opening secrets store")?;
        store.delete(key);
        store.save().context("flushing secrets store")?;
        Ok(())
    }
}
