//! Typed errors crossing the domain → application boundary. No `String`
//! errors leak past the use cases — adapters convert their I/O failures
//! into these variants so the renderer-facing layer can map exhaustively.

use std::fmt;

/// Closed set of failure modes the realtime subsystem can surface.
/// Each variant captures the information the UI needs to either ask the
/// user to fix something or display a precise diagnostic — never a
/// stringly-typed blob.
#[derive(Debug)]
pub enum RealtimeError {
    /// The Supabase project URL is empty or unparseable. Carries the raw
    /// input so we can echo it back in the error message.
    InvalidUrl(String),
    /// No API key has been supplied — either the user hasn't saved one
    /// or the vault returned `None`. Distinct from `ApiKeyRejected`
    /// because the remedy is different ("paste your key" vs. "your key
    /// is bad").
    MissingApiKey,
    /// The realtime URL is configured but the server refused the
    /// connection / handshake. Common causes: DNS, TLS, network down.
    /// Carries the underlying diagnostic for logs (NOT for the user).
    ConnectFailed(String),
    /// We connected but the `phx_join` reply came back with
    /// `status="error"`. Usually means wrong project URL, expired JWT,
    /// or private channel without RLS configured. The `reason` is the
    /// server's own error payload.
    ChannelRejected { reason: String },
    /// The connect+join cycle didn't finish within the budget. Carries
    /// the budget so the message can be precise.
    Timeout { seconds: u64 },
    /// Adapter-layer I/O failure that doesn't fit the above (filesystem
    /// write of the JSONL sink, serde error parsing a server frame,
    /// etc.). Use sparingly — prefer adding a specific variant.
    Io(String),
    /// The caller cancelled the operation mid-flight (Stop button on
    /// the test command, or Stop on a long-lived stream). Not really
    /// an error — kept here so the use case can return a uniform
    /// `Result` shape, and so the UI can render "Test cancelled."
    /// instead of "an unknown error occurred".
    Cancelled,
}

impl fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(url) => {
                write!(f, "invalid Supabase URL: {url}")
            }
            Self::MissingApiKey => {
                write!(f, "no API key configured (Settings → Realtime)")
            }
            Self::ConnectFailed(reason) => {
                write!(f, "connect failed: {reason}")
            }
            Self::ChannelRejected { reason } => {
                write!(f, "channel rejected: {reason}")
            }
            Self::Timeout { seconds } => {
                write!(f, "connect+join timed out after {seconds}s")
            }
            Self::Io(msg) => write!(f, "{msg}"),
            Self::Cancelled => write!(f, "Test cancelled."),
        }
    }
}

impl std::error::Error for RealtimeError {}
