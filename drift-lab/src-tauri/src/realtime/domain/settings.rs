//! Persistent realtime configuration — the user's saved defaults.
//!
//! Field names mirror the env vars `drift-profiler-python/.../sinks/supabase.py`
//! reads, so a `.env` copy-paste needs zero translation:
//!
//! | Field                  | Env var                         |
//! |------------------------|---------------------------------|
//! | `url`                  | `SUPABASE_URL`                  |
//! | `default_channel`      | `SUPABASE_REALTIME_CHANNEL`     |
//! | `default_event`        | (publisher `event_name=` kwarg) |
//! | `default_frame_filter` | (subscriber-side mini-DSL)      |
//!
//! The API key (`SUPABASE_REALTIME_API_KEY`) is intentionally NOT a field
//! here — it's a secret and belongs in [`crate::secret_store::SecretStore`].
//! Keeping the secret out of this struct guarantees that a "dump my config"
//! / config-file-diffing feature can never accidentally exfiltrate the JWT.

use serde::{Deserialize, Serialize};

/// User's saved realtime defaults. Today there is one of these globally;
/// PR-2 will introduce `Vec<RealtimeProfile>` and keep this struct as the
/// per-profile settings payload (renamed at that point).
///
/// `#[serde(default)]` on every field so configs written before a field
/// existed load cleanly with an empty string — and so adding a new field
/// later never breaks deserialization for upgrading users.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeConfig {
    /// Supabase project URL, e.g. `https://abc123.supabase.co`. Empty
    /// string until the user enters one in Settings → Realtime; the
    /// stream / test commands surface this as `RealtimeError::InvalidUrl`.
    #[serde(default)]
    pub url: String,

    /// Default channel name. Pre-fills the channel input on every new
    /// scan. Empty string means "use the publisher default", which
    /// resolves to [`super::defaults::DEFAULT_CHANNEL`] at the start of
    /// the stream.
    #[serde(default)]
    pub default_channel: String,

    /// Default inner `payload.event` filter. Subscriber drops broadcasts
    /// whose `payload.event` doesn't match. Empty = accept all event
    /// names (so a publisher running with a custom `event_name=` kwarg
    /// still flows through without per-scan reconfiguration).
    #[serde(default)]
    pub default_event: String,

    /// Default frame-filter DSL input (`name:foo file:/app/` …). Empty
    /// = no filter. This is a UI pre-fill; the actual matching happens
    /// renderer-side and never touches the wire.
    #[serde(default)]
    pub default_frame_filter: String,
}
