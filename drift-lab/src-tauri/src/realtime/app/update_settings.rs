//! Use case: persist a new [`RealtimeConfig`].
//!
//! Trivial today (validate-and-save). Lives in its own use case because:
//! * PR-2 will add per-profile invariants (e.g. "URL is required when a
//!   profile is created", "channel name shape") that need a home that
//!   isn't the Tauri command.
//! * The Tauri command stays a 3-line shim that translates arg shapes
//!   and maps `RealtimeError → String`.

use crate::realtime::domain::{RealtimeConfig, RealtimeError};
use crate::realtime::ports::SettingsRepository;

pub struct UpdateSettingsUseCase<'a, R: SettingsRepository + ?Sized> {
    repo: &'a R,
}

impl<'a, R: SettingsRepository + ?Sized> UpdateSettingsUseCase<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    /// Validate and persist. Returns the saved value so the renderer can
    /// reconcile against any server-side normalisation. Today the
    /// "validation" is implicit (serde defaults handle missing fields);
    /// add real checks here as the schema grows.
    pub async fn execute(
        &self,
        settings: RealtimeConfig,
    ) -> Result<RealtimeConfig, RealtimeError> {
        self.repo.save(&settings).await?;
        Ok(settings)
    }
}
