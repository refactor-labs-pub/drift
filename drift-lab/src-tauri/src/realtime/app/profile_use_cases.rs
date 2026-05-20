//! Profile-management use cases: save / delete / activate / list.
//!
//! Each is one struct, one `execute`, holds only ports. Tauri commands
//! instantiate per invocation; no shared state.
//!
//! ## Why one file (not one per verb)
//! Each use case is 10-30 lines. Splitting four nearly-identical CRUD
//! shells across four files adds navigation friction without isolating
//! anything that benefits from isolation. The test-connection and
//! start-stream use cases get their own files because they're
//! conceptually distinct operations with their own state.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::realtime::domain::{ProfileId, RealtimeError, RealtimeProfile, RealtimeSettings};
use crate::realtime::ports::ProfileRepository;

// ---------------------------------------------------------------------------
// SaveProfileUseCase
// ---------------------------------------------------------------------------

/// Persist a profile (insert-or-update). The repo handles `updated_at`;
/// this use case sets `created_at` on first save and validates the
/// caller-supplied fields. The API key is NOT a field on the profile —
/// it's written separately via a SecretStore write at the command layer
/// (see `commands::save_realtime_profile`).
pub struct SaveProfileUseCase<'a, R: ProfileRepository + ?Sized> {
    repo: &'a R,
}

#[derive(Debug, Clone, Default)]
pub struct SaveProfileInput {
    /// `None` → new profile (use case generates a fresh id).
    /// `Some(id)` → update an existing one.
    pub id: Option<ProfileId>,
    pub name: String,
    pub url: String,
    pub channel: String,
    pub event_name: String,
    pub frame_filter: String,
}

impl<'a, R: ProfileRepository + ?Sized> SaveProfileUseCase<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    pub async fn execute(&self, input: SaveProfileInput) -> Result<RealtimeProfile, RealtimeError> {
        // ----- validate -----
        let name = input.name.trim();
        if name.is_empty() {
            return Err(RealtimeError::Io("profile name is required".into()));
        }
        let url = input.url.trim();
        if url.is_empty() {
            return Err(RealtimeError::InvalidUrl(input.url.clone()));
        }

        let now = now_secs();
        let profile = match input.id {
            Some(existing_id) => {
                // Update: pull the existing record so we keep its
                // `created_at`. If the id is unknown, treat as a new
                // profile carrying the caller-supplied id (lets the
                // command layer round-trip an "edit then save" without
                // a second load).
                let settings = self.repo.load().await?;
                let created_at = settings
                    .find(&existing_id)
                    .map(|p| p.created_at)
                    .unwrap_or(now);
                RealtimeProfile {
                    id: existing_id,
                    name: name.to_string(),
                    url: url.to_string(),
                    channel: input.channel.trim().to_string(),
                    event_name: input.event_name.trim().to_string(),
                    frame_filter: input.frame_filter.trim().to_string(),
                    created_at,
                    updated_at: now,
                }
            }
            None => RealtimeProfile {
                id: ProfileId::new(),
                name: name.to_string(),
                url: url.to_string(),
                channel: input.channel.trim().to_string(),
                event_name: input.event_name.trim().to_string(),
                frame_filter: input.frame_filter.trim().to_string(),
                created_at: now,
                updated_at: now,
            },
        };
        self.repo.save(profile).await
    }
}

// ---------------------------------------------------------------------------
// DeleteProfileUseCase
// ---------------------------------------------------------------------------

pub struct DeleteProfileUseCase<'a, R: ProfileRepository + ?Sized> {
    repo: &'a R,
}

impl<'a, R: ProfileRepository + ?Sized> DeleteProfileUseCase<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    /// Delete the profile by id. Idempotent — returns `Ok(false)` if no
    /// profile with that id existed. The repo also cascades to clear
    /// the namespaced secret and (if the deleted profile was active)
    /// clears the active pointer.
    pub async fn execute(&self, id: &ProfileId) -> Result<bool, RealtimeError> {
        self.repo.delete(id).await
    }
}

// ---------------------------------------------------------------------------
// ActivateProfileUseCase
// ---------------------------------------------------------------------------

pub struct ActivateProfileUseCase<'a, R: ProfileRepository + ?Sized> {
    repo: &'a R,
}

impl<'a, R: ProfileRepository + ?Sized> ActivateProfileUseCase<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    /// Set the active profile. `None` clears the active pointer (no
    /// profile in use — LiveScan will refuse to start a stream).
    pub async fn execute(&self, id: Option<ProfileId>) -> Result<(), RealtimeError> {
        self.repo.set_active(id).await
    }
}

// ---------------------------------------------------------------------------
// ListProfilesUseCase
// ---------------------------------------------------------------------------

pub struct ListProfilesUseCase<'a, R: ProfileRepository + ?Sized> {
    repo: &'a R,
}

impl<'a, R: ProfileRepository + ?Sized> ListProfilesUseCase<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self { repo }
    }

    /// Return the current settings (profiles + active id). The renderer
    /// is the one that decorates each profile with a "key configured"
    /// presence flag (via the separate `secret_status` Tauri command)
    /// — this use case stays vault-free so it can be tested without
    /// SecretStore plumbing.
    pub async fn execute(&self) -> Result<RealtimeSettings, RealtimeError> {
        self.repo.load().await
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
