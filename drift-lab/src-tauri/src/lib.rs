mod agent_tools;
mod app_config;
mod backend;
mod commands;
mod db;
mod docker;
mod events;
mod history;
mod local_server;
mod model_config;
mod model_discovery;
mod persisted;
mod presets;
#[allow(dead_code)] // Trait + file-backed impl. Kept as the swap path to a future KeychainSecretStore.
mod secret_store;
mod state;
#[allow(dead_code)] // Each tool is independently callable by the LLM; not all are wired into workflow.rs yet.
mod tools;
mod tray;
mod workflow;

use tauri::Manager;
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .manage(state::AppState::new())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
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

            // Initialize SQLite app database.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = db::init(&handle).await {
                    tracing::error!("db init failed: {e:?}");
                }
                info!("drift-lab ready");
            });

            // Hydrate persisted LLM backend config. We seed AppState from the
            // multi-provider AppConfig (with one-time migration from the older
            // single-config `backend.json`) and, if there's an active provider,
            // kick a background resolve so chat is hot when the UI gets there.
            let handle_for_hydrate = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state: tauri::State<'_, state::AppState> = handle_for_hydrate.state();
                commands::hydrate_app_config_on_startup(&handle_for_hydrate, &state).await;

                // Eager resolve of the active provider, in the background.
                let active = {
                    let cfg = state.app_config.lock().await;
                    cfg.active_provider_id
                        .as_ref()
                        .and_then(|id| cfg.providers.iter().find(|p| &p.id == id).cloned())
                };
                if let Some(provider) = active {
                    let mode = match &provider.config {
                        model_config::ModelBackend::Api { .. } => "api".to_string(),
                        model_config::ModelBackend::Local { .. } => "local".to_string(),
                    };
                    let model_label = match &provider.config {
                        model_config::ModelBackend::Api { model, .. } => model.clone(),
                        model_config::ModelBackend::Local { spec, .. } => spec.clone(),
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
            commands::list_recent_runs,
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
            // Curated catalogs.
            presets::list_presets,
            presets::list_local_presets,
            // Live discovery (HF search + endpoint /v1/models probe).
            model_discovery::search_hf_models,
            model_discovery::list_hf_quants,
            model_discovery::list_models_from_endpoint,
            // Local-server pre-flight (is `llama-server` on PATH?).
            local_server::check_llama_server,
            // Chat.
            commands::chat,
            commands::chat_oneshot,
            commands::cancel_chat,
            // Conversation history (Phase 3).
            commands::list_conversations,
            commands::load_conversation,
            commands::new_conversation,
            commands::delete_conversation,
            commands::get_current_conversation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
