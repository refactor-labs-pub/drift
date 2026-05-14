use futures_util::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::message::Message;
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::{self as agent_loop, Agent, TokenLimitParam},
    agent_tools::{self, Toolset},
    app_config::{self, AppConfig, SavedProvider, ScanFilters},
    backend,
    events::{topic, BackendStatus},
    history::{self, Conversation, ConversationSummary},
    model_config::ModelBackend,
    state::AppState,
    workflow,
};

/// Optionally configure a `rig` agent builder with a token budget.
///
/// **Opt-in.** When `limit` is `None` (the default for both `chat` and
/// `chat_oneshot`), we let OpenAI's server pick its model-appropriate
/// default — that matches the canonical guidance in the
/// [Chat Completions API reference](https://platform.openai.com/docs/api-reference/chat/create)
/// and avoids fighting strict-validation deployments (Azure, LiteLLM)
/// over a value that's just a cap, not a forced allocation.
///
/// When `limit` is `Some(N)`, we route through the right wire field:
/// `rig::agent::AgentBuilder::max_tokens` always serialises as the JSON
/// `"max_tokens"`, which GPT-5 / o-series reject with
/// `400 unsupported_parameter` — so reasoning models go through
/// `.additional_params(json)` to inject `max_completion_tokens` instead.
fn with_token_budget<M>(
    builder: rig::agent::AgentBuilder<M>,
    model: &str,
    limit: Option<u32>,
) -> rig::agent::AgentBuilder<M>
where
    M: rig::completion::CompletionModel,
{
    let Some(limit) = limit else {
        return builder;
    };
    let param = TokenLimitParam::for_model(model);
    match param {
        TokenLimitParam::MaxTokens => builder.max_tokens(limit as u64),
        TokenLimitParam::MaxCompletionTokens => {
            builder.additional_params(param.as_additional_params(limit))
        }
    }
}

/// Resolve the currently-pending `ask_user` question with the operator's
/// free-text answer. The agent's `ask_user` tool was parked on a oneshot;
/// this command sends through it and the agent loop resumes with `answer`
/// as the tool's content. Idempotent-ish — if no question is pending we
/// return a friendly error so the UI can surface "nothing to answer".
#[tauri::command]
pub fn answer_blocked_question(answer: String) -> Result<(), String> {
    crate::user_input::answer(answer)
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
        ModelBackend::Anthropic { model, .. } => ("anthropic".to_string(), model.clone()),
    }
}

/// Save the config in memory + AppConfig store and drop any live runtime.
/// Resolve happens lazily on the next chat.
#[tauri::command]
pub async fn save_backend_config<R: Runtime>(
    config: ModelBackend,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
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

/// Clear the in-memory config + drop the live runtime.
#[tauri::command]
pub async fn clear_backend<R: Runtime>(
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    *state.config.lock().await = None;
    *state.backend.lock().await = None;
    set_status(&app, &state, BackendStatus::Unconfigured).await;
    Ok(())
}

#[tauri::command]
pub async fn get_backend_status(state: State<'_, AppState>) -> Result<BackendStatus, String> {
    Ok(state.status.lock().await.clone())
}

/// Eager configure: store the config + resolve immediately so the caller
/// blocks on connection errors instead of seeing them on first chat.
#[tauri::command]
pub async fn configure_backend<R: Runtime>(
    config: ModelBackend,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
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

    set_status(app, state, BackendStatus::Starting).await;

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
        let builder = with_token_budget(builder, &resolved.model, None);
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

    let builder = resolved
        .client
        .agent(&resolved.model)
        .preamble(
            preamble
                .as_deref()
                .unwrap_or("You are a helpful assistant embedded in Drift Lab."),
        );
    let agent = with_token_budget(builder, &resolved.model, None).build();

    agent.prompt(message).await.map_err(|e| e.to_string())
}

// ============================================================================
// Iterative agent (goose-style outer/inner loop)
// ============================================================================

/// Tauri-side `WorkflowSink` — forwards each captured event onto the
/// existing `run://*` topics so the existing `Steps.tsx` UI Just Works.
struct TauriWorkflowSink<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> crate::agent::workflow::WorkflowSink for TauriWorkflowSink<R> {
    fn emit_step(&self, update: crate::events::StepUpdate) {
        let _ = self.app.emit(topic::STEP, update);
    }
    fn emit_complete(&self, complete: crate::events::RunComplete) {
        let _ = self.app.emit(topic::COMPLETE, complete);
    }
    fn emit_error(&self, error: crate::events::RunError) {
        let _ = self.app.emit(topic::ERROR, error);
    }
    fn emit_agent_event(&self, event: &crate::agent::agent_loop::AgentEvent) {
        // Mirror the raw AgentEvent to the UI so the `ReasoningLog` panel
        // can render the streaming thoughts + tool dispatches.
        let _ = self.app.emit(topic::AGENT_EVENT, event);
    }
    fn emit_telemetry(&self, sample: crate::events::TelemetrySample) {
        // ~2 Hz docker-stats snapshots → TelemetryPane on the right of the
        // split-view running screen.
        let _ = self.app.emit(topic::TELEMETRY, sample);
    }
    fn emit_report(&self, report: crate::events::RunReport) {
        // Structured "visibility map" (critical issues + warnings + advice)
        // that powers Report.tsx's VisibilityMapPanel.
        let _ = self.app.emit(topic::REPORT, report);
    }
}

/// LLM-driven scan. Replaces the deterministic [`workflow::execute`] simulator
/// with a real agent run. Emits the same `run://step / run://complete /
/// run://error` events so existing UI listeners keep working — but the step
/// `detail` field is now the model's prose, not a hardcoded string.
///
/// `mode` defaults to `auto` because a scan is the user's explicit ask;
/// they're consenting to the destructive stages (install_profiler etc.).
#[derive(Debug, serde::Serialize)]
pub struct PromptPreset {
    pub label: &'static str,
    pub prompt: &'static str,
}

/// List the canned scan goals the UI shows above the "start scan" button.
/// `Other` is implicit — the UI lets the user type a free-text prompt, which
/// is passed verbatim as `goal_prompt` to `start_agent_run`.
#[tauri::command]
pub fn list_prompt_presets() -> Vec<PromptPreset> {
    crate::agent::workflow::PROMPT_PRESETS
        .iter()
        .map(|(label, prompt)| PromptPreset { label, prompt })
        .collect()
}

#[tauri::command]
pub async fn start_agent_run<R: Runtime>(
    project_path: String,
    mode: Option<String>,
    goal_prompt: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<String, String> {
    ensure_resolved(&app, &state).await?;

    let config = state
        .config
        .lock()
        .await
        .clone()
        .ok_or_else(|| "backend not configured".to_string())?;

    let mode = match mode.as_deref() {
        Some("default") => crate::agent::tools::Mode::Default,
        Some("read_only") => crate::agent::tools::Mode::ReadOnly,
        _ => crate::agent::tools::Mode::Auto,
    };

    let run_id = Uuid::new_v4().to_string();

    // Every scan starts from scratch. If a previous run is still in flight,
    // cancel it first — its task holds a clone of the OLD token, so just
    // overwriting the slot wouldn't stop it. Cancelling the old token
    // signals the previous workflow to break out of its `select!` loop;
    // the next allocation below installs the fresh token for the new run.
    let token = {
        let mut slot = state.cancel_token.lock().await;
        if let Some(prev) = slot.take() {
            prev.cancel();
            tracing::info!(run_id = %run_id, "cancelled previous in-flight scan");
        }
        let token = CancellationToken::new();
        *slot = Some(token.clone());
        token
    };

    let app_for_task = app.clone();
    let run_id_for_task = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let provider = crate::agent::make_provider(config);
        let sink = TauriWorkflowSink {
            app: app_for_task.clone(),
        };
        let req = crate::agent::workflow::RunRequest {
            run_id: run_id_for_task.clone(),
            project_path,
            provider,
            mode,
            goal_prompt,
        };
        tracing::info!(run_id = %run_id_for_task, "scan workflow starting");
        if let Err(e) = crate::agent::workflow::run(req, &sink, token).await {
            tracing::error!(run_id = %run_id_for_task, "agent workflow failed: {e}");
        }
        tracing::info!(run_id = %run_id_for_task, "scan workflow completed");
    });

    Ok(run_id)
}

/// One-shot iterative-agent invocation. Streams `agent:event` events to the UI
/// (TextDelta / AssistantMessage / ToolDispatched / ToolCompleted / Done /
/// Error). The same `cancel_chat` command interrupts this loop.
///
/// Mode controls the permission gate:
///   - `default` (default): read-only tools auto-approved, others denied with
///     a synthesised tool response so the model can pivot.
///   - `auto`: every tool runs without prompting. Opt-in for autonomous runs.
///   - `read_only`: like default but never auto-approves anything destructive.
#[tauri::command]
pub async fn agent_chat<R: Runtime>(
    message: String,
    preamble: Option<String>,
    mode: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<(), String> {
    use futures_util::StreamExt;

    ensure_resolved(&app, &state).await?;

    // Pull the active provider config and stand up the matching provider
    // (OpenAI-compat for cloud OpenAI / local runtimes; Anthropic for
    // Claude). The factory keeps both paths in lockstep.
    let config = state
        .config
        .lock()
        .await
        .clone()
        .ok_or_else(|| "backend not configured".to_string())?;

    let provider = crate::agent::make_provider(config);
    let mode = match mode.as_deref() {
        Some("auto") => agent_loop::Mode::Auto,
        Some("read_only") => agent_loop::Mode::ReadOnly,
        _ => agent_loop::Mode::Default,
    };

    let agent = Agent::new(
        provider,
        preamble.unwrap_or_else(|| {
            "You are Drift Lab's profiling agent. Use the available tools to \
             investigate the user's project, run a profile, and report findings."
                .to_string()
        }),
    )
    .with_mode(mode);

    let token = CancellationToken::new();
    *state.cancel_token.lock().await = Some(token.clone());

    let mut stream = Box::pin(agent.reply(message, vec![], token.clone()));
    while let Some(item) = stream.next().await {
        let event = match item {
            Ok(e) => e,
            Err(e) => agent_loop::AgentEvent::Error {
                message: e.to_string(),
            },
        };
        let _ = app.emit(topic::AGENT_EVENT, &event);
        // Stop streaming once we hit a terminal event.
        if matches!(
            event,
            agent_loop::AgentEvent::Done
                | agent_loop::AgentEvent::Error { .. }
                | agent_loop::AgentEvent::TurnBudgetExceeded { .. }
        ) {
            break;
        }
    }

    *state.cancel_token.lock().await = None;
    Ok(())
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

/// Persist a new set of scan-filter preferences. The Settings UI calls this
/// when the user toggles a switch; the static-scan runner reads from the
/// same `AppConfig.scan_filters` at scan kick-off, so a fresh toggle takes
/// effect on the very next scan with no extra plumbing.
#[tauri::command]
pub async fn update_scan_filters<R: Runtime>(
    filters: ScanFilters,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<ScanFilters, String> {
    let mut cfg = state.app_config.lock().await;
    cfg.scan_filters = filters;
    app_config::save(&app, &cfg).map_err(|e| e.to_string())?;
    Ok(cfg.scan_filters)
}

/// Probe a candidate provider config. Sends a 1-token request to verify the
/// URL + key + model triple speaks the expected protocol. Each provider has
/// its own quirks — auth header, token-limit field, body shape — so they
/// can't share a single probe.
#[tauri::command]
pub async fn test_provider(config: ModelBackend) -> Result<(), String> {
    match &config {
        ModelBackend::Api {
            base_url,
            api_key,
            model,
        } => {
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut body = serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "ping"}],
            });
            TokenLimitParam::for_model(model).apply(&mut body, 1);
            let resp = reqwest::Client::new()
                .post(&url)
                .bearer_auth(api_key)
                .json(&body)
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
        ModelBackend::Anthropic {
            base_url,
            api_key,
            model,
        } => {
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
            });
            let resp = reqwest::Client::new()
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", crate::agent::anthropic::ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
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
/// provider, seed `state.config` so chat is ready to go without an extra round trip.
pub async fn hydrate_app_config_on_startup<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    let cfg = match app_config::load(app) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("loading app-config: {e:?}");
            AppConfig::default()
        }
    };

    *state.app_config.lock().await = cfg.clone();

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
