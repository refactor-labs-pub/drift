//! `FolderFingerprint` — a stable, filename-safe id derived from an
//! absolute folder path.
//!
//! Use case: the desktop app wants to scope both static scans and live
//! realtime event logs under one directory per project, e.g.
//! `~/.drift/scans/<fingerprint>/...`. Random UUIDs would re-roll on
//! every "scan this folder again", which would scatter related output.
//! Hashing the canonicalised absolute path gives a deterministic id
//! that survives repeated invocations on the same folder, and naturally
//! groups related artefacts.
//!
//! ## Algorithm
//! SHA-256 over `canonicalize(path).to_string_lossy().as_bytes()`,
//! truncated to the first 16 hex chars (8 bytes). That's 2^64 of
//! collision space — orders of magnitude more than the number of
//! folders any human will ever scan — and short enough to fit in a
//! readable directory name. Filename-safe by construction (`[0-9a-f]`).
//!
//! `canonicalize` resolves symlinks and removes `..` segments, so two
//! paths that point at the same physical folder produce the same
//! fingerprint. If canonicalize fails (path doesn't exist yet), we
//! hash the raw string — best-effort identity that still works for
//! "register folder before first scan" flows.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable id for a folder. Always 16 lowercase hex chars.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FolderFingerprint(String);

impl FolderFingerprint {
    /// Compute the fingerprint of a path. Falls back to hashing the raw
    /// string when `canonicalize` fails (path doesn't exist) so this
    /// never panics — callers that want a strict check should call
    /// `Path::exists()` themselves first.
    pub fn from_path(p: &Path) -> Self {
        let canonical = p
            .canonicalize()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string_lossy().into_owned());
        Self::from_canonical_string(&canonical)
    }

    /// Compute the fingerprint of an already-canonicalised string.
    /// Useful when reading paths off `.meta.json` files — the path is
    /// stored verbatim and we shouldn't re-resolve it (it may no
    /// longer exist on disk).
    pub fn from_canonical_string(s: &str) -> Self {
        let digest = Sha256::digest(s.as_bytes());
        let hex = hex::encode(&digest[..8]); // first 8 bytes → 16 hex chars
        Self(hex)
    }

    /// Construct from an already-known fingerprint string (e.g. read
    /// off the wire from the renderer). Validates shape; returns
    /// `None` if the string isn't 16 lowercase hex chars.
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != 16 {
            return None;
        }
        if !s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            return None;
        }
        Some(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FolderFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_across_calls() {
        let a = FolderFingerprint::from_canonical_string("/foo/bar");
        let b = FolderFingerprint::from_canonical_string("/foo/bar");
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_differs_for_different_paths() {
        let a = FolderFingerprint::from_canonical_string("/foo/bar");
        let b = FolderFingerprint::from_canonical_string("/foo/baz");
        assert_ne!(a, b);
    }

    #[test]
    fn fingerprint_is_16_hex_chars() {
        let fp = FolderFingerprint::from_canonical_string("/anything");
        assert_eq!(fp.as_str().len(), 16);
        assert!(fp.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_accepts_valid_hex() {
        let fp = FolderFingerprint::from_canonical_string("/x").to_string();
        assert!(FolderFingerprint::parse(&fp).is_some());
    }

    #[test]
    fn parse_rejects_wrong_length() {
        assert!(FolderFingerprint::parse("deadbeef").is_none());
        assert!(FolderFingerprint::parse(&"f".repeat(17)).is_none());
    }

    #[test]
    fn parse_rejects_non_hex() {
        assert!(FolderFingerprint::parse("ABCDEFGHIJKLMNOP").is_none());
        assert!(FolderFingerprint::parse("0123456789abcdez").is_none());
    }
}
