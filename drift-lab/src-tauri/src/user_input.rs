//! Bridge between the `ask_user` tool (server-side) and the UI's BlockedModal
//! (client-side). The agent calls `ask_user` mid-loop; the tool's `run()`
//! parks a oneshot sender here and `await`s it. The UI's "Submit" button
//! invokes the `answer_blocked_question` Tauri command, which delivers the
//! answer through that sender — and the tool's `run()` resumes with the
//! answer as its output.
//!
//! We keep the wiring deliberately small: one pending question at a time
//! (the agent loop is sequential, so this is sound), a single emitter for
//! the `agent:blocked` event, no replay buffer.

use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::oneshot;

use crate::events::BlockedQuestion;

type BlockedEmitter = Arc<dyn Fn(BlockedQuestion) + Send + Sync>;

/// Callback installed by `setup()` in `lib.rs` once the Tauri `AppHandle`
/// exists. Until then the tool is effectively a no-op (the `await` would
/// hang) — but no agent runs before `setup()`, so this is safe.
static EMITTER: OnceLock<BlockedEmitter> = OnceLock::new();

/// The one in-flight question's reply channel. Replaced (not panicked) on
/// re-entry: a stale entry means a previous run died holding the sender,
/// and overwriting it lets the receiver in that dead run's task error out
/// cleanly while we move on.
static PENDING: Mutex<Option<PendingEntry>> = Mutex::new(None);

struct PendingEntry {
    id: String,
    tx: oneshot::Sender<String>,
}

pub fn register_emitter(f: impl Fn(BlockedQuestion) + Send + Sync + 'static) {
    let _ = EMITTER.set(Arc::new(f));
}

/// Park a question and `await` the operator's answer. Returns `Err` if the
/// answer channel is closed before a reply lands (most commonly: the user
/// cancelled the run).
pub async fn ask(question: String) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut slot = PENDING.lock().unwrap();
        // Overwriting a previous PendingEntry drops its sender, which
        // causes the awaiting `rx.await` in *that* run to return `Err`.
        // That run was cancelled or crashed; this is the cleanup.
        *slot = Some(PendingEntry { id: id.clone(), tx });
    }
    if let Some(emit) = EMITTER.get() {
        emit(BlockedQuestion {
            id,
            question: question.clone(),
        });
    } else {
        tracing::warn!(
            target: "drift::user_input",
            "ask_user fired with no UI emitter registered — the agent will hang"
        );
    }
    rx.await
        .map_err(|_| "question was dropped before the user answered".to_string())
}

/// Resolve the in-flight question with `text`. Called by the
/// `answer_blocked_question` Tauri command.
pub fn answer(text: String) -> Result<(), String> {
    let entry = PENDING.lock().unwrap().take();
    let Some(entry) = entry else {
        return Err("no question is pending".to_string());
    };
    entry
        .tx
        .send(text)
        .map_err(|_| "receiver dropped before answer could be delivered".to_string())
}

/// Drop the in-flight sender (if any). Called when a run is cancelled so
/// the `ask_user` task observes the cancellation immediately rather than
/// waiting on an answer that will never come.
#[allow(dead_code)]
pub fn cancel_pending() {
    drop(PENDING.lock().unwrap().take());
}
