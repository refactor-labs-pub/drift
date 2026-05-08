use futures_util::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::message::Message;
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent_tools::{self, Toolset},
    app_config::{self, AppConfig, SavedProvider},
    backend,
    events::{topic, BackendStatus},
    history::{self, Conversation, ConversationSummary},
    model_config::ModelBackend,
    persisted,
    state::AppState,
    workflow,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRun {
    pub run_id: String,
    pub project_path: String,
    pub created_at: String,
    pub issues_found: Option<u32>,
}

#[tauri::command]
pub async fn start_run<R: Runtime>(
    app: AppHandle<R>,
    project_path: String,
) -> Result<String, String> {
    let run_id = Uuid::new_v4().to_string();
    let id_for_task = run_id.clone();
    let path_for_task = project_path.clone();
    let app_for_task = app.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(e) = workflow::execute(app_for_task, id_for_task, path_for_task).await {
            tracing::error!("workflow failed: {e:?}");
        }
    });

    Ok(run_id)
}

#[tauri::command]
pub async fn cancel_run(_run_id: String) -> Result<(), String> {
    // TODO: wire cancellation token registry once long-running stages exist.
    Ok(())
}

#[tauri::command]
pub async fn list_recent_runs<R: Runtime>(_app: AppHandle<R>) -> Result<Vec<RecentRun>, String> {
    // TODO: read from sqlite once the schema exists.
    Ok(vec![])
}

// ============================================================================
// LLM backend lifecycle
// ============================================================================

pub mod chat_topic {
    pub const TOKEN: &str = "chat:token";
    pub const DONE: &str = "chat:done";
    pub const ERROR: &str = "chat:error";
    pub const CANCELLED: &str = "chat:cancelled";
}

/// Mutate the in-memory status, then broadcast to the UI. Single helper so we
/// don't drift between memory and the wire.
pub(crate) async fn set_status<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    status: BackendStatus,
) {
    *state.status.lock().await = status.clone();
    let _ = app.emit(topic::BACKEND_STATUS, status);
}

fn idle_status(config: &ModelBackend) -> BackendStatus {
    let (mode, model) = describe(config);
    BackendStatus::Idle { mode, model }
}

fn describe(config: &ModelBackend) -> (String, String) {
    match config {
        ModelBackend::Api { model, .. } => ("api".to_string(), model.clone()),
        ModelBackend::Local { spec, .. } => ("local".to_string(), spec.clone()),
    }
}

/// Save the config to disk, drop any live runtime, and emit a fresh status.
/// The actual download / spawn happens lazily on the next `chat()` call.
#[tauri::command]
pub async fn save_backend_config<R: Runtime>(
    config: ModelBackend,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    persisted::save(&app, &config).map_err(|e| e.to_string())?;
    *state.config.lock().await = Some(config.clone());
    *state.backend.lock().await = None;
    set_status(&app, &state, idle_status(&config)).await;
    Ok(())
}

/// Return the currently persisted config (or `None` if unconfigured). Secrets
/// are returned as-is — the threat model assumes the UI can already see them.
#[tauri::command]
pub async fn load_backend_config(
    state: State<'_, AppState>,
) -> Result<Option<ModelBackend>, String> {
    Ok(state.config.lock().await.clone())
}

/// Clear persisted config + drop the live runtime.
#[tauri::command]
pub async fn clear_backend<R: Runtime>(
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    persisted::clear(&app).map_err(|e| e.to_string())?;
    *state.config.lock().await = None;
    *state.backend.lock().await = None; // drop kills llama-server via kill_on_drop
    set_status(&app, &state, BackendStatus::Unconfigured).await;
    Ok(())
}

#[tauri::command]
pub async fn get_backend_status(state: State<'_, AppState>) -> Result<BackendStatus, String> {
    Ok(state.status.lock().await.clone())
}

/// Eager configure: save, then resolve immediately. Useful when the UI wants
/// to block on download/spawn (e.g. clicking "Activate" on a downloaded
/// model). Equivalent to `save_backend_config` followed by a chat-triggered
/// resolve, but surfaces errors synchronously.
#[tauri::command]
pub async fn configure_backend<R: Runtime>(
    config: ModelBackend,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    persisted::save(&app, &config).map_err(|e| e.to_string())?;
    *state.config.lock().await = Some(config.clone());
    *state.backend.lock().await = None;

    resolve_with_status(&app, &state, config)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve the given config into a live backend, broadcasting status as it
/// progresses. On success the resolved backend is stored in `state.backend`.
async fn resolve_with_status<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    config: ModelBackend,
) -> anyhow::Result<()> {
    let (mode, model) = describe(&config);

    // Coarse pre-resolve status. Local mode passes through Downloading/Starting
    // implicitly via download::ensure_model + local_server::spawn_llama_server,
    // but we don't have a hook for those yet — that's Phase D.
    set_status(
        app,
        state,
        match &config {
            ModelBackend::Local { spec, .. } => BackendStatus::Downloading {
                file: spec.clone(),
            },
            ModelBackend::Api { .. } => BackendStatus::Starting,
        },
    )
    .await;

    match backend::resolve(config, app).await {
        Ok(resolved) => {
            *state.backend.lock().await = Some(resolved);
            set_status(app, state, BackendStatus::Ready { mode, model }).await;
            Ok(())
        }
        Err(e) => {
            set_status(
                app,
                state,
                BackendStatus::Error {
                    message: e.to_string(),
                },
            )
            .await;
            Err(e)
        }
    }
}

/// If `state.backend` is `None` but `state.config` is `Some`, resolve it now.
/// No-op if the backend is already live or there's nothing to resolve.
async fn ensure_resolved<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> Result<(), String> {
    {
        if state.backend.lock().await.is_some() {
            return Ok(());
        }
    }
    let config = {
        match state.config.lock().await.clone() {
            Some(c) => c,
            None => return Err("backend not configured".to_string()),
        }
    };
    resolve_with_status(app, state, config)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn chat<R: Runtime>(
    message: String,
    preamble: Option<String>,
    toolset: Option<Toolset>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    ensure_resolved(&app, &state).await?;

    // Snapshot the agent + history *before* awaiting the stream so we don't
    // hold the locks across the streaming loop (which would deadlock other
    // commands like cancel_chat).
    let (agent, history) = {
        let guard = state.backend.lock().await;
        let resolved = guard
            .as_ref()
            .ok_or_else(|| "backend not configured".to_string())?;

        let builder = resolved
            .client
            .agent(&resolved.model)
            .preamble(
                preamble
                    .as_deref()
                    .unwrap_or("You are a helpful assistant embedded in Drift Lab."),
            );
        let agent = agent_tools::install(builder, toolset.unwrap_or_default()).build();

        // Start (or continue) a conversation. History is the prior messages —
        // the user's *new* message goes through `stream_chat`'s prompt arg.
        let mut conv_guard = state.current_conv.lock().await;
        if conv_guard.is_none() {
            *conv_guard = Some(Conversation::new(&message));
        }
        let history = conv_guard
            .as_ref()
            .map(|c| c.messages.clone())
            .unwrap_or_default();

        (agent, history)
    };

    // Cancellation: install a fresh token and race the stream against it.
    let token = CancellationToken::new();
    *state.cancel_token.lock().await = Some(token.clone());

    let mut stream = agent.stream_chat(message.clone(), history).await;
    let mut full_response = String::new();
    let mut cancelled = false;

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                cancelled = true;
                let _ = app.emit(chat_topic::CANCELLED, ());
                break;
            }
            item = stream.next() => {
                match item {
                    Some(Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    ))) => {
                        full_response.push_str(&text.text);
                        let _ = app.emit(chat_topic::TOKEN, text.text);
                    }
                    Some(Ok(_)) => {
                        // Tool calls, deltas, final response, etc. — surfaced once tools land.
                    }
                    Some(Err(e)) => {
                        let _ = app.emit(chat_topic::ERROR, e.to_string());
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    // Clear the cancellation token regardless of how we exited.
    *state.cancel_token.lock().await = None;

    // Persist the turn (even if cancelled — partial responses are useful).
    {
        let mut conv_guard = state.current_conv.lock().await;
        if let Some(conv) = conv_guard.as_mut() {
            conv.messages.push(Message::user(message));
            if !full_response.is_empty() {
                conv.messages.push(Message::assistant(full_response));
            }
            conv.touch();
            if let Err(e) = history::save(&app, conv) {
                tracing::warn!("saving conversation: {e:?}");
            }
        }
    }

    if !cancelled {
        let _ = app.emit(chat_topic::DONE, ());
    }
    Ok(())
}

/// Cancel the in-flight chat stream, if any. Returns immediately; the chat
/// command's loop sees the token and breaks on its next iteration.
#[tauri::command]
pub async fn cancel_chat(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(token) = state.cancel_token.lock().await.take() {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_oneshot<R: Runtime>(
    message: String,
    preamble: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<String, String> {
    ensure_resolved(&app, &state).await?;

    let guard = state.backend.lock().await;
    let resolved = guard
        .as_ref()
        .ok_or_else(|| "backend not configured".to_string())?;

    let agent = resolved
        .client
        .agent(&resolved.model)
        .preamble(
            preamble
                .as_deref()
                .unwrap_or("You are a helpful assistant embedded in Drift Lab."),
        )
        .build();

    agent.prompt(message).await.map_err(|e| e.to_string())
}

// ============================================================================
// Multi-provider config (Phase 1.5)
// ============================================================================

/// Returns the current app config. Frontend calls this on startup to decide
/// whether to show onboarding.
#[tauri::command]
pub async fn get_app_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(state.app_config.lock().await.clone())
}

/// Probe a candidate provider config. For API mode, sends a 1-token request
/// to verify the URL + key + model triple. For Local, only validates the spec
/// shape — actually spawning llama-server happens via `save_provider`.
#[tauri::command]
pub async fn test_provider(config: ModelBackend) -> Result<(), String> {
    match &config {
        ModelBackend::Api {
            base_url,
            api_key,
            model,
        } => {
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let resp = reqwest::Client::new()
                .post(&url)
                .bearer_auth(api_key)
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "ping"}],
                    "max_tokens": 1,
                }))
                .send()
                .await
                .map_err(|e| format!("network: {e}"))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP {status}: {body}"));
            }
            Ok(())
        }
        ModelBackend::Local { spec, .. } => {
            if !spec.contains('/') || !spec.contains(':') {
                return Err("Local spec must look like `repo_id:quant`".into());
            }
            Ok(())
        }
    }
}

/// Add a provider; if `activate`, set as active and **kick a background
/// resolve**. The resolve emits `backend:status` events as it progresses
/// (`downloading` → `starting` → `ready`/`error`) — the UI listens for those
/// instead of awaiting this command.
///
/// Why background: for Local mode, `llama-server -hf` can take 10+ minutes
/// to download a multi-GB model on first run. Blocking on that would freeze
/// the UI and route any error through a single point with no progress.
#[tauri::command]
pub async fn save_provider<R: Runtime>(
    name: String,
    config: ModelBackend,
    activate: bool,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<SavedProvider, String> {
    let provider = SavedProvider::new(name, config.clone());
    {
        let mut cfg = state.app_config.lock().await;
        cfg.providers.push(provider.clone());
        if activate {
            cfg.active_provider_id = Some(provider.id.clone());
            cfg.onboarding_complete = true;
        }
        app_config::save(&app, &cfg).map_err(|e| e.to_string())?;
    }

    if activate {
        *state.backend.lock().await = None;
        spawn_background_resolve(&app, config);
    }
    Ok(provider)
}

/// Switch which saved provider is active. Drops the old runtime, persists
/// the choice, **kicks resolve in the background**.
#[tauri::command]
pub async fn activate_provider<R: Runtime>(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    let provider = {
        let cfg = state.app_config.lock().await;
        cfg.providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| "provider not found".to_string())?
            .clone()
    };

    *state.backend.lock().await = None;

    {
        let mut cfg = state.app_config.lock().await;
        cfg.active_provider_id = Some(id.clone());
        app_config::save(&app, &cfg).map_err(|e| e.to_string())?;
    }

    spawn_background_resolve(&app, provider.config);
    Ok(())
}

/// Spawn a tokio task that runs [`resolve_with_status`] for the given
/// config. Status events are the only feedback channel — the JS caller has
/// already returned by the time this runs.
fn spawn_background_resolve<R: Runtime>(app: &AppHandle<R>, config: ModelBackend) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state: tauri::State<'_, AppState> = app.state();
        if let Err(e) = resolve_with_status(&app, &state, config).await {
            // `resolve_with_status` already emitted an Error status event.
            tracing::error!("background resolve failed: {e:?}");
        }
    });
}

/// Remove a saved provider. If it was the active one, the live backend is
/// dropped and `active_provider_id` is cleared.
#[tauri::command]
pub async fn delete_provider<R: Runtime>(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    let mut cfg = state.app_config.lock().await;
    cfg.providers.retain(|p| p.id != id);
    let was_active = cfg.active_provider_id.as_deref() == Some(&id);
    if was_active {
        cfg.active_provider_id = None;
    }
    app_config::save(&app, &cfg).map_err(|e| e.to_string())?;
    drop(cfg);

    if was_active {
        *state.backend.lock().await = None;
        set_status(&app, &state, BackendStatus::Unconfigured).await;
    }
    Ok(())
}

/// Nuclear reset for the "Reset Provider and Model" button. Wipes the entire
/// AppConfig and drops the live backend.
#[tauri::command]
pub async fn reset_all_config<R: Runtime>(
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    *state.backend.lock().await = None;
    {
        let mut cfg = state.app_config.lock().await;
        *cfg = AppConfig::default();
        app_config::save(&app, &cfg).map_err(|e| e.to_string())?;
    }
    set_status(&app, &state, BackendStatus::Unconfigured).await;
    Ok(())
}

/// Hydrate `AppConfig` from `tauri-plugin-store` and, if there's an active
/// provider, kick a background resolve so chat is hot when the UI reaches it.
/// Falls back to legacy single-config (`backend.json`) if no AppConfig exists
/// yet — one-time migration for installs that pre-date Phase 1.5.
pub async fn hydrate_app_config_on_startup<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    let cfg = match app_config::load(app) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("loading app-config: {e:?}");
            AppConfig::default()
        }
    };

    // Migration path: if no providers but a legacy single config exists, fold
    // it into the new shape so the user doesn't lose their settings.
    let cfg = if cfg.providers.is_empty() {
        match persisted::load(app) {
            Ok(Some(legacy)) => {
                let mut migrated = AppConfig::default();
                let provider = SavedProvider::new("Imported".to_string(), legacy);
                migrated.active_provider_id = Some(provider.id.clone());
                migrated.onboarding_complete = true;
                migrated.providers.push(provider);
                if let Err(e) = app_config::save(app, &migrated) {
                    tracing::warn!("migrating legacy backend.json: {e:?}");
                }
                let _ = persisted::clear(app); // best-effort cleanup
                migrated
            }
            _ => cfg,
        }
    } else {
        cfg
    };

    *state.app_config.lock().await = cfg.clone();

    // If there's an active provider, also seed the legacy `state.config` slot
    // and emit `idle` status so the existing Settings UI shows the right thing
    // until it migrates.
    if let Some(active) = cfg
        .active_provider_id
        .as_ref()
        .and_then(|id| cfg.providers.iter().find(|p| &p.id == id))
    {
        *state.config.lock().await = Some(active.config.clone());
        set_status(app, state, idle_status(&active.config)).await;
    } else {
        set_status(app, state, BackendStatus::Unconfigured).await;
    }
}

// ============================================================================
// Conversation history (Phase 3)
// ============================================================================

#[tauri::command]
pub async fn list_conversations<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<ConversationSummary>, String> {
    history::list(&app).map_err(|e| e.to_string())
}

/// Load a conversation by id and make it the active one (subsequent `chat`
/// calls will append to it).
#[tauri::command]
pub async fn load_conversation<R: Runtime>(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<Conversation, String> {
    let conv = history::load(&app, &id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("conversation not found: {id}"))?;
    *state.current_conv.lock().await = Some(conv.clone());
    Ok(conv)
}

/// Drop the active conversation. The next `chat()` call will start a fresh one.
#[tauri::command]
pub async fn new_conversation(state: State<'_, AppState>) -> Result<(), String> {
    *state.current_conv.lock().await = None;
    Ok(())
}

#[tauri::command]
pub async fn delete_conversation<R: Runtime>(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    history::delete(&app, &id).map_err(|e| e.to_string())?;
    let mut g = state.current_conv.lock().await;
    if g.as_ref().map(|c| c.id == id).unwrap_or(false) {
        *g = None;
    }
    Ok(())
}

/// Returns the active conversation (if any) — used by the UI on mount to
/// rehydrate the chat surface.
#[tauri::command]
pub async fn get_current_conversation(
    state: State<'_, AppState>,
) -> Result<Option<Conversation>, String> {
    Ok(state.current_conv.lock().await.clone())
}
