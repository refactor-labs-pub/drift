//! Shared app state held by Tauri's `State` registry.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app_config::AppConfig;
use crate::backend::ResolvedBackend;
use crate::event_log_commands::LiveScans;
use crate::event_source_commands::RealtimeStreams;

/// Registry of in-flight realtime *test* operations. Key is the
/// renderer-generated `testId`, value is the token a Stop click cancels.
/// Distinct from `realtime_streams` (which tracks long-lived
/// subscriptions) because the test is a one-shot — it lifecycles
/// differently (no file-tail aggregator paired with it).
pub type RealtimeTests = Arc<Mutex<HashMap<String, CancellationToken>>>;
use crate::events::BackendStatus;
use crate::history::Conversation;
use crate::model_config::ModelBackend;
use crate::scan::runner::{PickerRegistry, ScanCancelRegistry};
use crate::scan::suggester::SuggestionRegistry;

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

    /// Per-scan handshake registry. Holds the channel a parked
    /// `analyze_picked_with_progress` closure listens on while the user
    /// chooses an entry root from the UI. Owned by the AppState so multiple
    /// commands can poke at it without re-plumbing.
    pub scan_pickers: Arc<PickerRegistry>,

    /// Per-(scan, finding-index) cancellation registry for the LLM
    /// suggestion driver. A Stop click on a row resolves to
    /// `SuggestionRegistry::cancel`, which drops the underlying provider
    /// stream and lets the driver finalize cleanly. Held in `AppState` so
    /// both `start_scan_finding_suggestion` (registration) and
    /// `stop_scan_finding_suggestion` (cancellation) can share the map
    /// without re-plumbing through the runner.
    pub scan_suggestions: Arc<SuggestionRegistry>,

    /// Per-scan cancel flags for the static-scan runner. The Stop button on
    /// the running view resolves to `stop_static_scan`, which flips the
    /// flag — the progress sink polls it on every callback and panics with
    /// `CancelledByUser` to unwind the otherwise-uninterruptible rayon
    /// analysis pipeline.
    pub scan_cancels: Arc<ScanCancelRegistry>,

    /// Process-wide shutdown signal. Cancelled exactly once by the tray
    /// "Quit" menu / Cmd+Q path so the HTTP server's `with_graceful_shutdown`
    /// and any other long-lived task can wind down before the process
    /// exits. Kept as a `CancellationToken` (not a `oneshot::Sender`) so
    /// multiple subscribers can observe the same event.
    pub shutdown: CancellationToken,

    /// Whether `tray::install` succeeded at startup. The window-close
    /// handler reads this: when the tray is live, closing hides to tray;
    /// when it isn't (Linux without a status-notifier host, headless
    /// runtimes), closing must actually exit — otherwise the app
    /// disappears with no way to bring it back.
    pub tray_available: Arc<AtomicBool>,

    /// Live `events.log` tailer registry: `live_scan_id` → cancellation
    /// token. The stop-button path drops the entry; the task exits on the
    /// next tick. Shared with the (future) shutdown path so quitting the
    /// app cancels every in-flight tail at once.
    pub live_event_scans: LiveScans,

    /// Phase C: Supabase Realtime subscriber registry, `stream_id` →
    /// (`wss_token`, `live_scan_id`, `log_path`). One entry per active
    /// stream. `stop_realtime_event_stream` cancels the WSS token AND
    /// the paired entry in `live_event_scans` so the UI only needs to
    /// call one stop command.
    pub realtime_streams: RealtimeStreams,

    /// In-flight realtime test commands keyed by `testId`. The Stop
    /// button in Settings / Active Scan resolves to
    /// `cancel_realtime_test(testId)`, which cancels this token; the
    /// transport's `select!` arm wakes within one poll cycle. Empty
    /// in steady state — entries live only for the duration of a
    /// single connect+join test.
    pub realtime_tests: RealtimeTests,
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
            scan_pickers: Arc::new(PickerRegistry::new()),
            scan_suggestions: Arc::new(SuggestionRegistry::new()),
            scan_cancels: Arc::new(ScanCancelRegistry::new()),
            shutdown: CancellationToken::new(),
            tray_available: Arc::new(AtomicBool::new(false)),
            live_event_scans: Arc::new(Mutex::new(Default::default())),
            realtime_streams: Arc::new(Mutex::new(Default::default())),
            realtime_tests: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
