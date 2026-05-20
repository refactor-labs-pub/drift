//! Pure URL builder — no I/O, no `url` crate dependency, fully testable.
//!
//! Mirrors the Python publisher's `_build_wss_url` (`supabase.py:90-104`)
//! line for line so the publisher's and subscriber's URL canonicalisation
//! never diverge.

use super::{defaults::VSN, errors::RealtimeError};

/// Build the Realtime WSS URL from a project URL + JWT.
///
/// Tolerates the four input shapes the Python builder tolerates:
/// `https://<host>`, `http://<host>`, `wss://<host>`, `ws://<host>`,
/// and a bare `<host>` with no scheme. Strips anything after the
/// authority (path / query / fragment) — Supabase URLs never carry one,
/// but users occasionally paste with trailing slashes.
pub fn build_wss_url(supabase_url: &str, api_key: &str) -> Result<String, RealtimeError> {
    let trimmed = supabase_url.trim();
    if trimmed.is_empty() {
        return Err(RealtimeError::InvalidUrl(supabase_url.to_string()));
    }
    if api_key.is_empty() {
        return Err(RealtimeError::MissingApiKey);
    }
    let host = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .or_else(|| trimmed.strip_prefix("wss://"))
        .or_else(|| trimmed.strip_prefix("ws://"))
        .unwrap_or(trimmed);
    let host = host.split(['/', '?', '#']).next().unwrap_or(host);
    if host.is_empty() {
        return Err(RealtimeError::InvalidUrl(supabase_url.to_string()));
    }
    Ok(format!(
        "wss://{host}/realtime/v1/websocket?apikey={api_key}&vsn={VSN}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_url_canonicalises_to_wss() {
        let got = build_wss_url("https://abc123.supabase.co", "jwt").unwrap();
        assert_eq!(
            got,
            "wss://abc123.supabase.co/realtime/v1/websocket?apikey=jwt&vsn=1.0.0"
        );
    }

    #[test]
    fn bare_host_is_accepted() {
        let got = build_wss_url("abc123.supabase.co", "jwt").unwrap();
        assert!(got.starts_with("wss://abc123.supabase.co/"));
    }

    #[test]
    fn trailing_slash_and_path_are_stripped() {
        let got = build_wss_url("https://abc123.supabase.co/foo?x=y", "jwt").unwrap();
        assert!(got.starts_with("wss://abc123.supabase.co/realtime/v1/"));
    }

    #[test]
    fn whitespace_is_trimmed() {
        let got = build_wss_url("  https://abc.supabase.co  ", "jwt").unwrap();
        assert!(got.starts_with("wss://abc.supabase.co/"));
    }

    #[test]
    fn empty_url_is_rejected() {
        match build_wss_url("", "jwt").unwrap_err() {
            RealtimeError::InvalidUrl(_) => {}
            e => panic!("expected InvalidUrl, got {e:?}"),
        }
    }

    #[test]
    fn empty_key_is_rejected_as_missing() {
        match build_wss_url("https://abc.supabase.co", "").unwrap_err() {
            RealtimeError::MissingApiKey => {}
            e => panic!("expected MissingApiKey, got {e:?}"),
        }
    }
}
