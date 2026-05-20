//! [`EventSink`] that feeds incoming broadcast payloads into a shared
//! [`Aggregator`] state instead of (or alongside) a file.
//!
//! This is the in-memory half of the live-aggregation pipeline. The
//! [`crate::realtime::infra::tee_sink::TeeSink`] writes each broadcast
//! payload BOTH here (for the live snapshot) AND to a JSONL file (for
//! replay) — the user gets sub-second live updates while keeping the
//! "saved to disk" guarantee they had before.
//!
//! ## Design
//! * One `Aggregator` per active stream, shared via `Arc<Mutex<…>>`.
//!   The sink takes the lock, ingests one payload, releases — there's
//!   no contention with the broadcaster task because it only takes the
//!   lock long enough to call `snapshot()`.
//! * A separate `Notify` is signalled on every successful append so the
//!   broadcaster can wake immediately when an event lands (instead of
//!   spin-polling). The broadcaster still throttles emits to ≤4 Hz so
//!   a 100 ev/s burst doesn't churn the UI.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::event_log::Aggregator;
use crate::realtime::domain::RealtimeError;
use crate::realtime::ports::EventSink;

/// Shared handle the sink and the broadcaster both hold. The Mutex
/// guards the aggregator state; the Notify wakes the broadcaster on
/// each append.
#[derive(Clone)]
pub struct AggregatorHandle {
    pub aggregator: Arc<Mutex<Aggregator>>,
    pub notify: Arc<Notify>,
}

impl AggregatorHandle {
    pub fn new() -> Self {
        Self {
            aggregator: Arc::new(Mutex::new(Aggregator::new())),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl Default for AggregatorHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// `EventSink` that delegates `append` into a shared [`Aggregator`].
/// Holds clones of the `Arc<Mutex<…>>` and `Arc<Notify>` so the
/// broadcaster — created alongside — can read the same state.
pub struct AggregatorSink {
    handle: AggregatorHandle,
}

impl AggregatorSink {
    pub fn new(handle: AggregatorHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl EventSink for AggregatorSink {
    async fn append(&mut self, payload: &Value) -> Result<(), RealtimeError> {
        // Lock-ingest-unlock; the critical section is one HashMap update
        // plus one Vec push. The broadcaster takes the same lock for a
        // similarly short read (`snapshot()`), so contention is bounded
        // even under bursty input.
        {
            let mut agg = self.handle.aggregator.lock().await;
            agg.ingest_value(payload);
        }
        // Wake the broadcaster. `notify_one` is cheap and idempotent —
        // if nobody is waiting yet, the permit is stored.
        self.handle.notify.notify_one();
        Ok(())
    }
}
