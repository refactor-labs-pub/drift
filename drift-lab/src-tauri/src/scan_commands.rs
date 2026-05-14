//! Tauri commands that bridge the desktop UI to `crate::scan`.
//!
//! Split out of `commands.rs` so the static-scan lifecycle reads as one
//! concise unit. Each command is a thin shim — it parses Tauri args, calls
//! into `scan::runner` / `scan::storage` / `scan::suggester`, and returns.
//! No business logic lives here.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use uuid::Uuid;

use crate::model_config::ModelBackend;
use crate::scan::{runner, storage, suggester, types::ScanPickerRoot};
use crate::state::AppState;

/// Kick off a static scan and return its `scan_id` immediately. Progress
/// events stream over `scan://progress`; the picker fires
/// `scan://entries-ready` and parks until [`select_entry_and_scan`] is
/// called.
#[tauri::command]
pub async fn start_static_scan<R: Runtime>(
    project_path: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<String, String> {
    let path = PathBuf::from(&project_path);
    if !path.is_dir() {
        return Err(format!("not a directory: {project_path}"));
    }
    let scan_id = Uuid::new_v4().to_string();
    // Snapshot the user's scan-filter preferences at scan kick-off — the
    // settings UI can be opened/changed mid-scan without affecting the run
    // already in flight. Settings UI users see "next scan will use new
    // filters" semantics, which matches how the wire contract is documented.
    let filters = state.app_config.lock().await.scan_filters;
    runner::start_scan(
        app,
        scan_id.clone(),
        path,
        filters,
        Arc::clone(&state.scan_pickers),
    );
    Ok(scan_id)
}

/// Deliver the user's picker choice. `root_index` is the row index from the
/// `ScanEntriesReady` payload (or `None` to cancel the scan cleanly).
#[tauri::command]
pub async fn select_entry_and_scan(
    scan_id: String,
    root_index: Option<usize>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .scan_pickers
        .decide(&scan_id, root_index)
        .map_err(|e| format!("{e:#}"))
}

/// List every saved scan under `~/.drift/scans/`. Sorted by saved_at desc.
#[tauri::command]
pub async fn list_static_scans() -> Result<Vec<storage::ScanMeta>, String> {
    storage::list_scans().map_err(|e| format!("{e:#}"))
}

/// Return a previously-saved scan envelope (scan_id + saved_at + Report).
#[tauri::command]
pub async fn load_static_scan(scan_id: String) -> Result<storage::StoredScan, String> {
    storage::load_envelope(&scan_id).map_err(|e| format!("{e:#}"))
}

/// Return only the picker-style root list from a previously-saved scan —
/// useful when the UI wants to re-render the entry picker without parsing
/// the whole call-tree payload.
#[tauri::command]
pub async fn list_scan_entries(scan_id: String) -> Result<Vec<ScanPickerRoot>, String> {
    let env = storage::load_envelope(&scan_id).map_err(|e| format!("{e:#}"))?;
    Ok(env
        .report
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| ScanPickerRoot {
            index: i,
            name: e.name.clone(),
            file: e.file.clone(),
            line: e.line,
            reach: e.subtree_size,
            callers: e
                .callers
                .iter()
                .map(|c| crate::scan::types::ScanPickerCaller {
                    name: c.name.clone(),
                    file: c.file.clone(),
                    line: c.line,
                })
                .collect(),
        })
        .collect())
}

/// Return the canonical ranked + deduped finding list for a saved scan.
/// The frontend renders one "Study this" row per item; the `index` is the
/// key the UI passes back to [`start_scan_finding_suggestion`].
///
/// Why we expose this from Rust instead of having the UI compute it: the
/// suggester applies a specific dedupe (file, line, kind) and a hard cap on
/// the count. Keeping that policy in one place means the index the UI hands
/// us always matches the row the suggester will operate on.
#[tauri::command]
pub async fn list_scan_findings(scan_id: String) -> Result<Vec<suggester::FindingItem>, String> {
    let env = storage::load_envelope(&scan_id).map_err(|e| format!("{e:#}"))?;
    Ok(suggester::collect_findings(&env.report))
}

/// Kick off the LLM suggestion run for ONE finding in a saved scan. The
/// command returns immediately; the suggestion streams over
/// `scan://suggestion-{start,delta}` and terminates with
/// `scan://suggestion-done`.
///
/// **Idempotent per (scan_id, index)**: if a driver is already running for
/// this specific finding, we return `Ok(())` without spawning a duplicate
/// task. The user can still click Study This on a *different* finding while
/// another stream is in flight — each gets its own cancel token.
#[tauri::command]
pub async fn start_scan_finding_suggestion<R: Runtime>(
    scan_id: String,
    index: usize,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    let config = state
        .config
        .lock()
        .await
        .clone()
        .ok_or_else(|| "backend not configured".to_string())?;
    let provider = build_provider(config).map_err(|e| format!("{e:#}"))?;
    let Some(cancel) = state
        .scan_suggestions
        .register_if_absent(&scan_id, index)
    else {
        return Ok(());
    };
    suggester::start_finding_suggestion(
        app,
        scan_id,
        index,
        provider,
        cancel,
        Arc::clone(&state.scan_suggestions),
    );
    Ok(())
}

/// Stop the in-flight suggestion driver for `(scan_id, index)`. Idempotent
/// — calling for a finding with no live session is a silent no-op
/// (returns `false`).
///
/// Mechanism: trigger the `CancellationToken` in the registry. The driver's
/// `tokio::select!` on `cancel.cancelled()` fires immediately, dropping the
/// provider stream future, which drops the underlying HTTP connection. The
/// driver finalizes the row (emits `scan://suggestion` so the UI clears
/// `isStreaming`) and emits `scan://suggestion-done`.
#[tauri::command]
pub async fn stop_scan_finding_suggestion(
    scan_id: String,
    index: usize,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.scan_suggestions.cancel(&scan_id, index))
}

fn build_provider(
    config: ModelBackend,
) -> anyhow::Result<Arc<dyn crate::agent::provider::Provider>> {
    Ok(crate::agent::make_provider(config))
}
