//! Tauri commands that bridge the desktop UI to [`crate::event_log`].
//!
//! Shape mirrors `scan_commands.rs`: thin shims that parse args, call into
//! the library function, and surface errors as `Result<_, String>`.
//!
//! ## Live mode
//!
//! `start_live_event_scan` spawns a tokio task that polls the file every
//! [`LIVE_POLL_MS`] ms (cheap: a stat + read of the new tail) and emits a
//! fresh [`AggregateReport`] over [`topic::EVENT_LOG_AGG`]. The simple
//! shape is "re-aggregate every tick" — for the trace volumes a single
//! Python service produces, parsing tens of thousands of lines is far
//! below frame time. A future optimisation would diff against the prior
//! offset and merge incrementally, but the wire shape stays the same.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::event_log::{self, AggregateReport, EventLogMeta};
use crate::state::AppState;

const LIVE_POLL_MS: u64 = 1000;

/// In-flight live-scan registry: live_scan_id → cancellation handle.
///
/// Stored on [`AppState`] under a top-level `Mutex<HashMap<...>>` so the
/// shutdown path can drop every in-flight tail at once.
pub type LiveScans = Arc<Mutex<HashMap<String, CancellationToken>>>;

pub mod topic {
    /// Periodic re-aggregation of an `events.log` being live-tailed.
    pub const EVENT_LOG_AGG: &str = "event_log://aggregate";
    /// One-shot error from a live-tail task. Payload is `LiveErrorPayload`.
    pub const EVENT_LOG_ERR: &str = "event_log://error";
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAggPayload {
    pub live_scan_id: String,
    pub report: AggregateReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveErrorPayload {
    pub live_scan_id: String,
    pub message: String,
}

/// List `.log`/`.jsonl` files in the default `~/.drift/event_logs/` dir,
/// optionally augmented with an explicit `dir` override.
#[tauri::command]
pub async fn list_event_logs(dir: Option<String>) -> Result<Vec<EventLogMeta>, String> {
    let dir = match dir {
        Some(d) => PathBuf::from(d),
        None => match event_log::default_logs_dir() {
            Some(d) => d,
            None => return Ok(vec![]),
        },
    };
    event_log::list_logs(&dir).map_err(|e| format!("{e:#}"))
}

/// One-shot aggregation. Reads the entire file, returns the snakeviz-style
/// report. For very large traces the `calls[]` array is truncated to
/// [`event_log::MAX_RAW_CALLS`]; the aggregates and tree remain exact.
#[tauri::command]
pub async fn aggregate_event_log(path: String) -> Result<AggregateReport, String> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || event_log::aggregate(&path))
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("{e:#}"))
}

/// Phase F4. Read the JSONL file at `path` and convert it into a
/// `profile.schema.json`-shaped document with `mode: "sampled"`.
/// Returns the JSON as a string (the caller saves to disk or pipes
/// into the static-profile viewer). Errors surface as
/// `Result<_, String>` per project convention.
///
/// Output is loadable by the same viewer that renders static profiles:
/// every field name matches `drift-static-profiler/schema/profile.schema.json`.
/// A consumer that has BOTH a static profile and a converted sampled
/// profile for the same codebase can join on `CallTreeNode.id`.
#[tauri::command]
pub async fn export_static_profile_json(path: String) -> Result<String, String> {
    let pb = PathBuf::from(path);
    tokio::task::spawn_blocking(move || crate::event_log_to_profile::convert(&pb))
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("{e:#}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedEventLog {
    /// Absolute path of the saved file on disk.
    pub path: String,
    pub size_bytes: u64,
}

/// Download the JSONL events file from an observability-server's
/// `/events/log` endpoint into `~/.drift/event_logs/<timestamp>.jsonl`
/// and return the saved path. The desktop UI then feeds that path
/// straight into `aggregate_event_log` / `start_live_event_scan` —
/// same flow as a local file pick.
///
/// `url` should normally be `http://<obs-host>:8080/events/log`. The
/// server returns NDJSON with `Content-Disposition: attachment` so the
/// MIME and naming are unambiguous.
///
/// Errors come from three layers — HTTP (network, status), filesystem
/// (cannot create the logs dir or write the file), and bookkeeping
/// (cannot stat the result). All surface as `Result<_, String>` per
/// project convention.
#[tauri::command]
pub async fn download_event_log(url: String) -> Result<DownloadedEventLog, String> {
    // Where to save. We reuse the standard logs dir so the file shows
    // up in `list_event_logs` immediately after download.
    let dir = event_log::default_logs_dir()
        .ok_or_else(|| "cannot resolve ~/.drift/event_logs dir (HOME not set?)".to_string())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let target = dir.join(format!("downloaded-{stamp}.jsonl"));

    // Build a fresh client per call — these downloads are infrequent
    // and we don't want lingering connections in the app's state.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("reqwest build: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("read body from {url}: {e}"))?;

    tokio::fs::write(&target, &bytes)
        .await
        .map_err(|e| format!("write {}: {e}", target.display()))?;

    let size_bytes = bytes.len() as u64;
    Ok(DownloadedEventLog {
        path: target.to_string_lossy().into_owned(),
        size_bytes,
    })
}

/// Start a live-tail aggregator. Returns the `live_scan_id` immediately;
/// fresh `AggregateReport` payloads stream over [`topic::EVENT_LOG_AGG`]
/// at ~1Hz.
#[tauri::command]
pub async fn start_live_event_scan<R: Runtime>(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<String, String> {
    start_live_event_scan_inner(
        PathBuf::from(&path),
        Arc::clone(&state.live_event_scans),
        app,
    )
    .await
}

/// Start a live-tail aggregator without going through the `State` injection
/// layer. Lets other backend modules (the Supabase Realtime subscriber in
/// `event_source_commands.rs`) reuse the file-tail logic without
/// duplicating the polling loop. Same semantics as the Tauri command above.
pub async fn start_live_event_scan_inner<R: Runtime>(
    pb: PathBuf,
    live_event_scans: LiveScans,
    app: AppHandle<R>,
) -> Result<String, String> {
    if !pb.is_file() {
        return Err(format!("not a file: {}", pb.display()));
    }
    let live_scan_id = Uuid::new_v4().to_string();
    let token = CancellationToken::new();
    {
        let mut guard = live_event_scans.lock().await;
        guard.insert(live_scan_id.clone(), token.clone());
    }

    let id_for_task = live_scan_id.clone();
    let app_for_task = app.clone();
    let path_for_task = pb.clone();
    let live_scans = Arc::clone(&live_event_scans);

    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(LIVE_POLL_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let path_for_blocking = path_for_task.clone();
            let res = tokio::task::spawn_blocking(move || {
                event_log::aggregate(&path_for_blocking)
            })
            .await;
            match res {
                Ok(Ok(report)) => {
                    let _ = app_for_task.emit(
                        topic::EVENT_LOG_AGG,
                        LiveAggPayload {
                            live_scan_id: id_for_task.clone(),
                            report,
                        },
                    );
                }
                Ok(Err(e)) => {
                    let _ = app_for_task.emit(
                        topic::EVENT_LOG_ERR,
                        LiveErrorPayload {
                            live_scan_id: id_for_task.clone(),
                            message: format!("{e:#}"),
                        },
                    );
                }
                Err(e) => {
                    let _ = app_for_task.emit(
                        topic::EVENT_LOG_ERR,
                        LiveErrorPayload {
                            live_scan_id: id_for_task.clone(),
                            message: format!("join error: {e}"),
                        },
                    );
                }
            }
        }
        // Drop the registry entry when the task exits — under normal stop()
        // the entry was removed already; under panic we still clean up.
        let mut guard = live_scans.lock().await;
        guard.remove(&id_for_task);
    });

    Ok(live_scan_id)
}

/// Stop a live-tail aggregator. Idempotent — returns `false` if no scan
/// with that id is registered.
#[tauri::command]
pub async fn stop_live_event_scan(
    live_scan_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    stop_live_event_scan_inner(live_scan_id, Arc::clone(&state.live_event_scans)).await
}

/// Stop a live-tail aggregator without going through `State` injection. See
/// `start_live_event_scan_inner` for the rationale.
pub async fn stop_live_event_scan_inner(
    live_scan_id: String,
    live_event_scans: LiveScans,
) -> Result<bool, String> {
    let mut guard = live_event_scans.lock().await;
    let Some(token) = guard.remove(&live_scan_id) else {
        return Ok(false);
    };
    token.cancel();
    Ok(true)
}
