use serde::Serialize;

use crate::tools::analyze_samples::Issue;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    // Part of the StepStatus wire contract consumed by the frontend; the backend
    // never emits Pending (steps start as Active), but the variant must exist so
    // the serde representation stays in sync with desktop-ui's StepStatus type.
    #[allow(dead_code)]
    Pending,
    Active,
    Done,
    Error,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StepUpdate {
    pub run_id: String,
    pub index: usize,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunComplete {
    pub run_id: String,
    pub issues_found: u32,
    pub critical_count: u32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunError {
    pub run_id: String,
    pub message: String,
}

/// One snapshot of container telemetry. Sampled at ~500ms cadence by the
/// `telemetry` module while a scan is running. Counters (`net_*_bytes`,
/// `block_*_bytes`) are the cumulative values reported by `docker stats`;
/// the UI derives bytes/sec by diffing successive samples.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySample {
    pub run_id: String,
    pub ts_ms: i64,
    pub container_id: String,
    pub cpu_pct: f32,
    pub mem_mb: f32,
    pub mem_pct: f32,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
}

/// Final "visibility map" delivered at the end of a successful scan. Built
/// from the issues `analyze_samples` ranked, plus a synthesis turn from the
/// LLM for the architecture advice.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityMap {
    /// Up to 3 critical-severity issues (sorted by self-time desc).
    pub critical: Vec<Issue>,
    /// Up to 10 high/medium-severity issues.
    pub warnings: Vec<Issue>,
    /// Heuristic upper bound: sum of critical issues' self-pct, capped at 50.
    pub estimated_cpu_reduction_pct: f32,
    /// 3-5 short architectural recommendations from the LLM. Falls back to a
    /// deterministic per-category list if the synthesis turn fails.
    pub architecture_advice: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub run_id: String,
    pub map: VisibilityMap,
}

/// One formatted line out of the backend's `tracing` pipeline. A custom
/// `tracing_subscriber::Layer` produces these so the UI can render the same
/// stream that lands on stderr (`drift::*`, `drift_lab_lib::*` targets).
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts_ms: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Agent has called `ask_user` and is parked waiting on a reply. The UI shows
/// a BlockedModal with this `question`; the user's answer flows back through
/// the `answer_blocked_question` Tauri command.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BlockedQuestion {
    pub id: String,
    pub question: String,
}

pub mod topic {
    pub const STEP: &str = "run://step";
    pub const COMPLETE: &str = "run://complete";
    pub const ERROR: &str = "run://error";
    /// Structured "visibility map" emitted just before `COMPLETE` when a run
    /// produced enough data to summarise (i.e. `analyze_samples` succeeded).
    pub const REPORT: &str = "run://report";

    pub const BACKEND_STATUS: &str = "backend:status";

    /// Iterative-agent stream events (see `agent` module). One event per
    /// `AgentEvent` variant — payload is the serialised enum.
    pub const AGENT_EVENT: &str = "agent:event";
    /// Live container-telemetry samples (~2 Hz). Emitted once a tool returns
    /// a `container_id`; stops when the scan ends.
    pub const TELEMETRY: &str = "agent:telemetry";
    /// Formatted tracing log lines mirrored from the backend so the UI can
    /// show what's happening without the user needing a terminal.
    pub const LOG: &str = "agent:log";
    /// Agent has called `ask_user` and is parked waiting on a reply. The UI
    /// renders a BlockedModal with the question text.
    pub const BLOCKED: &str = "agent:blocked";
}

/// Coarse lifecycle of the LLM backend, broadcast as `backend:status` events.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BackendStatus {
    /// No persisted config — the welcome path.
    Unconfigured,
    /// Config persisted but the OpenAI-compatible client hasn't been built yet.
    Idle { mode: String, model: String },
    /// Resolving the client (building HTTP client, validating URL).
    Starting,
    /// Ready to take chat requests.
    Ready { mode: String, model: String },
    /// Last resolve attempt failed.
    Error { message: String },
}
