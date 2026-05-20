//! Tauri command shims for the Supabase Realtime subsystem.
//!
//! Every command in this file is a thin wrapper that:
//!   1. Translates camelCase JS args into domain types.
//!   2. Builds adapters from the app handle / state.
//!   3. Invokes a use case from [`crate::realtime::app`].
//!   4. Maps [`RealtimeError`] → `String` for the renderer.
//!
//! No business logic, no protocol details, no transport plumbing — those
//! all live in [`crate::realtime`]. If a change here grows past
//! ~20 lines, that's a hint the change belongs in the use case or the
//! adapter, not the shim.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime, State};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::event_log_commands::{
    topic as event_log_topic, LiveAggPayload, LiveErrorPayload,
};
use crate::realtime::{
    app::{
        ActivateProfileUseCase, DeleteProfileUseCase, ListProfilesUseCase, SaveProfileInput,
        SaveProfileUseCase, StartStreamUseCase, TestConnectionUseCase, TestInputs,
    },
    domain::{ProfileId, RealtimeError, RealtimeProfile, RealtimeSettings, StreamOverrides},
    infra::{
        namespaced_realtime_api_key_for, AggregatorHandle, AggregatorSink,
        AppConfigProfileRepository, FileApiKeyVault, JsonlSinkFactory, TeeSink,
        TungsteniteTransport,
    },
    ports::{StreamStatus, TestConnectionOutcome, TestStage},
};
use crate::secret_store::{FileSecretStore, SecretStore};
use crate::state::AppState;

/// How long the live broadcaster waits between snapshot emits. 250 ms ≈
/// 4 Hz — fast enough for the UI to feel live, slow enough to coalesce
/// 100+ event bursts into a single re-render. Stays below the 5-minute
/// prompt-cache window so debouncing logic remains cheap.
const LIVE_BROADCAST_DEBOUNCE_MS: u64 = 250;

/// Tauri event topics the realtime subsystem emits.
pub mod topic {
    /// Per-stage update emitted during a connect-test so the renderer can
    /// swap "Testing…" with "Connecting…" → "Joining…" → "Awaiting reply…".
    /// Payload: `TestProgressPayload`.
    pub const REALTIME_TEST_PROGRESS: &str = "realtime://test_progress";
}

/// Payload for [`topic::REALTIME_TEST_PROGRESS`]. `stage` is the
/// machine-readable token; `label` is the human string the UI shows on
/// the button — provided server-side so wording stays consistent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestProgressPayload {
    pub stage: &'static str,
    pub label: &'static str,
}

fn progress_payload_for(stage: TestStage) -> TestProgressPayload {
    match stage {
        TestStage::Connecting => TestProgressPayload {
            stage: "connecting",
            label: "Connecting…",
        },
        TestStage::Joining => TestProgressPayload {
            stage: "joining",
            label: "Joining channel…",
        },
        TestStage::AwaitingReply => TestProgressPayload {
            stage: "awaiting_reply",
            label: "Awaiting reply…",
        },
    }
}

/// Build a progress callback that emits on the Tauri event bus. Cheap to
/// construct per-test (just clones the AppHandle, which is itself an
/// `Arc`); we make a fresh one each invocation so the closure captures
/// only what it needs.
fn make_progress_emitter<R: Runtime>(app: AppHandle<R>) -> Box<dyn Fn(TestStage) + Send + Sync> {
    Box::new(move |stage: TestStage| {
        let _ = app.emit(topic::REALTIME_TEST_PROGRESS, progress_payload_for(stage));
    })
}

// ===========================================================================
// In-flight stream registry
// ===========================================================================
//
// Lives in this file (not in `realtime::` proper) because it's a delivery-
// layer concern: it ties the WSS task's cancellation token to the file-tail
// aggregator's `live_scan_id` so the UI's single "Stop" click can cancel
// both. The use case knows nothing about either side of this registry.

pub type RealtimeStreams = Arc<Mutex<std::collections::HashMap<String, RealtimeStreamEntry>>>;

#[derive(Debug, Clone)]
pub struct RealtimeStreamEntry {
    pub wss_token: CancellationToken,
    /// `live_scan_id` of the paired file-tail aggregator.
    pub live_scan_id: String,
    pub log_path: PathBuf,
}

// ===========================================================================
// IPC types — kept compatible with the existing TypeScript bindings.
// ===========================================================================

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub ok: bool,
    /// Human-readable status: `"Connected and joined channel '<name>'"`
    /// on success, the error message on failure.
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeStreamHandle {
    pub stream_id: String,
    pub live_scan_id: String,
    pub log_path: String,
}

// ===========================================================================
// test_realtime_connection — unified test (both UI surfaces)
// ===========================================================================
//
// One command, two callers:
//
//   * Settings form passes everything the user typed (URL + key + channel).
//   * LiveScan "Test" button passes only the channel — `supabase_url` and
//     `api_key` come back as `None`, the use case falls through to the
//     saved URL + the SecretStore JWT.
//
// `test_id` is a renderer-generated UUID. The command registers a
// cancellation token under it; a Stop click sends `cancel_realtime_test
// (test_id)` to the registry, which wakes the transport's `select!` arm
// within one poll cycle. The entry is removed automatically on completion
// — no leak even if the renderer crashes mid-test.

#[tauri::command]
pub async fn test_realtime_connection<R: Runtime>(
    test_id: String,
    // Each override is optional. `None` and `Some("")` both fall through
    // to the saved value. Keeps a one-arg JS surface where the renderer
    // can pass `null` for "use the saved one" without a special case.
    supabase_url: Option<String>,
    api_key: Option<String>,
    channel: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<TestConnectionResult, String> {
    // ----- 1. register a cancel token under test_id -----
    let cancel = CancellationToken::new();
    {
        let mut guard = state.realtime_tests.lock().await;
        guard.insert(test_id.clone(), cancel.clone());
    }

    // ----- 2. wire adapters + use case -----
    // Build the progress emitter FIRST (AppHandle moves out next).
    let progress = make_progress_emitter(app.clone());
    // Project the active profile (if any) into a RealtimeConfig the
    // use case understands. When no profile is active (Settings →
    // Add → Test before saving), we fall back to an empty config —
    // the use case takes overrides on top, so the explicit URL/key
    // typed in the form drive the test.
    let (settings_for_test, vault): (crate::realtime::domain::RealtimeConfig, FileApiKeyVault<R>) = {
        let cfg = state.app_config.lock().await;
        match cfg.realtime_settings.active_profile_id.as_ref() {
            Some(id) => {
                let profile = cfg
                    .realtime_settings
                    .find(id)
                    .cloned();
                let v = FileApiKeyVault::for_profile(app.clone(), id);
                let s = profile.as_ref().map(Into::into).unwrap_or_default();
                (s, v)
            }
            #[allow(deprecated)]
            None => (
                crate::realtime::domain::RealtimeConfig::default(),
                FileApiKeyVault::new(app.clone()),
            ),
        }
    };
    let transport = TungsteniteTransport::new();
    let use_case = TestConnectionUseCase::new(settings_for_test, &vault, &transport);

    // Resolve what channel the success message will name. The use case
    // does this internally too; pre-computing here keeps the message
    // wording in one place and avoids a second repo lookup. Falls through
    // to saved → publisher default exactly like the use case does.
    let channel_label = resolve_channel_label(&state, channel.clone()).await;

    let inputs = TestInputs {
        supabase_url,
        api_key,
        channel,
    };

    // ----- 3. run, then ALWAYS clean up the registry entry -----
    // `defer`-style guard via a scope-local closure isn't ergonomic in
    // Rust without a helper crate; do the cleanup explicitly on every
    // exit path. Cancellation can race the natural completion, so the
    // dedup-on-remove is intentional.
    let result = use_case.execute(inputs, progress, cancel).await;
    {
        let mut guard = state.realtime_tests.lock().await;
        guard.remove(&test_id);
    }

    match result {
        Ok(TestConnectionOutcome::Ok) => Ok(TestConnectionResult {
            ok: true,
            message: format!("Connected and joined channel '{channel_label}'"),
        }),
        Ok(TestConnectionOutcome::Failed(e)) => {
            tracing::warn!("realtime test {test_id} failed: {e}");
            Ok(TestConnectionResult {
                ok: false,
                message: friendly_failure_message(&e),
            })
        }
        // Reaches here when the use case can't even attempt the test —
        // URL or key missing on the "use saved" path. Render the typed
        // hint instead of a bare error message.
        Err(e) => Ok(TestConnectionResult {
            ok: false,
            message: friendly_unconfigured_message(&e),
        }),
    }
}

// ===========================================================================
// cancel_realtime_test — Stop button for an in-flight test
// ===========================================================================

/// Cancel an in-flight realtime test by its renderer-generated `test_id`.
/// Idempotent: returns `false` if no test with that id is currently
/// running. The transport's `select!` wakes within one poll cycle; the
/// command itself returns immediately.
#[tauri::command]
pub async fn cancel_realtime_test(
    test_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let token = {
        let mut guard = state.realtime_tests.lock().await;
        guard.remove(&test_id)
    };
    match token {
        Some(t) => {
            t.cancel();
            Ok(true)
        }
        None => Ok(false),
    }
}

// ===========================================================================
// start_realtime_event_stream — long-lived subscriber
// ===========================================================================

#[tauri::command]
pub async fn start_realtime_event_stream<R: Runtime>(
    folder_fingerprint: String,
    channel: Option<String>,
    event_filter: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<RealtimeStreamHandle, String> {
    // ----- folder gate -----
    // Both static scans and active runs are scoped to a folder so they
    // can be joined later. Refuse to start if the renderer didn't pass
    // a fingerprint, or if it's not a valid shape, or if no static
    // scan has run against this folder yet — that last check is the
    // "active scan only against statically-scanned folders" rule.
    let folder_fp = crate::folder::FolderFingerprint::parse(&folder_fingerprint).ok_or_else(
        || format!("invalid folder fingerprint shape: {folder_fingerprint}"),
    )?;
    if !crate::folder::has_static_scan(&folder_fp) {
        return Err(
            "This folder hasn't been statically scanned yet. Run a static scan against it \
             first so we have a code reference to join live samples against."
                .into(),
        );
    }

    // ----- active profile gate -----
    let active_profile_id = {
        let cfg = state.app_config.lock().await;
        cfg.realtime_settings.active_profile_id.clone()
    };
    let Some(profile_id) = active_profile_id else {
        return Err(
            "No active realtime profile. Settings → Realtime → Add / Activate one first.".into(),
        );
    };
    // Project the active profile to a RealtimeConfig (the use case
    // speaks in this shape; the conversion lives in domain::profile).
    let settings_for_stream: crate::realtime::domain::RealtimeConfig = {
        let cfg = state.app_config.lock().await;
        let profile = cfg
            .realtime_settings
            .find(&profile_id)
            .ok_or_else(|| {
                "Active profile id is dangling — open Settings → Realtime to pick one.".to_string()
            })?;
        profile.into()
    };
    let vault = FileApiKeyVault::for_profile(app.clone(), &profile_id);
    // Sink factory scoped to THIS folder's event_logs dir
    // (`~/.drift/scans/<fp>/event_logs/`). Past runs for the same
    // folder are colocated; the Active Scan rail can list them per
    // folder.
    let sink_factory = JsonlSinkFactory::for_folder(&folder_fp).map_err(|e| e.to_string())?;
    let transport: Arc<dyn crate::realtime::ports::RealtimeTransport> =
        Arc::new(TungsteniteTransport::new());
    let use_case = StartStreamUseCase::new(transport);

    // ----- 1. resolve config + open the log file -----
    let overrides = StreamOverrides::from_options(channel, event_filter);
    let mut plan = use_case
        .prepare(settings_for_stream, &vault, &sink_factory, overrides)
        .await
        .map_err(|e| e.to_string())?;

    let log_path = plan.log_path().clone();

    // ----- 2. layer an in-memory aggregator over the JSONL sink -----
    //
    // The transport receives a `TeeSink` that writes each broadcast both
    // to disk (replay / persistence) AND into the shared `Aggregator`
    // (live snapshots). The bespoke 1 Hz file-tail aggregator the old
    // path used is gone — we get sub-second updates AND skip the redundant
    // re-read of the JSONL file from disk.
    let agg_handle = AggregatorHandle::new();
    let agg_handle_for_broadcaster = agg_handle.clone();
    plan.wrap_sink(|jsonl| {
        Box::new(TeeSink::new(
            jsonl,
            Box::new(AggregatorSink::new(agg_handle)),
        ))
    });

    // ----- 3. register the stream so Stop can cancel both halves -----
    // `live_scan_id` is still a UUID — but it now identifies the
    // broadcaster, not a file-tail. The UI keeps matching aggregate
    // payloads by this id, so the wire shape is unchanged.
    let live_scan_id = Uuid::new_v4().to_string();
    let stream_id = Uuid::new_v4().to_string();
    let wss_token = CancellationToken::new();
    {
        let mut guard = state.realtime_streams.lock().await;
        guard.insert(
            stream_id.clone(),
            RealtimeStreamEntry {
                wss_token: wss_token.clone(),
                live_scan_id: live_scan_id.clone(),
                log_path: log_path.clone(),
            },
        );
    }

    // ----- 4. spawn the live broadcaster (snapshot emit at ≤4 Hz) -----
    spawn_live_broadcaster(
        agg_handle_for_broadcaster,
        app.clone(),
        live_scan_id.clone(),
        log_path.clone(),
        wss_token.clone(),
    );

    // ----- 5. spawn the long-lived WSS drain task -----
    let app_for_task = app.clone();
    let live_scan_id_for_task = live_scan_id.clone();
    let streams_for_cleanup = Arc::clone(&state.realtime_streams);
    let stream_id_for_cleanup = stream_id.clone();

    tauri::async_runtime::spawn(async move {
        // Translate use-case `StreamStatus` into the existing
        // `LiveErrorPayload` UI shape so PR-1 doesn't perturb renderer
        // listeners. PR-2 can introduce a richer status payload.
        let on_status: Box<dyn Fn(StreamStatus) + Send + Sync> = {
            let app = app_for_task.clone();
            let live_scan_id = live_scan_id_for_task.clone();
            Box::new(move |status| match status {
                StreamStatus::Connected => {
                    // The old code didn't emit a "connected" frame — keep
                    // that behavior. The aggregator's first non-empty
                    // tick implicitly tells the UI we're live.
                }
                StreamStatus::Reconnecting {
                    retry_in_secs,
                    reason,
                } => {
                    let _ = app.emit(
                        event_log_topic::EVENT_LOG_ERR,
                        LiveErrorPayload {
                            live_scan_id: live_scan_id.clone(),
                            message: format!(
                                "reconnecting after {retry_in_secs}s: {reason}"
                            ),
                        },
                    );
                }
            })
        };

        if let Err(e) = use_case.run(plan, wss_token, on_status).await {
            tracing::warn!(
                "realtime stream {} hit unrecoverable error: {e}",
                live_scan_id_for_task
            );
            let _ = app_for_task.emit(
                event_log_topic::EVENT_LOG_ERR,
                LiveErrorPayload {
                    live_scan_id: live_scan_id_for_task.clone(),
                    message: e.to_string(),
                },
            );
        }

        // Cleanup: drop the registry entry. The file-tail aggregator
        // keeps running until `stop_realtime_event_stream` cancels it
        // explicitly — matches the prior contract.
        let mut guard = streams_for_cleanup.lock().await;
        guard.remove(&stream_id_for_cleanup);
    });

    Ok(RealtimeStreamHandle {
        stream_id,
        live_scan_id,
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

// ===========================================================================
// stop_realtime_event_stream — single cancel cancels the WSS task AND the
// live broadcaster (both share `wss_token`).
// ===========================================================================

#[tauri::command]
pub async fn stop_realtime_event_stream(
    stream_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let entry = {
        let mut guard = state.realtime_streams.lock().await;
        guard.remove(&stream_id)
    };
    let Some(entry) = entry else {
        return Ok(false);
    };
    // Cancels: (a) the WSS drain task in the transport, and
    //          (b) the live aggregator broadcaster, which shares the
    //              same token so a single Stop click tears down both.
    // The file-tail aggregator path is no longer used for live realtime
    // — the in-memory broadcaster replaced it — so there's nothing else
    // to stop here. `live_scan_id` stays on the entry only as a wire
    // identifier for the UI's `LiveAggPayload` matcher.
    let _ = entry.live_scan_id; // suppress unused-field lint
    entry.wss_token.cancel();
    Ok(true)
}

// ===========================================================================
// helpers
// ===========================================================================

/// What channel name the saved-test will end up using, computed up-front
/// so the success message can name the exact channel without round-
/// tripping through the use case again. Tolerates an empty saved default
/// (falls through to the publisher default).
async fn resolve_channel_label(
    state: &State<'_, AppState>,
    override_value: Option<String>,
) -> String {
    if let Some(v) = override_value {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let saved = state.app_config.lock().await.realtime.default_channel.clone();
    if !saved.trim().is_empty() {
        return saved.trim().to_string();
    }
    crate::realtime::domain::defaults::DEFAULT_CHANNEL.to_string()
}

/// The old saved-test command short-circuited with bespoke strings when
/// the URL or key wasn't set yet ("Supabase URL is not configured…",
/// "No API key configured…"). The use case returns typed
/// [`RealtimeError`] for the same cases; map to the same strings so the
/// renderer keeps showing the same messages.
fn friendly_unconfigured_message(e: &RealtimeError) -> String {
    match e {
        RealtimeError::InvalidUrl(_) => {
            "Supabase URL is not configured (Settings → Realtime)".into()
        }
        RealtimeError::MissingApiKey => {
            "No API key configured (Settings → Realtime)".into()
        }
        other => other.to_string(),
    }
}

/// Spawn the live-aggregate broadcaster task.
///
/// The broadcaster owns one half of the live-aggregation pipeline: the
/// other half is the [`AggregatorSink`] feeding events INTO the
/// [`AggregatorHandle`]. Every time a broadcast arrives, the sink
/// `notify_one`s us; we wake, debounce for [`LIVE_BROADCAST_DEBOUNCE_MS`]
/// so a burst of 100+ events coalesces into one emit, snapshot the
/// aggregator, and emit a [`LiveAggPayload`] on the same Tauri topic the
/// file-tail aggregator uses.
///
/// The UI's subscriber matches by `liveScanId`, so the wire shape stays
/// identical between file-load and live-realtime modes — no UI change
/// required for the new path.
///
/// Task exits when `cancel` is cancelled. Before exiting we do ONE
/// final snapshot+emit so the last few events (received during the
/// debounce window before Stop) aren't lost on the wire.
fn spawn_live_broadcaster<R: Runtime>(
    handle: AggregatorHandle,
    app: AppHandle<R>,
    live_scan_id: String,
    source_file: PathBuf,
    cancel: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        let source_label = source_file.to_string_lossy().into_owned();
        let mut last_emitted_events: u32 = 0;

        loop {
            // 1. Wait for either an event-arrival notification or shutdown.
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = handle.notify.notified() => {}
            }

            // 2. Debounce — any events that land in this window get
            //    folded into the next snapshot. Cancellation also wakes
            //    us early so Stop doesn't have to wait for the debounce.
            tokio::select! {
                _ = cancel.cancelled() => {
                    emit_snapshot(&handle, &app, &live_scan_id, &source_label).await;
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(LIVE_BROADCAST_DEBOUNCE_MS)) => {}
            }

            // 3. Snapshot + emit. Skip the emit if the event counter
            //    hasn't moved — `notify_one` can fire spuriously when a
            //    permit is already queued, and we don't want to wake the
            //    UI for zero new data.
            let total = {
                let agg = handle.aggregator.lock().await;
                agg.total_events()
            };
            if total != last_emitted_events {
                last_emitted_events = total;
                emit_snapshot(&handle, &app, &live_scan_id, &source_label).await;
            }
        }
    });
}

/// Take one snapshot under the aggregator lock and emit it as a
/// `LiveAggPayload` on the existing `event_log://aggregate` topic.
async fn emit_snapshot<R: Runtime>(
    handle: &AggregatorHandle,
    app: &AppHandle<R>,
    live_scan_id: &str,
    source_file: &str,
) {
    let report = {
        let agg = handle.aggregator.lock().await;
        agg.snapshot(source_file)
    };
    let _ = app.emit(
        event_log_topic::EVENT_LOG_AGG,
        LiveAggPayload {
            live_scan_id: live_scan_id.to_string(),
            report,
        },
    );
}

/// User-facing message for test failures. Default is the typed error's
/// `Display`, but a `Timeout` gets a targeted hint — server silence past
/// the budget is almost always "Realtime isn't enabled on this project"
/// or "the JWT is bad enough that the server is dropping the join
/// silently", and that's worth telling the user instead of a bare
/// "timed out".
fn friendly_failure_message(e: &RealtimeError) -> String {
    match e {
        RealtimeError::Timeout { seconds } => format!(
            "No reply from the server within {seconds}s. Common causes: \
             Realtime is not enabled on this Supabase project, or the API \
             key is wrong / expired."
        ),
        RealtimeError::ChannelRejected { reason } => {
            format!("Channel rejected by server: {reason}")
        }
        other => other.to_string(),
    }
}

// ===========================================================================
// Profile CRUD commands (PR-2a)
// ===========================================================================
//
// Each command is a thin shim:
//   1. Build the repo adapter from app+state.
//   2. Invoke a single use case from `realtime::app::profile_use_cases`.
//   3. Map `RealtimeError → String` for the renderer.
//
// The API-key write path lives in `save_realtime_profile`: when the
// caller passes `api_key: Some(_)`, we write it to the namespaced
// SecretStore slot for this profile. `None` means "leave the saved key
// alone" — same convention as the legacy form.

/// Wire shape for save: the input fields PLUS an optional API key. The
/// key is intentionally separate from the profile struct so a future
/// "export my config" feature can serialise the profile without
/// touching the secret.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfileRequest {
    /// `null` = create a new profile. `Some(id)` = update existing.
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub channel: String,
    pub event_name: String,
    pub frame_filter: String,
    /// `null` = leave existing key (if any). Empty string treated as
    /// `null`. Non-empty = write to the namespaced SecretStore key.
    pub api_key: Option<String>,
}

/// List all saved profiles + the active id. Returns whatever the
/// migration would produce on first call — a pre-PR-2a user sees their
/// legacy single record imported as one profile named "default".
#[tauri::command]
pub async fn list_realtime_profiles<R: Runtime>(
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<RealtimeSettings, String> {
    let repo = AppConfigProfileRepository::new(app, Arc::clone(&state.app_config));
    ListProfilesUseCase::new(&repo)
        .execute()
        .await
        .map_err(|e| e.to_string())
}

/// Create or update a profile. If `api_key` is `Some(non-empty)`, write
/// it to the namespaced SecretStore slot — the profile's id (newly
/// generated or existing) is used to derive the key name. Returns the
/// saved profile so the renderer sees the canonical shape.
#[tauri::command]
pub async fn save_realtime_profile<R: Runtime>(
    request: SaveProfileRequest,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<RealtimeProfile, String> {
    let SaveProfileRequest {
        id,
        name,
        url,
        channel,
        event_name,
        frame_filter,
        api_key,
    } = request;

    let repo = AppConfigProfileRepository::new(app.clone(), Arc::clone(&state.app_config));
    let input = SaveProfileInput {
        id: id.map(ProfileId::from_str),
        name,
        url,
        channel,
        event_name,
        frame_filter,
    };
    let saved = SaveProfileUseCase::new(&repo)
        .execute(input)
        .await
        .map_err(|e| e.to_string())?;

    // Write the API key if the caller supplied one. Done AFTER the
    // profile save so we know the id is real (especially for new
    // profiles where the id is generated by the use case). On macOS
    // this writes to:
    //   ~/Library/Application Support/io.refactor-labs.drift-lab/secrets.json
    // …under the key `supabase_realtime_api_key:<profile-id>`. The
    // log line below confirms the slot for diagnostics; it never
    // prints the value.
    if let Some(key) = api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            let store = FileSecretStore::new(app);
            let key_name = namespaced_realtime_api_key_for(&saved.id);
            store
                .set(&key_name, trimmed)
                .map_err(|e| format!("write profile secret: {e}"))?;
        }
    }
    Ok(saved)
}

/// Delete a profile (and its namespaced API key). Idempotent —
/// returns `false` when no profile by that id exists. If the deleted
/// profile was active, the active pointer is cleared (no auto-promote).
#[tauri::command]
pub async fn delete_realtime_profile<R: Runtime>(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<bool, String> {
    let repo = AppConfigProfileRepository::new(app, Arc::clone(&state.app_config));
    DeleteProfileUseCase::new(&repo)
        .execute(&ProfileId::from_str(id))
        .await
        .map_err(|e| e.to_string())
}

/// Set / clear the active profile. Passing `null` clears the pointer
/// (LiveScan will then refuse to Start until the user picks one).
#[tauri::command]
pub async fn activate_realtime_profile<R: Runtime>(
    id: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    let repo = AppConfigProfileRepository::new(app, Arc::clone(&state.app_config));
    ActivateProfileUseCase::new(&repo)
        .execute(id.map(ProfileId::from_str))
        .await
        .map_err(|e| e.to_string())
}
