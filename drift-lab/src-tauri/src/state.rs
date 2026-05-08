//! Shared app state held by Tauri's `State` registry.

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app_config::AppConfig;
use crate::backend::ResolvedBackend;
use crate::events::BackendStatus;
use crate::history::Conversation;
use crate::model_config::ModelBackend;

pub struct AppState {
    pub backend: Arc<Mutex<Option<ResolvedBackend>>>,

    /// **Deprecated** single-config slot. Retained transitionally — the new
    /// flow uses [`app_config`] (`AppConfig`) below.
    pub config: Arc<Mutex<Option<ModelBackend>>>,

    /// Multi-provider app config (onboarding flag + saved providers).
    pub app_config: Arc<Mutex<AppConfig>>,

    /// Last-broadcast lifecycle status. Hydrated by `get_backend_status`.
    pub status: Arc<Mutex<BackendStatus>>,

    /// Conversation currently being chatted with. `None` between turns when
    /// the user has explicitly started a new chat.
    pub current_conv: Arc<Mutex<Option<Conversation>>>,

    /// Cancellation handle for the in-flight chat stream. Set at the start of
    /// `chat()`, taken (and cancelled) by `cancel_chat`.
    pub cancel_token: Arc<Mutex<Option<CancellationToken>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(Mutex::new(None)),
            config: Arc::new(Mutex::new(None)),
            app_config: Arc::new(Mutex::new(AppConfig::default())),
            status: Arc::new(Mutex::new(BackendStatus::Unconfigured)),
            current_conv: Arc::new(Mutex::new(None)),
            cancel_token: Arc::new(Mutex::new(None)),
        }
    }
}
