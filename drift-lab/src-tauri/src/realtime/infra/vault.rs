//! [`ApiKeyVault`] backed by the existing [`FileSecretStore`].
//!
//! ## Key naming
//!
//! Two key shapes live in the SecretStore during the migration window:
//!
//! * **Legacy** (`supabase_realtime_api_key`) — written by old code
//!   that only knew about ONE realtime config. Read by
//!   [`read_legacy_realtime_api_key`] during the one-time migration
//!   in [`super::profile_repository::AppConfigProfileRepository::load`].
//!   Cleared by [`delete_legacy_realtime_api_key`] once migration
//!   succeeds. After the transition window we can remove these.
//!
//! * **Namespaced** (`supabase_realtime_api_key:<profile_id>`) — the
//!   one new code writes / reads. One secret per profile. Built by
//!   [`namespaced_realtime_api_key_for`].
//!
//! ## Per-profile binding
//!
//! [`FileApiKeyVault`] is parameterised by a [`ProfileId`] at
//! construction time. Use cases hold a vault and call `read()`; they
//! never see the profile id directly. This lets the
//! [`crate::realtime::ports::ApiKeyVault`] trait stay keyless while
//! still scaling to N profiles.

use tauri::{AppHandle, Runtime};

use crate::realtime::domain::{ProfileId, RealtimeError};
use crate::realtime::ports::ApiKeyVault;
use crate::secret_store::{FileSecretStore, SecretStore};

/// Legacy SecretStore key (single-record realtime config). New writes
/// go to the namespaced shape; the legacy slot is read once during
/// migration and then cleared.
pub const LEGACY_REALTIME_API_KEY: &str = "supabase_realtime_api_key";

/// Backwards-compatibility alias: the codebase still imports
/// `REALTIME_API_KEY` from this module. Keep it pointing at the legacy
/// constant for now; once everything moves to per-profile vaults this
/// can be deleted.
#[deprecated(note = "use namespaced_realtime_api_key_for(profile_id) instead")]
pub const REALTIME_API_KEY: &str = LEGACY_REALTIME_API_KEY;

/// Build the SecretStore key for a given profile. Stable string ID so
/// `secret_status(...)` from the renderer can presence-check a specific
/// profile without exposing the SecretStore layout to JS.
pub fn namespaced_realtime_api_key_for(id: &ProfileId) -> String {
    format!("{LEGACY_REALTIME_API_KEY}:{}", id.as_str())
}

/// Read the legacy single-record JWT, if any. Used by the migration
/// path; not used at runtime by any production code path.
pub fn read_legacy_realtime_api_key<R: Runtime>(
    handle: &AppHandle<R>,
) -> anyhow::Result<Option<String>> {
    let store = FileSecretStore::new(handle.clone());
    store.get(LEGACY_REALTIME_API_KEY)
}

/// Clear the legacy single-record JWT. Called once at migration end
/// so the SecretStore doesn't keep a duplicate around.
pub fn delete_legacy_realtime_api_key<R: Runtime>(
    handle: &AppHandle<R>,
) -> anyhow::Result<()> {
    let store = FileSecretStore::new(handle.clone());
    store.delete(LEGACY_REALTIME_API_KEY)
}

/// Vault bound to ONE profile at construction. Use cases hold this and
/// don't know which profile they're reading for — the binding is set
/// where the adapter is wired (in the Tauri command shim).
pub struct FileApiKeyVault<R: Runtime> {
    handle: AppHandle<R>,
    /// Which secret-store key this vault reads from. `None` means
    /// "legacy mode" (read the un-namespaced key) — kept ONLY for the
    /// pre-migration tests and the transition-period code path. PR-2b
    /// removes it.
    key_name: String,
}

impl<R: Runtime> FileApiKeyVault<R> {
    /// **Deprecated** legacy constructor — reads the un-namespaced
    /// `supabase_realtime_api_key`. Used by code that hasn't been
    /// switched to multi-profile yet. New code should use
    /// [`Self::for_profile`].
    #[deprecated(note = "use FileApiKeyVault::for_profile(handle, id) instead")]
    pub fn new(handle: AppHandle<R>) -> Self {
        Self {
            handle,
            key_name: LEGACY_REALTIME_API_KEY.to_string(),
        }
    }

    /// Vault that reads from the namespaced key for `profile_id`.
    pub fn for_profile(handle: AppHandle<R>, profile_id: &ProfileId) -> Self {
        Self {
            handle,
            key_name: namespaced_realtime_api_key_for(profile_id),
        }
    }
}

impl<R: Runtime> ApiKeyVault for FileApiKeyVault<R> {
    fn read(&self) -> Result<String, RealtimeError> {
        let store = FileSecretStore::new(self.handle.clone());
        let value = store
            .get(&self.key_name)
            .map_err(|e| RealtimeError::Io(format!("read secret: {e}")))?;
        match value {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(RealtimeError::MissingApiKey),
        }
    }
}
