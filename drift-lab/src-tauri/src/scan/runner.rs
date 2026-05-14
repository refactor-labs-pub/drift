//! Two-step static-scan orchestrator.
//!
//! Flow:
//!   1. UI calls `start_scan(project_path)` → we spawn a blocking task that
//!      runs `analyze_picked_with_progress`. While discovery is in flight,
//!      progress events stream over `scan://progress`.
//!   2. When discovery hits the picker step, the closure emits
//!      `scan://entries-ready` and **parks** on a per-scan oneshot. The UI
//!      shows the user the top-N roots.
//!   3. UI calls `select_entry(scan_id, root_index)`. We send through the
//!      oneshot — the blocking task wakes, builds the focused call tree,
//!      assembles the Report, persists it to `~/.drift/scans/<id>.json`,
//!      and emits `scan://complete`.
//!
//! The `PickerRegistry` is the single owner of the per-scan decision senders.
//! Holding it in `AppState` keeps the blocking analysis task decoupled from
//! Tauri commands — the runner doesn't know about commands, the commands
//! don't know about `analyze_picked_with_progress`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{Context, Result};
use drift_static_profiler::{
    analyze_picked_with_progress, AnalyzeOptions, AnalyzeOutcome, DiscoverOpts,
};
use tauri::{AppHandle, Emitter, Runtime};

use super::progress_sink::TauriProgressSink;
use super::storage;
use super::types::{
    topic, ScanComplete, ScanEntriesReady, ScanError, ScanPickerCaller, ScanPickerRoot,
};

/// Maximum number of root entries we surface in the picker. The user
/// specified "top 10".
const PICKER_LIMIT: usize = 10;

/// Per-scan handshake state. Holds the channel the analysis closure parks
/// on while the user picks an entry from the UI. One slot per in-flight
/// `scan_id`.
#[derive(Default)]
pub struct PickerRegistry {
    inner: Mutex<HashMap<String, std::sync::mpsc::SyncSender<Option<usize>>>>,
}

impl PickerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn install(&self, scan_id: String) -> std::sync::mpsc::Receiver<Option<usize>> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Option<usize>>(1);
        // Last-writer-wins for the same scan_id is fine — a fresh
        // start_scan with the same id (shouldn't happen since we generate
        // UUIDs) would just shadow the previous handshake.
        if let Ok(mut g) = self.inner.lock() {
            g.insert(scan_id, tx);
        }
        rx
    }

    fn take(&self, scan_id: &str) -> Option<std::sync::mpsc::SyncSender<Option<usize>>> {
        self.inner.lock().ok()?.remove(scan_id)
    }

    /// Forward the user's decision into the parked analysis closure.
    /// `Some(index)` builds the call tree for that root, `None` aborts.
    pub fn decide(&self, scan_id: &str, choice: Option<usize>) -> Result<()> {
        let tx = self
            .take(scan_id)
            .with_context(|| format!("no pending picker for scan {scan_id}"))?;
        tx.send(choice)
            .context("picker decision channel closed — analysis task exited")?;
        Ok(())
    }
}

/// Kick off a static scan. Returns immediately; results stream over events.
///
/// The blocking `analyze_picked_with_progress` call runs on Tokio's blocking
/// pool so the async runtime stays responsive.
pub fn start_scan<R: Runtime>(
    app: AppHandle<R>,
    scan_id: String,
    project_path: PathBuf,
    registry: Arc<PickerRegistry>,
) {
    let app_for_task = app.clone();
    let scan_id_for_task = scan_id.clone();
    let pick_rx = registry.install(scan_id.clone());

    tauri::async_runtime::spawn_blocking(move || {
        run_blocking(app_for_task, scan_id_for_task, project_path, pick_rx, registry);
    });
}

fn run_blocking<R: Runtime>(
    app: AppHandle<R>,
    scan_id: String,
    project_path: PathBuf,
    pick_rx: std::sync::mpsc::Receiver<Option<usize>>,
    registry: Arc<PickerRegistry>,
) {
    let sink = TauriProgressSink::new(app.clone(), scan_id.clone());
    let discover = DiscoverOpts {
        max_roots: PICKER_LIMIT,
        ..DiscoverOpts::default()
    };
    let opts = AnalyzeOptions::default();

    // `pick_callback` runs synchronously on the blocking task. It emits the
    // picker rows then blocks on the registry channel until the UI sends
    // through `select_entry_and_scan(scan_id, index)`.
    let app_for_picker = app.clone();
    let scan_id_for_picker = scan_id.clone();
    let pick_callback = move |roots: &[drift_static_profiler::PickerRoot]| -> Option<usize> {
        let payload = ScanEntriesReady {
            scan_id: scan_id_for_picker.clone(),
            roots: roots.iter().enumerate().map(decorate).collect(),
        };
        let _ = app_for_picker.emit(topic::ENTRIES_READY, payload);
        // Wait for the user. If the sender is dropped (cancelled), recv
        // returns Err — treat as abort.
        match pick_rx.recv() {
            Ok(choice) => choice,
            Err(_) => None,
        }
    };

    let result = analyze_picked_with_progress(
        &project_path,
        &discover,
        &opts,
        &sink,
        pick_callback,
    );

    // Ensure the registry slot is freed even if the closure short-circuited
    // before recv (e.g. discovery returned no roots).
    let _ = registry.take(&scan_id);

    match result {
        Ok(Some(outcome)) => finalize(app, scan_id, outcome),
        Ok(None) => {
            // User cancelled or no roots discovered. Emit a friendly error
            // so the UI can reset its picker state.
            let _ = app.emit(
                topic::ERROR,
                ScanError {
                    scan_id,
                    message: "scan cancelled — no entry selected or no roots discovered"
                        .into(),
                },
            );
        }
        Err(e) => {
            let _ = app.emit(
                topic::ERROR,
                ScanError {
                    scan_id,
                    message: format!("{e:#}"),
                },
            );
        }
    }
}

fn finalize<R: Runtime>(app: AppHandle<R>, scan_id: String, outcome: AnalyzeOutcome) {
    let picked_root = outcome
        .report
        .entries
        .first()
        .map(|e| e.name.clone());

    match storage::save_report(&scan_id, &outcome.report) {
        Ok(path) => {
            let _ = app.emit(
                topic::COMPLETE,
                ScanComplete {
                    scan_id,
                    saved_path: path.display().to_string(),
                    picked_root,
                },
            );
        }
        Err(e) => {
            let _ = app.emit(
                topic::ERROR,
                ScanError {
                    scan_id,
                    message: format!("saving scan: {e:#}"),
                },
            );
        }
    }
}

fn decorate((index, r): (usize, &drift_static_profiler::PickerRoot)) -> ScanPickerRoot {
    ScanPickerRoot {
        index,
        name: r.name.clone(),
        file: r.file.clone(),
        line: r.line,
        reach: r.reach,
        callers: r
            .callers
            .iter()
            .map(|c| ScanPickerCaller {
                name: c.name.clone(),
                file: c.file.clone(),
                line: c.line,
            })
            .collect(),
    }
}
