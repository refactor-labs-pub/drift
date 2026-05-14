//! Static-scan integration layer.
//!
//! Glue between the `drift_static_profiler` crate and the desktop UI:
//!   - [`runner`] — orchestrates the two-step scan (discover roots → user
//!     picks → focused analysis), spawning blocking work on Tokio's pool.
//!   - [`progress_sink`] — `drift_static_profiler::Progress` impl that
//!     forwards every pipeline callback to the UI as a Tauri event.
//!   - [`storage`] — read/write of `~/.drift/scans/<scan_id>.json` reports.
//!   - [`suggester`] — per-finding LLM driver that calls
//!     `read_file_lines` and asks for one concrete code suggestion.
//!   - [`types`] — wire types crossing the IPC boundary (`ScanProgress`,
//!     `ScanPickerRoot`, `ScanSuggestion`, …).

pub mod progress_sink;
pub mod runner;
pub mod storage;
pub mod suggester;
pub mod types;
