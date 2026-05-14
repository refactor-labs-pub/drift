//! Wire types for the static-scan flow.
//!
//! These types cross the Tauri IPC boundary, so the field shapes are part of
//! the public contract with the frontend. Keep the `serde` representation
//! aligned with `desktop-ui/src/lib/scanApi.ts`.
//!
//! Design split — three families:
//!   - **progress** events: streamed while the scan is running.
//!   - **picker** payload: top-N entry roots the user chooses between.
//!   - **summary** payload: the final `Report` view the UI renders.
//!
//! The picker rows are decorated for display only (file path relative to the
//! scanned root, first few callers resolved by name) so the UI never has to
//! reach back into `CallGraph` internals.

use serde::Serialize;

/// One streamed progress event. The frontend dispatches on `kind` to pick an
/// icon and to update the right phase row. Shapes mirror the
/// `drift_static_profiler::Progress` trait so we can forward each callback
/// without lossy translation.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanProgress {
    /// Filesystem walk has begun. No total yet — UI shows a spinner.
    WalkStart {
        scan_id: String,
    },
    /// Periodic walk tick — running count of files discovered so far.
    WalkProgress {
        scan_id: String,
        files_seen: u64,
    },
    /// Walk finished. `bytes` is the combined size of every file walked.
    WalkEnd {
        scan_id: String,
        total_files: u64,
        bytes: u64,
    },
    /// Parse phase starting — `total` source files queued for tree-sitter.
    ParseStart {
        scan_id: String,
        total_source_files: u64,
    },
    /// One source file finished parsing.
    ParseProgress {
        scan_id: String,
        done: u64,
        total: u64,
        current: Option<String>,
    },
    /// Atomic post-parse phase started (graph build, collect entries, …).
    Phase {
        scan_id: String,
        name: String,
    },
    /// Counted post-parse phase started.
    StepStart {
        scan_id: String,
        label: String,
        total: u64,
    },
    /// Counted post-parse phase progress.
    StepProgress {
        scan_id: String,
        label: String,
        done: u64,
        total: u64,
        current: Option<String>,
    },
}

/// One row in the top-N entry-root picker. Same fields as
/// `drift_static_profiler::PickerRoot`, mapped to camelCase via `serde`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPickerRoot {
    pub index: usize,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub reach: usize,
    pub callers: Vec<ScanPickerCaller>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPickerCaller {
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// Emitted once root discovery completes and the user has to choose. The
/// scan is parked until [`crate::scan::runner::select_entry`] is called.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanEntriesReady {
    pub scan_id: String,
    pub roots: Vec<ScanPickerRoot>,
}

/// Emitted once the final Report has been serialised to disk. The UI fetches
/// the report body via `load_scan(scan_id)` and renders the summary cards.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanComplete {
    pub scan_id: String,
    /// Absolute path to the saved JSON under `~/.drift/scans/`.
    pub saved_path: String,
    /// Display label for the picked root, so the UI can show "scan complete
    /// for `<name>`" without rehydrating the whole picker payload.
    pub picked_root: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub scan_id: String,
    pub message: String,
}

/// One streamed LLM suggestion. The suggester runs sequentially over the
/// findings, emits one of these per finding so the UI can render a growing
/// list as the model produces answers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSuggestion {
    pub scan_id: String,
    /// Stable index in the merged finding list (findings_top → immediate_fixes
    /// → refactor_candidates) so the UI can render them in order.
    pub index: usize,
    /// One of "finding", "immediate_fix", "refactor_candidate".
    pub source: &'static str,
    /// "kind" from the underlying finding (e.g. n_plus_one, hot_zone).
    pub kind: String,
    pub severity: String,
    pub file: String,
    pub line: usize,
    pub name: String,
    /// Suggestion body — formatted as the model returned it, expected to be
    /// markdown with one fenced code block plus a one-line rationale.
    pub suggestion: String,
}

/// Emitted when the suggestion phase finishes — total count + how many failed
/// (so the UI can show "12 of 14 succeeded").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSuggestionDone {
    pub scan_id: String,
    pub total: usize,
    pub failed: usize,
}

pub mod topic {
    /// Streamed progress event (one per pipeline callback).
    pub const PROGRESS: &str = "scan://progress";
    /// Picker payload — UI prompts the user to choose a root.
    pub const ENTRIES_READY: &str = "scan://entries-ready";
    /// Final summary saved to disk.
    pub const COMPLETE: &str = "scan://complete";
    pub const ERROR: &str = "scan://error";

    /// One suggestion-per-finding stream from the LLM driver.
    pub const SUGGESTION: &str = "scan://suggestion";
    pub const SUGGESTION_DONE: &str = "scan://suggestion-done";
}
