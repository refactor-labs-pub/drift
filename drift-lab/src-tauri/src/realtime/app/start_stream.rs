//! Use case: resolve a stream's configuration, allocate its log file, and
//! drive the WSS task to completion.
//!
//! Split into TWO methods on purpose:
//!
//! * [`StartStreamUseCase::prepare`] is *fast* and synchronous-ish (one
//!   mutex grab, one secret read, one filesystem touch). The Tauri
//!   command awaits it inline so the renderer can show "connecting…"
//!   based on the returned handle.
//! * [`StartStreamUseCase::run`] is the long-lived drain loop. The
//!   command spawns it onto Tauri's async runtime; it owns the WSS
//!   reconnect schedule and only returns when the cancellation token
//!   fires.
//!
//! The use case owns no state of its own beyond an `Arc<dyn
//! RealtimeTransport>` — every per-stream artefact (open sink, log path,
//! cancellation token) lives in the [`StreamPlan`] that flows from
//! `prepare` into `run`.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::realtime::domain::{
    resolve, EffectiveStreamConfig, RealtimeError, StreamOverrides,
};
use crate::realtime::ports::{
    ApiKeyVault, EventSink, EventSinkFactory, RealtimeTransport, StreamStatus,
};

/// Everything `run` needs to drive one stream. Public fields the Tauri
/// command consumes; private `sink` because nobody outside the use case
/// should touch the file handle.
pub struct StreamPlan {
    pub config: EffectiveStreamConfig,
    pub log_path: PathBuf,
    /// Open append-mode sink. Moved into the WSS task on `run`.
    sink: Box<dyn EventSink>,
}

impl StreamPlan {
    /// Where the stream's events will be written. Echoed to the
    /// renderer for "saved to …" messaging.
    pub fn log_path(&self) -> &PathBuf {
        &self.log_path
    }

    /// Replace the open sink with `decorator(current_sink)`. Used by the
    /// Tauri command to layer a `TeeSink` over the bare `JsonlSink` so
    /// each broadcast feeds an in-memory aggregator alongside the
    /// durable JSONL file — without exposing the sink field publicly or
    /// breaking the "the plan owns its sink" invariant.
    ///
    /// `FnOnce` because the swap happens exactly once between `prepare`
    /// and `run`; the closure receives the previously-open sink so it
    /// can be retained inside the decorator.
    pub fn wrap_sink<F>(&mut self, decorator: F)
    where
        F: FnOnce(Box<dyn EventSink>) -> Box<dyn EventSink>,
    {
        // Pull the old sink out via a Cell-style swap: replace with a
        // no-op placeholder so we never have an "uninitialised" field
        // in the gap between take and put. The placeholder is dropped
        // immediately when `decorator` returns.
        let placeholder: Box<dyn EventSink> = Box::new(NoopSink);
        let old = std::mem::replace(&mut self.sink, placeholder);
        self.sink = decorator(old);
    }
}

/// One-line `EventSink` that drops everything. Used only during the
/// swap inside `wrap_sink` so the field is never momentarily invalid.
struct NoopSink;

#[async_trait::async_trait]
impl EventSink for NoopSink {
    async fn append(
        &mut self,
        _payload: &serde_json::Value,
    ) -> Result<(), RealtimeError> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct StartStreamUseCase {
    transport: Arc<dyn RealtimeTransport>,
}

impl StartStreamUseCase {
    pub fn new(transport: Arc<dyn RealtimeTransport>) -> Self {
        Self { transport }
    }

    /// Resolve config + open the log file. Returns once the file exists
    /// (so the file-tail aggregator the caller starts next doesn't
    /// ENOENT before the first broadcast arrives).
    ///
    /// `settings` is the resolved config (URL + channel + event +
    /// frame_filter) the caller wants this stream to use — typically
    /// the active profile projected via `RealtimeProfile::into`. By
    /// taking it as a value instead of going through a
    /// `SettingsRepository`, the use case stays unaware of whether
    /// the source is the multi-profile store or the legacy single
    /// record — the command shim picks.
    pub async fn prepare<V, S>(
        &self,
        settings: crate::realtime::domain::RealtimeConfig,
        vault: &V,
        sink_factory: &S,
        overrides: StreamOverrides,
    ) -> Result<StreamPlan, RealtimeError>
    where
        V: ApiKeyVault + ?Sized,
        S: EventSinkFactory + ?Sized,
    {
        let api_key = vault.read()?;
        let config = resolve(&settings, &api_key, &overrides)?;
        let log_path = sink_factory.allocate_path()?;
        let sink = sink_factory.open(&log_path).await?;
        Ok(StreamPlan {
            config,
            log_path,
            sink,
        })
    }

    /// Drive the WSS task to completion. Returns when `cancel` fires or
    /// an unrecoverable error is encountered. Transient errors are
    /// handled inside the transport's reconnect loop and surface
    /// through `on_status` — they don't return from this method.
    pub async fn run(
        self,
        plan: StreamPlan,
        cancel: CancellationToken,
        on_status: Box<dyn Fn(StreamStatus) + Send + Sync>,
    ) -> Result<(), RealtimeError> {
        self.transport
            .run_stream(&plan.config, plan.sink, cancel, on_status)
            .await
    }
}
