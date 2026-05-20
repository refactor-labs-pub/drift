//! Folder identity + registry.
//!
//! Each project/folder the user scans gets a stable
//! [`FolderFingerprint`] derived from its absolute path. Both static
//! scans and active realtime sessions are stored under
//! `~/.drift/scans/<fingerprint>/` so the two are colocated and easy
//! to join later.
//!
//! ## Why a derived fingerprint, not a UUID
//! UUIDs would re-roll on every "scan this folder again", scattering
//! related artefacts across different ids. The hash-of-path approach
//! gives the same id for repeated scans of the same folder — exactly
//! the grouping behaviour the UI needs.
//!
//! ## Existing data
//! Static scans already write `~/.drift/scans/<scan-uuid>.meta.json`
//! with a `sourceRoot` field. [`registry::list_scanned_folders`] derives
//! the folder list by grouping those metas — no separate manifest to
//! keep in sync.

pub mod fingerprint;
pub mod registry;

pub use fingerprint::FolderFingerprint;
pub use registry::{
    event_logs_dir_for, find, has_static_scan, list_scanned_folders, register, ScannedFolder,
};
