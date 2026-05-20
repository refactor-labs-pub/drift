//! `RealtimeProfile` — the per-project unit of realtime configuration.
//!
//! Each profile owns one transport pair (URL + secret-key reference) and
//! one routing triple (channel + event name + frame filter). A user can
//! have many profiles; one is "active" at a time and is what the
//! LiveScan page wires up by default.
//!
//! The API key is intentionally *not* a field on this struct. The
//! [`crate::realtime::ports::ApiKeyVault`] holds it under the namespaced
//! SecretStore key `supabase_realtime_api_key:<profile_id>`. Keeping
//! secrets out of the persisted struct guarantees that a future "dump
//! my config" feature can never leak a JWT.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::defaults::{DEFAULT_CHANNEL, DEFAULT_EVENT_NAME};
use super::settings::RealtimeConfig;

/// Newtype around a UUID-v4 string so the rest of the codebase can't
/// confuse a `ProfileId` with any other UUID (live-scan id, stream id,
/// test id, …). String-shaped on the wire to keep the JS bindings
/// boring.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProfileId(pub String);

impl ProfileId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_str(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One saved realtime configuration. Field names use the *current*
/// concept names (no more `default_*` prefixes) — a profile's `channel`
/// IS its channel; per-scan overrides still exist but live transient on
/// [`crate::realtime::domain::StreamOverrides`], not here.
///
/// `created_at` / `updated_at` are epoch seconds so the renderer can
/// sort the list deterministically without parsing dates. Setting them
/// at save time lives in the use case ([`crate::realtime::app::SaveProfileUseCase`]),
/// not in `Default` — `Default` is for tests / fresh-form pre-fill.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeProfile {
    pub id: ProfileId,
    /// Human label, e.g. `"checkout-service prod"`. Required, non-empty.
    pub name: String,
    /// Supabase project URL, e.g. `"https://abc123.supabase.co"`.
    pub url: String,
    /// Channel name the publisher writes to (no `realtime:` prefix —
    /// that's a wire detail the transport adds).
    pub channel: String,
    /// Inner `payload.event` filter. Empty string accepts any inner
    /// event (a publisher running with a custom `event_name=` still
    /// flows through without re-saving the profile).
    pub event_name: String,
    /// Default frame-filter DSL input (`name:foo file:/app/` …). Empty
    /// = no filter. UI pre-fill; never crosses the wire.
    pub frame_filter: String,
    /// Epoch seconds at creation.
    pub created_at: u64,
    /// Epoch seconds at last update.
    pub updated_at: u64,
}

impl RealtimeProfile {
    /// Build a new profile with a fresh id and "now" timestamps.
    /// Empty channel/event default to the publisher constants so the
    /// resulting profile is immediately usable without further input.
    pub fn new(name: impl Into<String>, url: impl Into<String>, now_secs: u64) -> Self {
        Self {
            id: ProfileId::new(),
            name: name.into(),
            url: url.into(),
            channel: DEFAULT_CHANNEL.to_string(),
            event_name: DEFAULT_EVENT_NAME.to_string(),
            frame_filter: String::new(),
            created_at: now_secs,
            updated_at: now_secs,
        }
    }
}

/// Project a profile down to the legacy [`RealtimeConfig`] shape so
/// the existing [`super::resolve`] / [`super::EffectiveStreamConfig`]
/// pipeline can consume it unchanged. Pure projection — no allocation
/// shortcut, no validation. Used wherever a use case has a profile but
/// the downstream resolver still speaks `RealtimeConfig`.
impl From<&RealtimeProfile> for RealtimeConfig {
    fn from(p: &RealtimeProfile) -> Self {
        RealtimeConfig {
            url: p.url.clone(),
            default_channel: p.channel.clone(),
            default_event: p.event_name.clone(),
            default_frame_filter: p.frame_filter.clone(),
        }
    }
}

/// Container for the user's profiles + which one is active. Replaces
/// the singleton [`super::RealtimeConfig`]; the old struct stays for
/// one release as the *migration source*.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeSettings {
    /// Saved profiles in creation order (newest last). The repository
    /// keeps this stable; the renderer is free to re-sort for display.
    #[serde(default)]
    pub profiles: Vec<RealtimeProfile>,
    /// ID of the currently active profile, or `None` when there's
    /// nothing active (no profiles yet, or the active one was just
    /// deleted — we don't auto-promote; the user picks).
    #[serde(default)]
    pub active_profile_id: Option<ProfileId>,
}

impl RealtimeSettings {
    /// Borrow the active profile if any. Returns `None` if no profile
    /// is active, or if `active_profile_id` points at a profile that
    /// was deleted (defensive — shouldn't happen if save/delete is
    /// transactional, but the trait permits inconsistent stores).
    pub fn active(&self) -> Option<&RealtimeProfile> {
        let id = self.active_profile_id.as_ref()?;
        self.profiles.iter().find(|p| &p.id == id)
    }

    pub fn find(&self, id: &ProfileId) -> Option<&RealtimeProfile> {
        self.profiles.iter().find(|p| &p.id == id)
    }

    pub fn find_mut(&mut self, id: &ProfileId) -> Option<&mut RealtimeProfile> {
        self.profiles.iter_mut().find(|p| &p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_profile_uses_publisher_defaults_for_routing() {
        let p = RealtimeProfile::new("a", "https://x.supabase.co", 42);
        assert_eq!(p.channel, DEFAULT_CHANNEL);
        assert_eq!(p.event_name, DEFAULT_EVENT_NAME);
        assert_eq!(p.frame_filter, "");
        assert_eq!(p.created_at, 42);
        assert_eq!(p.updated_at, 42);
    }

    #[test]
    fn active_returns_none_when_pointer_is_dangling() {
        let mut s = RealtimeSettings::default();
        s.active_profile_id = Some(ProfileId::from_str("nope"));
        assert!(s.active().is_none());
    }

    #[test]
    fn active_resolves_to_matching_profile() {
        let mut s = RealtimeSettings::default();
        let p = RealtimeProfile::new("a", "https://x.supabase.co", 0);
        let id = p.id.clone();
        s.profiles.push(p);
        s.active_profile_id = Some(id.clone());
        assert_eq!(s.active().map(|p| p.id.clone()), Some(id));
    }
}
