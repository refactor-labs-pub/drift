//! Tungstenite-backed [`RealtimeTransport`]. The one adapter that actually
//! speaks WSS and Phoenix Channels frames. Lives in `infra/` because it's
//! the only place in this subsystem that touches `tokio_tungstenite`.
//!
//! What lives here:
//! * `TungsteniteTransport` — stateless, holds no I/O resources. One
//!   instance per process is fine.
//! * `test_connection` impl — one-shot connect+join+close used by Settings.
//! * `run_stream` impl — long-lived connect → join → drain → reconnect
//!   loop, with heartbeats and cancellation-aware sleeps.
//!
//! What does NOT live here:
//! * Anything that touches `AppHandle`, `AppState`, the secret store, or
//!   the file-tail aggregator. Those are wiring concerns owned by the
//!   `app/` (use cases) or the Tauri command shims.
//! * Envelope shapes — those are in [`super::protocol`] so they can be
//!   tested without a network in sight.

use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::time::interval;

// rustls CryptoProvider is installed eagerly via `crate::realtime::init()`
// at process startup (from `lib::run()` for the desktop app and from each
// example's `main()`). No lazy install here.
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

use super::protocol::{
    extract_broadcast_payload, heartbeat_envelope, join_envelope, leave_envelope,
    parse_join_reply, topic_for_channel, JoinReply,
};
use crate::realtime::domain::{
    defaults::{BACKOFF_STEPS_SECS, HEARTBEAT_SECS, TEST_CONNECTION_BUDGET_SECS},
    url::build_wss_url,
    EffectiveStreamConfig, RealtimeError,
};
use crate::realtime::ports::{
    EventSink, RealtimeTransport, StreamStatus, TestConnectionOutcome, TestStage,
};

/// Concrete transport. Empty struct — every operation takes the config it
/// needs as an argument, so a single instance is shared by every use case
/// and every active stream.
#[derive(Debug, Default, Clone, Copy)]
pub struct TungsteniteTransport;

impl TungsteniteTransport {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RealtimeTransport for TungsteniteTransport {
    async fn test_connection(
        &self,
        config: &EffectiveStreamConfig,
        on_progress: Box<dyn Fn(TestStage) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<TestConnectionOutcome, RealtimeError> {
        // The test is "did the creds + URL + channel cooperate well enough
        // for a single connect+join cycle to succeed". Any error inside
        // is returned as a typed `Failed(_)` so the UI can render a red
        // badge instead of a Tauri command rejection. `Cancelled` is a
        // typed `Failed(Cancelled)` for the same reason — the caller
        // doesn't have to distinguish "I cancelled it" from "it failed"
        // in error-handling code.
        match do_one_shot_test(config, on_progress.as_ref(), &cancel).await {
            Ok(()) => Ok(TestConnectionOutcome::Ok),
            Err(e) => Ok(TestConnectionOutcome::Failed(e)),
        }
    }

    async fn run_stream(
        &self,
        config: &EffectiveStreamConfig,
        mut sink: Box<dyn EventSink>,
        cancel: CancellationToken,
        on_status: Box<dyn Fn(StreamStatus) + Send + Sync>,
    ) -> Result<(), RealtimeError> {
        let wss_url = build_wss_url(&config.supabase_url, &config.api_key)?;
        let topic = topic_for_channel(&config.channel);

        // Outer reconnect loop. Mirrors `supabase.py:259-275`. A cancelled
        // token at any backoff point exits cleanly; otherwise we walk the
        // backoff schedule and try again.
        let mut backoff_idx = 0usize;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            match connect_and_drain(
                &wss_url,
                &config.api_key,
                &topic,
                &config.event_filter,
                sink.as_mut(),
                &cancel,
                &on_status,
            )
            .await
            {
                Ok(()) => return Ok(()), // Stop requested — clean exit.
                Err(err) => {
                    let wait_secs = BACKOFF_STEPS_SECS[backoff_idx];
                    on_status(StreamStatus::Reconnecting {
                        retry_in_secs: wait_secs,
                        reason: err.to_string(),
                    });
                    // Cancellable sleep so Stop during backoff doesn't
                    // wait the full 10s.
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(wait_secs)) => {}
                    }
                    backoff_idx = (backoff_idx + 1).min(BACKOFF_STEPS_SECS.len() - 1);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// One-shot test (Settings → "Test Connection")
// ---------------------------------------------------------------------------

async fn do_one_shot_test(
    config: &EffectiveStreamConfig,
    on_progress: &(dyn Fn(TestStage) + Send + Sync),
    cancel: &CancellationToken,
) -> Result<(), RealtimeError> {
    let wss_url = build_wss_url(&config.supabase_url, &config.api_key)?;
    let topic = topic_for_channel(&config.channel);
    let budget = Duration::from_secs(TEST_CONNECTION_BUDGET_SECS);
    let deadline = tokio::time::Instant::now() + budget;

    // ---- 1. Connect (no ws yet → nothing to clean up here) -----------------
    on_progress(TestStage::Connecting);
    let request = wss_url
        .into_client_request()
        .map_err(|e| RealtimeError::ConnectFailed(format!("invalid websocket URL: {e}")))?;
    let (mut ws, _resp) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(RealtimeError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(RealtimeError::Timeout { seconds: TEST_CONNECTION_BUDGET_SECS });
        }
        result = connect_async(request) => {
            result.map_err(|e| RealtimeError::ConnectFailed(format!("websocket connect failed: {e}")))?
        }
    };

    // From here on out, ws is open. EVERY exit must run the cleanup
    // below — phx_leave (if we joined) + Close — so the server can
    // reap the subscription immediately instead of waiting on the
    // heartbeat-timeout sweep. The `joined` flag tracks whether we
    // ever got a phx_reply{ok}; only then does phx_leave make sense.
    let join_ref = "1";
    let mut joined = false;
    let result = run_join_phase(
        &mut ws,
        &topic,
        join_ref,
        &config.api_key,
        on_progress,
        cancel,
        deadline,
        &mut joined,
    )
    .await;

    // ---- 4. Cleanup (always runs). Best-effort writes — a failure
    // here means the socket is already dead, which is fine.
    if joined {
        let leave = leave_envelope(&topic, join_ref, "leave");
        let _ = ws.send(Message::Text(leave.to_string())).await;
    }
    let _ = ws.send(Message::Close(None)).await;
    let _ = ws.close(None).await;

    result
}

/// Send `phx_join` and wait for `phx_reply{ok}`. Cancellation and the
/// deadline are interleaved with each await so the cleanup in
/// [`do_one_shot_test`] always gets to run — no future is dropped
/// mid-await past this point.
#[allow(clippy::too_many_arguments)]
async fn run_join_phase<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    topic: &str,
    join_ref: &str,
    api_key: &str,
    on_progress: &(dyn Fn(TestStage) + Send + Sync),
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    joined_out: &mut bool,
) -> Result<(), RealtimeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // ---- 2. Send phx_join -------------------------------------------------
    on_progress(TestStage::Joining);
    let join = join_envelope(topic, join_ref, api_key);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(RealtimeError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(RealtimeError::Timeout { seconds: TEST_CONNECTION_BUDGET_SECS });
        }
        r = ws.send(Message::Text(join.to_string())) => {
            r.map_err(|e| RealtimeError::ConnectFailed(format!("send phx_join failed: {e}")))?;
        }
    }

    // ---- 3. Drain frames until phx_reply{ok|error} -----------------------
    on_progress(TestStage::AwaitingReply);
    loop {
        let msg = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(RealtimeError::Cancelled),
            _ = tokio::time::sleep_until(deadline) => {
                return Err(RealtimeError::Timeout { seconds: TEST_CONNECTION_BUDGET_SECS });
            }
            m = ws.next() => m,
        };
        let msg = msg
            .ok_or_else(|| RealtimeError::ConnectFailed("server closed before phx_reply".into()))?
            .map_err(|e| RealtimeError::ConnectFailed(format!("websocket recv error: {e}")))?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                return Err(RealtimeError::ConnectFailed(
                    "server closed the connection".into(),
                ))
            }
            Message::Frame(_) => continue,
        };
        let frame: Value = serde_json::from_str(&text)
            .map_err(|e| RealtimeError::Io(format!("server sent non-JSON: {e}")))?;
        match parse_join_reply(&frame, join_ref) {
            Some(JoinReply::Ok) => {
                *joined_out = true;
                return Ok(());
            }
            Some(JoinReply::Error(reason)) => {
                return Err(RealtimeError::ChannelRejected { reason });
            }
            None => continue,
        }
    }
}

/// Replace the `apikey=<JWT>` query value with `apikey=<redacted>`.
/// Currently unused after the diagnostic info-logging was stripped;
/// kept around because future error-path logging will want to surface
/// the URL without leaking the JWT.
#[allow(dead_code)]
fn redact_apikey(url: &str) -> String {
    let Some((prefix, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    for kv in query.split('&') {
        if let Some(rest) = kv.strip_prefix("apikey=") {
            if rest.is_empty() {
                parts.push("apikey=".into());
            } else {
                parts.push("apikey=<redacted>".into());
            }
        } else {
            parts.push(kv.to_string());
        }
    }
    format!("{prefix}?{}", parts.join("&"))
}

// ---------------------------------------------------------------------------
// Long-lived drive loop (LiveScan → "▶ Start")
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn connect_and_drain(
    wss_url: &str,
    api_key: &str,
    topic: &str,
    event_filter: &str,
    sink: &mut dyn EventSink,
    cancel: &CancellationToken,
    on_status: &(dyn Fn(StreamStatus) + Send + Sync),
) -> Result<(), RealtimeError> {
    let request = wss_url
        .into_client_request()
        .map_err(|e| RealtimeError::ConnectFailed(format!("invalid websocket URL: {e}")))?;
    let (mut ws, _) = connect_async(request)
        .await
        .map_err(|e| RealtimeError::ConnectFailed(format!("websocket connect failed: {e}")))?;

    // Send phx_join but don't wait for the reply — the Python publisher
    // also doesn't (`supabase.py:285-288`). If the channel is rejected,
    // the next send will raise and the outer loop will reconnect.
    let join_ref = "1";
    let join = join_envelope(topic, join_ref, api_key);
    ws.send(Message::Text(join.to_string()))
        .await
        .map_err(|e| RealtimeError::ConnectFailed(format!("send phx_join failed: {e}")))?;

    on_status(StreamStatus::Connected);

    let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_SECS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick fires immediately — skip so we don't ping before the
    // join has had time to land.
    let _ = heartbeat.tick().await;

    let mut heartbeat_ref: u64 = 2; // join used "1"

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Phoenix-level graceful leave before tearing down the
                // socket. Stops the server from holding the broadcast
                // subscription open until heartbeat timeout (~60s) on
                // every Stop click.
                let leave = leave_envelope(topic, join_ref, "leave");
                let _ = ws.send(Message::Text(leave.to_string())).await;
                let _ = ws.close(None).await;
                return Ok(());
            }
            _ = heartbeat.tick() => {
                heartbeat_ref += 1;
                let env = heartbeat_envelope(&heartbeat_ref.to_string());
                ws.send(Message::Text(env.to_string()))
                    .await
                    .map_err(|e| RealtimeError::ConnectFailed(format!("send heartbeat failed: {e}")))?;
            }
            msg = ws.next() => {
                let msg = msg
                    .ok_or_else(|| RealtimeError::ConnectFailed("server closed the connection".into()))?
                    .map_err(|e| RealtimeError::ConnectFailed(format!("websocket recv error: {e}")))?;
                if let Some(payload) = handle_frame(msg, topic, event_filter) {
                    sink.append(&payload).await?;
                }
            }
        }
    }
}

/// Convert one inbound message into `Some(payload)` if it's a broadcast
/// we want to forward, `None` otherwise (heartbeat replies, presence,
/// non-JSON, ping/pong). The payload is *owned* JSON because the
/// transport doesn't keep the frame alive past this call.
fn handle_frame(msg: Message, topic: &str, event_filter: &str) -> Option<Value> {
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => return None,
        Message::Close(_) | Message::Frame(_) => return None,
    };
    let frame: Value = serde_json::from_str(&text).ok()?;
    extract_broadcast_payload(&frame, topic, event_filter).cloned()
}
