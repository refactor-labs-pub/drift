//! Append-mode JSONL [`EventSink`] — one event per line under
//! `~/.drift/event_logs/realtime-<stamp>.jsonl`. The downstream file-tail
//! aggregator (in `event_log_commands`) polls the same file at ~1 Hz and
//! re-emits aggregates on `event_log://aggregate`. Reusing one canonical
//! aggregator (instead of a separate "live realtime" aggregator) means
//! the file-load path and the realtime-stream path can never diverge on
//! edge cases.
//!
//! ## File semantics
//! * Created (touch) at `allocate_path` time so the tailer doesn't ENOENT
//!   before the first broadcast.
//! * Appended-to only; one event = one line, newline-terminated.
//! * Closed when the [`EventSink`] is dropped (`tokio::fs::File`
//!   semantics).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;

use crate::event_log;
use crate::realtime::domain::RealtimeError;
use crate::realtime::ports::{EventSink, EventSinkFactory};

/// Factory that allocates JSONL paths under a specific directory.
///
/// Two construction shapes:
/// * [`Self::for_folder`] — scoped to one project. Writes under
///   `~/.drift/scans/<folder-fingerprint>/event_logs/`. The Active
///   Scan flow always uses this — every stream is bound to a folder.
/// * [`Self::legacy_flat`] — the old shared `~/.drift/event_logs/`
///   directory. Kept for back-compat reads of pre-folder runs; no
///   write path uses it any more.
#[derive(Debug, Clone)]
pub struct JsonlSinkFactory {
    dir: PathBuf,
}

impl JsonlSinkFactory {
    /// Scope writes to a specific folder's per-project log dir.
    pub fn for_folder(
        fingerprint: &crate::folder::FolderFingerprint,
    ) -> Result<Self, RealtimeError> {
        let dir = crate::folder::event_logs_dir_for(fingerprint).map_err(RealtimeError::Io)?;
        Ok(Self { dir })
    }

    /// Pre-PR legacy `~/.drift/event_logs/` location. New code should
    /// use [`Self::for_folder`]; this stays only so legacy log files
    /// keep being readable from the file-load path.
    #[allow(dead_code)]
    pub fn legacy_flat() -> Result<Self, RealtimeError> {
        let dir = event_log::default_logs_dir().ok_or_else(|| {
            RealtimeError::Io("cannot resolve ~/.drift/event_logs (HOME not set?)".into())
        })?;
        Ok(Self { dir })
    }
}

#[async_trait]
impl EventSinkFactory for JsonlSinkFactory {
    async fn open(&self, path: &Path) -> Result<Box<dyn EventSink>, RealtimeError> {
        // Make sure the parent dir exists. `allocate_path` already does
        // this for paths it produces, but the trait permits callers to
        // hand us any path.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                RealtimeError::Io(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| RealtimeError::Io(format!("open {}: {e}", path.display())))?;
        Ok(Box::new(JsonlSink { file }))
    }

    fn allocate_path(&self) -> Result<PathBuf, RealtimeError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| RealtimeError::Io(format!("mkdir {}: {e}", self.dir.display())))?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        Ok(self.dir.join(format!("realtime-{stamp}.jsonl")))
    }
}

struct JsonlSink {
    file: File,
}

#[async_trait]
impl EventSink for JsonlSink {
    async fn append(&mut self, payload: &Value) -> Result<(), RealtimeError> {
        // Serialise into the smallest envelope the aggregator understands
        // (one object per line). Errors here are real I/O — propagate.
        let line = serde_json::to_string(payload)
            .map_err(|e| RealtimeError::Io(format!("serialise payload: {e}")))?;
        self.file
            .write_all(line.as_bytes())
            .await
            .map_err(|e| RealtimeError::Io(format!("write event: {e}")))?;
        if !line.ends_with('\n') {
            self.file
                .write_all(b"\n")
                .await
                .map_err(|e| RealtimeError::Io(format!("write newline: {e}")))?;
        }
        Ok(())
    }
}
