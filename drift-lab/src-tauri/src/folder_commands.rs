//! Tauri commands for the folder registry.
//!
//! Thin shims over [`crate::folder`] — translate IPC arg shapes,
//! map errors to `Result<_, String>`. No business logic here.

use std::path::PathBuf;

use crate::folder::{self, FolderFingerprint, ScannedFolder};

/// List every folder that has either a static scan or registered
/// placeholder under `~/.drift/scans/`. Used by the desktop UI to
/// populate the "pick a folder" step that gates both Active Scan and
/// (eventually) Static Scan.
#[tauri::command]
pub async fn list_scanned_folders() -> Result<Vec<ScannedFolder>, String> {
    folder::list_scanned_folders()
}

/// Register a folder by path so it appears in
/// `list_scanned_folders()` even before any static scan has run.
/// Returns the canonical [`FolderFingerprint`] the UI then uses to
/// kick off scans against this folder.
///
/// Errors if the path doesn't exist or isn't a directory — the user
/// should pick something real, not a typo.
#[tauri::command]
pub async fn register_folder(path: String) -> Result<FolderFingerprint, String> {
    folder::register(&PathBuf::from(path))
}

/// Whether a folder has at least one saved static scan. The Active
/// Scan page calls this before enabling its "Start" button — we
/// refuse to start a live session against a folder that's never been
/// statically scanned, so the two paths stay correlated.
#[tauri::command]
pub async fn folder_has_static_scan(fingerprint: String) -> Result<bool, String> {
    let fp = FolderFingerprint::parse(&fingerprint)
        .ok_or_else(|| format!("invalid fingerprint shape: {fingerprint}"))?;
    Ok(folder::has_static_scan(&fp))
}
