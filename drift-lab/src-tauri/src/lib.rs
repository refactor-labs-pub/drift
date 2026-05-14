#[allow(dead_code)] // Public API surface — callers (tests, future commands) opt in over time.
pub mod agent;
mod agent_tools;
mod app_config;
mod backend;
mod commands;
mod db;
mod docker;
pub mod events;
mod history;
mod model_config;
pub mod model_discovery;
mod presets;
pub mod scan;
mod scan_commands;
#[allow(dead_code)] // Trait + file-backed impl. Kept as the swap path to a future KeychainSecretStore.
mod secret_store;
mod state;
mod telemetry;
mod user_input;
#[allow(dead_code)] // Each tool is independently callable by the LLM; not all are wired into workflow.rs yet.
pub mod tools;
mod tray;
mod workflow;

use std::sync::{Arc, OnceLock};

use tauri::{Emitter, Manager};
use tracing::info;

use crate::events::LogLine;

/// Bridge from the backend's `tracing` pipeline to the UI: every event that
/// makes it past the `EnvFilter` is also packaged as a [`LogLine`] and pushed
/// to whatever callback `register_log_emitter` last installed.
///
/// We use an `Arc<dyn Fn>` (not a direct `AppHandle`) so the
/// `TauriEventLayer` stays generic — `AppHandle<R>` is `Runtime`-parameterised
/// and would force the layer (and every test that touches tracing) to pick a
/// runtime up front.
type LogEmitter = Arc<dyn Fn(LogLine) + Send + Sync>;
static LOG_EMITTER: OnceLock<LogEmitter> = OnceLock::new();

fn register_log_emitter(emitter: impl Fn(LogLine) + Send + Sync + 'static) {
    // `OnceLock::set` only succeeds the first time; subsequent setup runs
    // (e.g. in tests) silently keep the original — that's fine here.
    let _ = LOG_EMITTER.set(Arc::new(emitter));
}

/// Initialise a `tracing` subscriber that writes formatted lines to stderr
/// *and* mirrors them to the UI via [`register_log_emitter`].
///
/// `RUST_LOG` controls the filter (default `info,drift=debug` so the agent /
/// tools / workflow log lines all show up while we keep external crates
/// quiet). Note: this also installs a global `log` logger via `tracing-log`,
/// which is why we don't register `tauri-plugin-log` — both would race to set
/// the global `log::set_logger` and the second one panics.
fn init_tracing() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,drift=debug,drift_lab_lib=debug"));
    let stderr_layer = fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .compact();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(TauriEventLayer)
        .try_init();
}

/// `tracing_subscriber::Layer` that forwards each filtered event to the
/// registered emitter. No-op until the emitter is installed (which happens
/// inside the Tauri `setup()` once an `AppHandle` is available).
struct TauriEventLayer;

impl<S> tracing_subscriber::Layer<S> for TauriEventLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(emitter) = LOG_EMITTER.get() else { return };
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let line = LogLine {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: visitor.into_string(),
        };
        emitter(line);
    }
}

/// Field visitor that pulls out the `message` plus any `k=v` extras a
/// `tracing::info!` invocation attached. We strip the surrounding quotes
/// that `Debug` adds around string-like fields so the UI doesn't show
/// `"scan workflow starting"` with literal quotes.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn into_string(self) -> String {
        let MessageVisitor {
            mut message,
            fields,
        } = self;
        if fields.is_empty() {
            return message;
        }
        if !message.is_empty() {
            message.push_str("  ");
        }
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 {
                message.push(' ');
            }
            use std::fmt::Write as _;
            let _ = write!(message, "{k}={v}");
        }
        message
    }

    fn strip_debug_quotes(s: String) -> String {
        if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
            s[1..s.len() - 1].to_string()
        } else {
            s
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let raw = format!("{value:?}");
        let cleaned = Self::strip_debug_quotes(raw);
        if field.name() == "message" {
            self.message = cleaned;
        } else {
            self.fields.push((field.name().to_string(), cleaned));
        }
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();
    let mut builder = tauri::Builder::default()
        .manage(state::AppState::new())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .setup(|app| {
            // Set up tray icon (best-effort; fails silently in headless test envs).
            if let Err(e) = tray::install(app.handle()) {
                tracing::warn!("tray install failed: {e}");
            }

            // Hook the tracing pipeline into Tauri's event bus so the UI's
            // BackendLogPane can mirror what's printed to stderr. Installing
            // the emitter here (not in `init_tracing`) means tests and CLI
            // launches that never build an AppHandle keep getting the
            // stderr-only behaviour.
            let handle_for_log = app.handle().clone();
            register_log_emitter(move |line| {
                let _ = handle_for_log.emit(events::topic::LOG, line);
            });

            // Bridge the `ask_user` tool to the BlockedModal: the tool parks
            // a oneshot and emits a question; this callback forwards the
            // question payload over Tauri so the UI can render the modal.
            let handle_for_blocked = app.handle().clone();
            user_input::register_emitter(move |q| {
                let _ = handle_for_blocked.emit(events::topic::BLOCKED, q);
            });

            info!("drift-lab ready");

            // Open the SQLite app store (runtime_cache + future tables).
            let handle_for_db = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = db::init(&handle_for_db).await {
                    tracing::warn!("sqlite init failed (non-fatal): {e:?}");
                }
            });

            // Hydrate persisted LLM backend config + eager-resolve the active
            // provider in the background so chat is hot when the UI gets there.
            let handle_for_hydrate = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state: tauri::State<'_, state::AppState> = handle_for_hydrate.state();
                commands::hydrate_app_config_on_startup(&handle_for_hydrate, &state).await;

                let active = {
                    let cfg = state.app_config.lock().await;
                    cfg.active_provider_id
                        .as_ref()
                        .and_then(|id| cfg.providers.iter().find(|p| &p.id == id).cloned())
                };
                if let Some(provider) = active {
                    let mode = "api".to_string();
                    let model_label = match &provider.config {
                        model_config::ModelBackend::Api { model, .. } => model.clone(),
                    };

                    match backend::resolve(provider.config, &handle_for_hydrate).await {
                        Ok(resolved) => {
                            *state.backend.lock().await = Some(resolved);
                            commands::set_status(
                                &handle_for_hydrate,
                                &state,
                                events::BackendStatus::Ready {
                                    mode,
                                    model: model_label,
                                },
                            )
                            .await;
                            tracing::info!("active provider `{}` resolved", provider.name);
                        }
                        Err(e) => {
                            tracing::warn!("eager backend resolve failed: {e:?}");
                            commands::set_status(
                                &handle_for_hydrate,
                                &state,
                                events::BackendStatus::Error {
                                    message: e.to_string(),
                                },
                            )
                            .await;
                        }
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_run,
            commands::cancel_run,
            commands::answer_blocked_question,
            // Single-provider (legacy) — kept for the existing Settings UI.
            commands::configure_backend,
            commands::save_backend_config,
            commands::load_backend_config,
            commands::clear_backend,
            commands::get_backend_status,
            // Multi-provider (Phase 1.5).
            commands::get_app_config,
            commands::test_provider,
            commands::save_provider,
            commands::activate_provider,
            commands::delete_provider,
            commands::reset_all_config,
            // Curated provider catalog.
            presets::list_presets,
            // Live discovery: probe local runtimes + endpoint /v1/models probe.
            model_discovery::probe_local_runtimes,
            model_discovery::cached_local_runtimes,
            model_discovery::list_models_from_endpoint,
            // Chat.
            commands::chat,
            commands::chat_oneshot,
            commands::cancel_chat,
            // Iterative agent loop (goose-style).
            commands::agent_chat,
            commands::start_agent_run,
            commands::list_prompt_presets,
            // Conversation history (Phase 3).
            commands::list_conversations,
            commands::load_conversation,
            commands::new_conversation,
            commands::delete_conversation,
            commands::get_current_conversation,
            // Static scan — two-step pick flow + suggestion driver.
            scan_commands::start_static_scan,
            scan_commands::select_entry_and_scan,
            scan_commands::list_static_scans,
            scan_commands::load_static_scan,
            scan_commands::list_scan_entries,
            scan_commands::start_scan_suggestions,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Hard-exit on any quit path. The static scan runs via
            // `spawn_blocking` + rayon — neither honors cancellation,
            // so a graceful Tauri/Tokio shutdown would block (sometimes
            // for minutes) waiting for the analysis to finish. We
            // deliberately bypass runtime drop and kill the process
            // (and all its threads) immediately.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                std::process::exit(0);
            }
        });
}
