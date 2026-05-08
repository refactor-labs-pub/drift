use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
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

pub mod topic {
    pub const STEP: &str = "run://step";
    pub const COMPLETE: &str = "run://complete";
    pub const ERROR: &str = "run://error";

    pub const BACKEND_STATUS: &str = "backend:status";
}

/// Coarse lifecycle of the LLM backend, broadcast as `backend:status` events.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BackendStatus {
    /// No persisted config — the welcome path.
    Unconfigured,
    /// Config persisted but the runtime (client / llama-server) hasn't been resolved yet.
    Idle { mode: String, model: String },
    /// Pulling a GGUF from HuggingFace.
    Downloading { file: String },
    /// llama-server spawned, waiting for the OpenAI-compatible endpoint to come up.
    Starting,
    /// Ready to take chat requests.
    Ready { mode: String, model: String },
    /// Last resolve attempt failed.
    Error { message: String },
}
