//! [`EventSink`] that fans `append` into two downstream sinks. Used by the
//! live realtime path to write each broadcast payload BOTH to a JSONL
//! file (durable / replayable) AND into an in-memory [`Aggregator`]
//! (sub-second live snapshots) — the user gets both guarantees without
//! the file-tail aggregator round trip we used to pay.
//!
//! ## Failure policy
//! `append` calls both inner sinks in order. If either fails, the error
//! is propagated. Persistence (`JsonlSink`) is intentionally the primary
//! — if disk fails we want to know about it. The aggregator sink is
//! pure in-memory and effectively infallible, so this ordering is
//! "primary first, observer second".

use async_trait::async_trait;
use serde_json::Value;

use crate::realtime::domain::RealtimeError;
use crate::realtime::ports::EventSink;

/// Pair of sinks driven from one input. Owns both; `append` forwards.
pub struct TeeSink {
    primary: Box<dyn EventSink>,
    observer: Box<dyn EventSink>,
}

impl TeeSink {
    pub fn new(primary: Box<dyn EventSink>, observer: Box<dyn EventSink>) -> Self {
        Self { primary, observer }
    }
}

#[async_trait]
impl EventSink for TeeSink {
    async fn append(&mut self, payload: &Value) -> Result<(), RealtimeError> {
        self.primary.append(payload).await?;
        self.observer.append(payload).await?;
        Ok(())
    }
}
