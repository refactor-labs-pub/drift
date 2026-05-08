//! Configuration for the LLM agent backend.
//!
//! Frontend sends `{ "mode": "api", ... }` or `{ "mode": "local", ... }` to the
//! `configure_backend` Tauri command; serde dispatches into the matching variant.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ModelBackend {
    Api {
        base_url: String,
        api_key: String,
        model: String,
    },
    Local {
        /// `repo_id:quant`, e.g. `unsloth/gemma-3-1b-it-GGUF:Q4_K_M`.
        spec: String,
        port: u16,
    },
}
