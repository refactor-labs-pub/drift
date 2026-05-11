//! Live model discovery for OpenAI-compatible endpoints.
//!
//! Two related capabilities, both built on the same `GET <base_url>/models`
//! probe (OpenAI shape):
//!
//! 1. [`list_models_from_endpoint`] — single-endpoint probe. Used by the
//!    "Add Provider" form to populate the model dropdown live once a user
//!    types a base URL.
//! 2. [`probe_local_runtimes`] — fans out the probe across the curated set
//!    of local OpenAI-compatible runtimes (Ollama, LM Studio, Docker Model
//!    Runner) in parallel with a hard per-runtime timeout. Used to render
//!    the "Detected local runtimes" picker on the home/settings/onboarding
//!    screens — plug-and-play: whichever runtime the user already has
//!    installed shows up automatically.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{db, presets::PRESETS};

#[derive(Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

/// Hit `<base_url>/models` (OpenAI shape) and return the model ids. Works
/// for OpenAI itself, Ollama, LM Studio, Docker Model Runner, vLLM, etc.
/// Returns `Err` if the endpoint is unreachable or doesn't speak the protocol.
#[tauri::command]
pub async fn list_models_from_endpoint(
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    do_list_endpoint(&base_url, api_key.as_deref(), Duration::from_secs(8))
        .await
        .map_err(|e| e.to_string())
}

async fn do_list_endpoint(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("building http client")?;
    let mut req = client.get(&url).header("user-agent", "drift-lab");
    if let Some(k) = api_key {
        if !k.is_empty() && k != "not-needed" {
            req = req.bearer_auth(k);
        }
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("connecting to {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("{} returned HTTP {}", url, resp.status());
    }
    let list: OpenAiModelList = resp.json().await.context("parsing /v1/models response")?;
    Ok(list.data.into_iter().map(|m| m.id).collect())
}

/// One local runtime detected on the user's machine, with the live model
/// list pulled from its `/v1/models` endpoint at probe time.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRuntime {
    /// Preset id from [`crate::presets::PRESETS`] (e.g. `ollama`, `lm-studio`).
    pub preset_id: String,
    /// Display name from the preset.
    pub name: String,
    /// OpenAI-compatible base URL the runtime is bound to.
    pub base_url: String,
    /// Models the runtime currently has loaded / available.
    pub models: Vec<String>,
    /// Optional UI hint — e.g. set when models were detected via the Docker
    /// CLI but the HTTP endpoint isn't reachable (host-side TCP toggle off).
    /// When present, the UI should disable activation and show this string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Resolve the `docker` binary. macOS GUI apps launched from Finder/Dock
/// inherit a stripped PATH (no `/usr/local/bin`, no `/opt/homebrew/bin`), so
/// `Command::new("docker")` fails for the bundled .app even though `docker`
/// works fine in the user's shell. Fall back to the well-known install
/// locations before giving up.
fn resolve_docker_binary() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    // 1. PATH — fast path for `make dev` and any env where PATH is sane.
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = PathBuf::from(dir).join("docker");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 2. Well-known install locations (macOS + Linux).
    let fallbacks = [
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/Applications/Docker.app/Contents/Resources/bin/docker",
        "/usr/bin/docker",
    ];
    fallbacks
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// Run `docker model list --openai` with a tight timeout and parse the
/// OpenAI-shaped model list. Returns `None` if Docker isn't installed, the
/// daemon is down, or Docker Model Runner isn't enabled — i.e. anything that
/// makes the command fail. Drift-lab doesn't surface those as errors because
/// they're the normal "no DMR here" path.
async fn probe_docker_model_runner_cli() -> Option<Vec<String>> {
    let Some(docker) = resolve_docker_binary() else {
        tracing::debug!("docker binary not found in PATH or known install locations");
        return None;
    };

    let output = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::process::Command::new(&docker)
            .args(["model", "list", "--openai"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        tracing::debug!(
            docker = %docker.display(),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "docker model list --openai exited non-zero"
        );
        return None;
    }

    let body: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let data = body.get("data")?.as_array()?;
    let models: Vec<String> = data
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if !models.is_empty() {
        tracing::info!(
            docker = %docker.display(),
            count = models.len(),
            "docker model runner detected via CLI fallback"
        );
    }
    Some(models)
}

/// Probe every preset that looks like a local OpenAI-compatible runtime —
/// i.e. requires no API key AND its base URL points at loopback. Each probe
/// gets a tight 800ms ceiling so the call returns fast even when nothing is
/// running. Also shells out to `docker model list --openai` to catch Docker
/// Model Runner models even when its host-side TCP endpoint is disabled.
///
/// This is the discovery surface the UI uses to show "Detected local
/// runtimes" — plug-and-play. The user installs Ollama / LM Studio / Docker
/// Desktop with Model Runner, and drift-lab picks it up automatically.
#[tauri::command]
pub async fn probe_local_runtimes() -> Vec<DiscoveredRuntime> {
    let timeout = Duration::from_millis(800);
    let candidates: Vec<_> = PRESETS
        .iter()
        .filter(|p| !p.requires_api_key && is_loopback(p.base_url))
        .collect();

    // HTTP probes in parallel — one slow runtime can't block the rest.
    let http_probes = futures_util::future::join_all(candidates.into_iter().map(|p| async move {
        let result = do_list_endpoint(p.base_url, None, timeout).await;
        match result {
            Ok(models) => Some(DiscoveredRuntime {
                preset_id: p.id.to_string(),
                name: p.name.to_string(),
                base_url: p.base_url.to_string(),
                models,
                note: None,
            }),
            Err(e) => {
                tracing::debug!(
                    preset = p.id,
                    url = p.base_url,
                    error = %e,
                    "runtime probe failed (likely not running)"
                );
                None
            }
        }
    }));

    // Docker CLI fallback runs concurrently with the HTTP probes.
    let (http_results, dmr_cli_models) =
        tokio::join!(http_probes, probe_docker_model_runner_cli());

    let mut detected: Vec<DiscoveredRuntime> = http_results.into_iter().flatten().collect();

    // Merge in DMR-via-CLI: if the HTTP probe already caught it, just union
    // the model lists. Otherwise add a new entry with a "TCP toggle" hint so
    // the user knows why activation isn't immediate.
    if let Some(cli_models) = dmr_cli_models {
        if !cli_models.is_empty() {
            let dmr_preset = PRESETS.iter().find(|p| p.id == "docker-model-runner");
            let already_via_http = detected.iter().any(|r| r.preset_id == "docker-model-runner");

            if already_via_http {
                if let Some(existing) = detected
                    .iter_mut()
                    .find(|r| r.preset_id == "docker-model-runner")
                {
                    for m in cli_models {
                        if !existing.models.iter().any(|e| e == &m) {
                            existing.models.push(m);
                        }
                    }
                }
            } else if let Some(p) = dmr_preset {
                detected.push(DiscoveredRuntime {
                    preset_id: p.id.to_string(),
                    name: p.name.to_string(),
                    base_url: p.base_url.to_string(),
                    models: cli_models,
                    note: Some(
                        "Docker Model Runner is running but localhost:12434 isn't \
                         reachable. In Docker Desktop → Settings → AI, enable \
                         \"Host-side TCP support\" (port 12434) and click \
                         \"Apply & Restart\" — the toggle alone doesn't take effect."
                            .to_string(),
                    ),
                });
            }
        }
    }

    // Best-effort: persist to the SQLite cache so the next launch can show
    // these runtimes instantly. Cache failures don't fail the probe.
    if let Err(e) = write_runtime_cache(&detected).await {
        tracing::warn!(error = %e, "failed to write runtime_cache (non-fatal)");
    }

    detected
}

/// Return last-known discovered runtimes from the SQLite cache. The UI
/// calls this on mount to render something instantly; a fresh
/// [`probe_local_runtimes`] then runs in the background and updates the
/// cache (and the UI on next refresh).
#[tauri::command]
pub async fn cached_local_runtimes() -> Vec<DiscoveredRuntime> {
    match read_runtime_cache().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "runtime_cache read failed; returning empty");
            Vec::new()
        }
    }
}

async fn read_runtime_cache() -> Result<Vec<DiscoveredRuntime>> {
    let pool = db::pool().context("sqlite pool not initialised")?;
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT preset_id, name, base_url, models_json, note FROM runtime_cache ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .context("reading runtime_cache")?;
    Ok(rows
        .into_iter()
        .map(|(preset_id, name, base_url, models_json, note)| {
            let models: Vec<String> =
                serde_json::from_str(&models_json).unwrap_or_default();
            DiscoveredRuntime {
                preset_id,
                name,
                base_url,
                models,
                note,
            }
        })
        .collect())
}

async fn write_runtime_cache(runtimes: &[DiscoveredRuntime]) -> Result<()> {
    let pool = db::pool().context("sqlite pool not initialised")?;
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Clear and reinsert. Simpler than upserting + deleting stale rows: the
    // table is small (3-7 rows max) and the writes are infrequent.
    let mut tx = pool.begin().await.context("starting cache tx")?;
    sqlx::query("DELETE FROM runtime_cache")
        .execute(&mut *tx)
        .await
        .context("clearing runtime_cache")?;
    for rt in runtimes {
        let models_json = serde_json::to_string(&rt.models).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO runtime_cache (preset_id, name, base_url, models_json, note, last_seen_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&rt.preset_id)
        .bind(&rt.name)
        .bind(&rt.base_url)
        .bind(models_json)
        .bind(rt.note.as_deref())
        .bind(now_secs)
        .execute(&mut *tx)
        .await
        .context("inserting runtime_cache row")?;
    }
    tx.commit().await.context("committing cache tx")?;
    Ok(())
}

fn is_loopback(base_url: &str) -> bool {
    let lower = base_url.to_lowercase();
    lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_check_matches_expected_hosts() {
        assert!(is_loopback("http://localhost:11434/v1"));
        assert!(is_loopback("http://127.0.0.1:1234/v1"));
        assert!(is_loopback("http://[::1]:8080/v1"));
        assert!(!is_loopback("https://api.openai.com/v1"));
        assert!(!is_loopback("http://example.com/v1"));
    }
}
