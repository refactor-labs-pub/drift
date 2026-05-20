//! Per-scan, fully-resolved stream config — what a single subscription
//! actually uses on the wire.
//!
//! [`RealtimeConfig`] is the *saved* state (URL + defaults). Each scan
//! layers optional overrides on top and resolves to a
//! [`EffectiveStreamConfig`] before the WSS task is spawned. Keeping the
//! "saved" type and the "in-flight" type distinct means a per-scan
//! override never mutates the user's persisted defaults — and the
//! resolver is one pure function that's trivial to unit-test.

use super::{defaults, errors::RealtimeError, settings::RealtimeConfig};

/// Optional per-scan overrides. Anything `Some` wins over the saved
/// defaults; `None` (or `Some("")`) falls through to the saved default,
/// then to the publisher-side constant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamOverrides {
    pub channel: Option<String>,
    pub event_filter: Option<String>,
}

impl StreamOverrides {
    /// Convenience for the common Tauri-command argument shape where
    /// every override is a separate `Option<String>`.
    pub fn from_options(channel: Option<String>, event_filter: Option<String>) -> Self {
        Self {
            channel,
            event_filter,
        }
    }
}

/// All inputs the WSS task needs, after defaults + overrides + secret
/// have been resolved. Pure data — no I/O, no Tokio types. The transport
/// adapter consumes one of these per stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveStreamConfig {
    /// The Supabase project URL (untouched — `build_wss_url` does the
    /// canonicalisation at the wire boundary). Always non-empty by
    /// construction: [`resolve`] rejects an empty URL.
    pub supabase_url: String,
    /// JWT for both socket auth and `phx_join.payload.access_token`.
    /// Always non-empty by construction.
    pub api_key: String,
    /// Channel name to subscribe to. Already non-empty (falls back to
    /// `defaults::DEFAULT_CHANNEL` if the user supplied nothing).
    pub channel: String,
    /// Inner `payload.event` filter. Empty string is a legitimate
    /// value here — it means "accept any inner event".
    pub event_filter: String,
}

/// Resolve a saved config + secret + optional per-scan overrides into the
/// in-flight config the transport will use. Pure function — no I/O, no
/// async.
///
/// Empty strings in overrides are treated the same as `None` (the UI
/// can't easily emit `null` from an empty text input, so we normalise
/// here). Empty strings in saved defaults likewise fall through.
pub fn resolve(
    settings: &RealtimeConfig,
    api_key: &str,
    overrides: &StreamOverrides,
) -> Result<EffectiveStreamConfig, RealtimeError> {
    let supabase_url = settings.url.trim();
    if supabase_url.is_empty() {
        return Err(RealtimeError::InvalidUrl(String::new()));
    }
    if api_key.is_empty() {
        return Err(RealtimeError::MissingApiKey);
    }
    let channel = pick(&overrides.channel, &settings.default_channel)
        .unwrap_or_else(|| defaults::DEFAULT_CHANNEL.to_string());
    // event_filter has a publisher-side default too — but unlike channel,
    // an empty string here is also legitimate (it means "accept any
    // inner event"). So we ONLY fall through to the default when neither
    // the override nor the saved setting provided ANYTHING.
    let event_filter = pick(&overrides.event_filter, &settings.default_event)
        .unwrap_or_else(|| defaults::DEFAULT_EVENT_NAME.to_string());
    Ok(EffectiveStreamConfig {
        supabase_url: supabase_url.to_string(),
        api_key: api_key.to_string(),
        channel,
        event_filter,
    })
}

/// `Some(value)` if `override_value` or `default` has non-whitespace
/// content; `None` if both are empty/whitespace. Override wins.
fn pick(override_value: &Option<String>, default: &str) -> Option<String> {
    if let Some(v) = override_value {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let t = default.trim();
    if !t.is_empty() {
        Some(t.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(url: &str, channel: &str, event: &str) -> RealtimeConfig {
        RealtimeConfig {
            url: url.into(),
            default_channel: channel.into(),
            default_event: event.into(),
            default_frame_filter: String::new(),
        }
    }

    #[test]
    fn overrides_win_over_saved_defaults() {
        let s = settings("https://x.supabase.co", "saved-ch", "saved-ev");
        let o = StreamOverrides::from_options(
            Some("override-ch".into()),
            Some("override-ev".into()),
        );
        let r = resolve(&s, "jwt", &o).unwrap();
        assert_eq!(r.channel, "override-ch");
        assert_eq!(r.event_filter, "override-ev");
    }

    #[test]
    fn empty_override_falls_through_to_saved() {
        let s = settings("https://x.supabase.co", "saved-ch", "saved-ev");
        let o = StreamOverrides::from_options(Some("".into()), Some("   ".into()));
        let r = resolve(&s, "jwt", &o).unwrap();
        assert_eq!(r.channel, "saved-ch");
        assert_eq!(r.event_filter, "saved-ev");
    }

    #[test]
    fn empty_saved_falls_through_to_publisher_default() {
        let s = settings("https://x.supabase.co", "", "");
        let o = StreamOverrides::default();
        let r = resolve(&s, "jwt", &o).unwrap();
        assert_eq!(r.channel, defaults::DEFAULT_CHANNEL);
        assert_eq!(r.event_filter, defaults::DEFAULT_EVENT_NAME);
    }

    #[test]
    fn empty_url_is_rejected() {
        let s = settings("", "ch", "ev");
        let o = StreamOverrides::default();
        assert!(matches!(
            resolve(&s, "jwt", &o),
            Err(RealtimeError::InvalidUrl(_))
        ));
    }

    #[test]
    fn empty_key_is_rejected() {
        let s = settings("https://x.supabase.co", "ch", "ev");
        let o = StreamOverrides::default();
        assert!(matches!(
            resolve(&s, "", &o),
            Err(RealtimeError::MissingApiKey)
        ));
    }
}
