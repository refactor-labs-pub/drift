//! Spawn `llama-server` as a child process. We pass `-hf <spec>` and let the
//! binary handle the GGUF download itself — no separate hf-hub layer needed.
//! The returned `Child` is `kill_on_drop(true)` so it dies with the Tauri app.
//!
//! Prereq: `llama-server` must be on PATH (e.g. `brew install llama.cpp`)
//! until Phase 6 swaps it for a bundled sidecar.

use std::time::Duration;

use anyhow::{bail, Result};
use tokio::process::{Child, Command};
use tokio::time::sleep;

/// Pre-flight check: returns the `llama-server --version` string, or an
/// actionable install hint if the binary isn't on PATH. Exposed as a Tauri
/// command so the UI can fail-fast before attempting to download a model.
#[tauri::command]
pub async fn check_llama_server() -> Result<String, String> {
    match Command::new("llama-server").arg("--version").output().await {
        Ok(out) => {
            // `--version` writes to stderr on some builds, stdout on others.
            // Combine both, take the first non-empty line.
            let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
            if combined.trim().is_empty() {
                combined = String::from_utf8_lossy(&out.stderr).to_string();
            }
            let line = combined
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("llama-server")
                .trim()
                .to_string();
            Ok(line)
        }
        Err(e) => Err(format!(
            "llama-server is not on PATH ({e}). \
             Install it with `brew install llama.cpp` (macOS) or download a \
             prebuilt release from https://github.com/ggml-org/llama.cpp/releases."
        )),
    }
}

/// Spawn llama-server with `-hf <spec>` so the binary fetches the GGUF on
/// first run (or hits its cache on subsequent runs). The readiness loop is
/// long (10 min) because the very first run also performs the download.
pub async fn spawn_llama_server(spec: &str, port: u16) -> Result<Child> {
    let child = Command::new("llama-server")
        .arg("-hf")
        .arg(spec)
        .arg("--port")
        .arg(port.to_string())
        .arg("--host")
        .arg("127.0.0.1")
        .arg("-c")
        .arg("4096")
        .arg("--jinja")
        .kill_on_drop(true)
        .spawn()?;

    wait_for_ready(port).await?;
    Ok(child)
}

async fn wait_for_ready(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::Client::new();
    // 1200 × 500ms = 10 min — covers first-run downloads of multi-GB models.
    for i in 0..1200 {
        sleep(Duration::from_millis(500)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if i % 20 == 0 && i > 0 {
            tracing::info!("waiting for llama-server on :{port} (attempt {i})");
        }
    }
    bail!("llama-server didn't become ready within 10 minutes on port {port}")
}
