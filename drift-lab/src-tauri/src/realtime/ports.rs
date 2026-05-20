//! Ports — the interfaces use cases depend on.
//!
//! Each trait is the seam between the application layer (`app/`) and a
//! concrete adapter (`infra/`). Use cases hold trait objects (or are
//! generic over the trait); adapters live behind the trait and are the
//! only place I/O happens.
//!
//! The cardinal rule: nothing in this file may name a type from `tokio`,
//! `tauri`, `tungstenite`, or `std::fs`. If a method would need to, the
//! signature is wrong — find the right abstraction.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::domain::{
    EffectiveStreamConfig, ProfileId, RealtimeConfig, RealtimeError, RealtimeProfile,
    RealtimeSettings,
};

// ---------------------------------------------------------------------------
// SettingsRepository — load and persist the user's saved defaults.
// ---------------------------------------------------------------------------

/// Read/write the user's persisted realtime config. The adapter is
/// responsible for whatever serialisation / file layout the persistence
/// store needs; the trait surface is "give me the current settings,
/// take this new one and durably save it".
///
/// `async` because the underlying tauri-plugin-store calls block briefly
/// on disk and the adapter today holds a `tokio::sync::Mutex` over the
/// in-memory copy. Keeping the trait async lets a future SQLite adapter
/// drop in without changing every caller.
#[async_trait]
pub trait SettingsRepository: Send + Sync {
    async fn load(&self) -> Result<RealtimeConfig, RealtimeError>;
    async fn save(&self, settings: &RealtimeConfig) -> Result<(), RealtimeError>;
}

// ---------------------------------------------------------------------------
// ProfileRepository — multi-profile persistence.
// ---------------------------------------------------------------------------

/// Persistence for the per-project profile set. Replaces
/// [`SettingsRepository`] going forward; both coexist for the migration
/// window so the legacy use cases keep compiling against the old type
/// while the new ones speak in terms of [`RealtimeProfile`].
///
/// Operations are coarse-grained on purpose — the implementation is
/// free to batch the full settings doc on disk. Save & delete are
/// expected to be atomic at the on-disk level (write the whole
/// `RealtimeSettings` blob), so a crash mid-write doesn't leave a
/// partial profile list.
#[async_trait]
pub trait ProfileRepository: Send + Sync {
    /// Snapshot of the entire saved state.
    async fn load(&self) -> Result<RealtimeSettings, RealtimeError>;

    /// Save (insert-or-update by id) one profile. Sets `updated_at` to
    /// "now" inside the adapter — callers don't need to mind timestamps.
    /// Returns the saved profile so the renderer sees the exact
    /// persisted shape (including any server-side normalisation).
    async fn save(&self, profile: RealtimeProfile) -> Result<RealtimeProfile, RealtimeError>;

    /// Remove the profile. Idempotent — returns `Ok(false)` when no
    /// profile with that id existed. Side-effect: if the removed
    /// profile was active, `active_profile_id` is cleared (we don't
    /// auto-promote — explicit > implicit).
    async fn delete(&self, id: &ProfileId) -> Result<bool, RealtimeError>;

    /// Set / clear which profile is active. Passing an id that doesn't
    /// exist returns [`RealtimeError::Io`] — we don't accept dangling
    /// pointers, since the renderer would otherwise see an "active but
    /// missing" state.
    async fn set_active(&self, id: Option<ProfileId>) -> Result<(), RealtimeError>;
}

// ---------------------------------------------------------------------------
// ApiKeyVault — read the realtime JWT.
// ---------------------------------------------------------------------------

/// Pull the realtime API key out of secure storage. The renderer can
/// only WRITE secrets (via the existing `set_secret` Tauri command) —
/// reads happen server-side here.
///
/// **Per-profile binding** (PR-2a): the trait is intentionally keyless.
/// Profile awareness lives at the *adapter* level — each
/// `FileApiKeyVault` is constructed for ONE specific profile id via
/// [`FileApiKeyVault::for_profile`](crate::realtime::infra::FileApiKeyVault::for_profile)
/// and reads from the namespaced SecretStore key
/// `supabase_realtime_api_key:<profile_id>`. Use cases never see the
/// profile id; they hold a vault and call `read()`.
pub trait ApiKeyVault: Send + Sync {
    /// Returns the saved JWT, or [`RealtimeError::MissingApiKey`] when
    /// no key has been configured. The vault must never return an empty
    /// string masquerading as "configured".
    fn read(&self) -> Result<String, RealtimeError>;
}

// ---------------------------------------------------------------------------
// EventSink — where the WSS task's broadcast payloads land.
// ---------------------------------------------------------------------------

/// Open a logical sink (today: a JSONL file under `~/.drift/event_logs/`)
/// and append one event per call. The file-tail aggregator reads it back
/// at ~1 Hz on a separate thread, so the contract is "one JSON object
/// per line, newline-terminated, durably flushed".
///
/// The trait is async because the production adapter uses
/// `tokio::fs::File`; tests can implement a synchronous version that
/// returns ready futures.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Append one event. `payload` is the *inner* `payload.payload` dict
    /// extracted from the Phoenix broadcast envelope — already in the
    /// shape the file-tail aggregator expects.
    async fn append(&mut self, payload: &serde_json::Value) -> Result<(), RealtimeError>;
}

/// Factory for opening a sink at a specific path. Lets the
/// `StartStreamUseCase` decide WHERE to write while staying ignorant of
/// HOW. The adapter creates the parent dir, touches the file (so a
/// downstream tailer doesn't ENOENT before the first broadcast), and
/// returns an open append-mode handle.
#[async_trait]
pub trait EventSinkFactory: Send + Sync {
    async fn open(&self, path: &Path) -> Result<Box<dyn EventSink>, RealtimeError>;

    /// Where new realtime logs should land by default. Today this is
    /// `~/.drift/event_logs/realtime-<stamp>.jsonl`. The factory returns
    /// the absolute path so the caller can echo it back to the UI for
    /// "your session was saved to …" messaging.
    fn allocate_path(&self) -> Result<PathBuf, RealtimeError>;
}

// ---------------------------------------------------------------------------
// RealtimeTransport — the WSS-facing seam.
// ---------------------------------------------------------------------------

/// Outcome of a test connect+join. Distinguishes "everything worked" from
/// the specific failure modes the UI needs to render differently. Errors
/// from the transport always come back as a `Result<_, RealtimeError>` —
/// the use case decides whether to surface them as a typed result or a
/// `Result::Err`. The current Settings UI prefers the typed result so a
/// failed test renders as a red badge, not a Tauri command rejection.
#[derive(Debug)]
pub enum TestConnectionOutcome {
    Ok,
    Failed(RealtimeError),
}

/// Stage of the one-shot connect+join test. Surfaced via the
/// `on_progress` callback so the UI can render "Connecting… → Joining…
/// → Awaiting reply…" instead of a single opaque spinner. The renderer
/// gets a clear breadcrumb when a slow / hung step costs the test.
#[derive(Debug, Clone, Copy)]
pub enum TestStage {
    /// Building the request and opening the WSS handshake.
    Connecting,
    /// Sending `phx_join` to the channel.
    Joining,
    /// Waiting for `phx_reply{ok}` from the server.
    AwaitingReply,
}

/// Drive a Supabase Realtime subscription. Two operations:
///
/// 1. [`test_connection`] — one-shot connect → `phx_join` → wait for
///    `phx_reply{ok}` → close. Used by Settings to validate creds.
///
/// 2. [`run_stream`] — long-lived subscription. Drains broadcasts into
///    the given [`EventSink`] until `cancel` is cancelled or an
///    unrecoverable error is returned. The adapter owns reconnect /
///    heartbeat / framing.
#[async_trait]
pub trait RealtimeTransport: Send + Sync {
    /// One-shot connect+join+close. `on_progress` is invoked synchronously
    /// at each stage transition so the renderer can swap the button label;
    /// it must not block (queue an emit and return).
    ///
    /// `cancel` lets the caller abort mid-flight (Stop-during-test). The
    /// transport must wake any blocking await on this token and return
    /// [`TestConnectionOutcome::Failed`] carrying [`RealtimeError::Cancelled`]
    /// within one event-loop tick; long backoffs / re-attempts are not
    /// applicable here (the test is a single-shot) but a hung
    /// `connect_async` or recv must not pin shutdown.
    async fn test_connection(
        &self,
        config: &EffectiveStreamConfig,
        on_progress: Box<dyn Fn(TestStage) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<TestConnectionOutcome, RealtimeError>;

    async fn run_stream(
        &self,
        config: &EffectiveStreamConfig,
        sink: Box<dyn EventSink>,
        cancel: CancellationToken,
        on_status: Box<dyn Fn(StreamStatus) + Send + Sync>,
    ) -> Result<(), RealtimeError>;
}

/// Status updates the transport emits during a long-lived stream so the
/// UI can render reconnect / error state. Kept narrow on purpose — the
/// transport doesn't expose every internal frame, just the lifecycle
/// transitions the renderer cares about.
#[derive(Debug, Clone)]
pub enum StreamStatus {
    /// Successfully connected and joined the channel.
    Connected,
    /// A transient error occurred; the transport will reconnect after
    /// `retry_in_secs`. Mirrors the existing renderer-facing string
    /// ("reconnecting after Ns: …") so PR-1 doesn't change UI behavior.
    Reconnecting { retry_in_secs: u64, reason: String },
}
