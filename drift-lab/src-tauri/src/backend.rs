//! Resolve a [`ModelBackend`] config into a uniform `ResolvedBackend`.
//!
//! Both API and Local mode produce the same `rig` OpenAI client — Local mode
//! spawns `llama-server -hf <spec>` (the binary handles the GGUF download
//! itself; first boot is slow, subsequent boots hit the cache).

use anyhow::{Context, Result};
use rig::providers::openai;
use tauri::{AppHandle, Runtime};
use tokio::process::Child;

use crate::{local_server, model_config::ModelBackend};

pub struct ResolvedBackend {
    pub client: openai::Client,
    pub model: String,
    /// Held so the subprocess is killed when the backend is dropped/replaced.
    _server: Option<Child>,
}

pub async fn resolve<R: Runtime>(
    backend: ModelBackend,
    app: &AppHandle<R>,
) -> Result<ResolvedBackend> {
    match backend {
        ModelBackend::Api {
            base_url,
            api_key,
            model,
        } => {
            let client = openai::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()
                .context("building OpenAI client")?;
            Ok(ResolvedBackend {
                client,
                model,
                _server: None,
            })
        }
        ModelBackend::Local { spec, port } => {
            let _ = app; // currently unused for local; reserved for future progress events
            let server = local_server::spawn_llama_server(&spec, port).await?;

            let base = format!("http://127.0.0.1:{port}/v1");
            let client = openai::Client::builder()
                .api_key("not-needed".to_string())
                .base_url(base)
                .build()
                .context("building local OpenAI-compatible client")?;

            // llama-server uses the spec as the model identifier; chat/completions
            // requests with `model: spec` route to the loaded GGUF.
            Ok(ResolvedBackend {
                client,
                model: spec,
                _server: Some(server),
            })
        }
    }
}
