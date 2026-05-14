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

use crate::agent::OpenAiProvider;
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
    runner::start_scan(
        app,
        scan_id.clone(),
        path,
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

/// Kick off the per-finding suggestion phase against a saved scan. The
/// command returns immediately; suggestions stream over `scan://suggestion`
/// and the run terminates with `scan://suggestion-done`.
#[tauri::command]
pub async fn start_scan_suggestions<R: Runtime>(
    scan_id: String,
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
    suggester::start_suggestions(app, scan_id, provider);
    Ok(())
}

fn build_provider(
    config: ModelBackend,
) -> anyhow::Result<Arc<dyn crate::agent::provider::Provider>> {
    let ModelBackend::Api {
        base_url,
        api_key,
        model,
    } = config;
    Ok(Arc::new(OpenAiProvider::new(base_url, api_key, model)))
}
