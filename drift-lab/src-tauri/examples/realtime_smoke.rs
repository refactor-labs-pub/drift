//! Standalone smoke test for the Supabase Realtime subscribe path.
//!
//! **Crucially, this binary calls the EXACT same code the Tauri app
//! does** — [`drift_lab_lib::realtime::infra::TungsteniteTransport`]
//! and the [`drift_lab_lib::realtime::app::TestConnectionUseCase`].
//! There is no parallel implementation to drift out of sync. If this
//! example connects+joins+leaves cleanly, the desktop app's "Test
//! Connection" button will too. If it fails, you'll see the exact same
//! failure the user would see — without Tauri / IPC / the renderer in
//! the way.
//!
//! ## How to run
//!
//! From `drift-lab/`:
//!
//! ```bash
//! make realtime-smoke
//! ```
//!
//! That target sources `drift-lab/.env` (gitignored) and invokes us via
//! `cargo run -p drift-lab --example realtime_smoke`. Required env:
//!
//! ```text
//! SUPABASE_URL                 = https://abc123.supabase.co
//! SUPABASE_REALTIME_API_KEY    = <JWT>
//! SUPABASE_REALTIME_CHANNEL    = drift-profiler-events    # optional
//! ```
//!
//! ## What it does
//!
//! Builds a fake [`SettingsRepository`] + [`ApiKeyVault`] from env
//! vars (so the production use case path is exercised end-to-end),
//! runs ONE `test_connection`, and prints staged progress + the final
//! result. Mirrors the Settings tab's "Test Connection" flow byte for
//! byte, minus the Tauri command shim.

use std::env;
use std::process::ExitCode;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use drift_lab_lib::realtime::{
    app::{TestConnectionUseCase, TestInputs},
    domain::{RealtimeConfig, RealtimeError},
    infra::TungsteniteTransport,
    ports::{ApiKeyVault, TestConnectionOutcome, TestStage},
};

// ----- tiny in-process vault (no AppHandle / SecretStore needed) ----------

/// API-key vault that returns a fixed value taken from env. Same trait
/// the production `FileApiKeyVault` implements — the use case can't tell
/// the difference.
struct EnvVault {
    key: String,
}

impl ApiKeyVault for EnvVault {
    fn read(&self) -> Result<String, RealtimeError> {
        if self.key.is_empty() {
            Err(RealtimeError::MissingApiKey)
        } else {
            Ok(self.key.clone())
        }
    }
}

// ----- ANSI helpers --------------------------------------------------------

fn use_color() -> bool {
    // Honor the universal NO_COLOR convention. We don't try to detect a
    // TTY — cargo run already buffers stdout, so the practical test is
    // "is the user in a CI pipeline" (NO_COLOR=1) or not.
    env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, s: &str) -> String {
    if use_color() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn stage_line(label: &str) {
    println!("{} {label}", paint("1;34", "▶"));
}

fn ok_line(label: &str) {
    println!("{} {label}", paint("1;32", "✓"));
}

fn fail_line(label: &str) {
    println!("{} {label}", paint("1;31", "✗"));
}

fn dim(s: &str) -> String {
    paint("2", s)
}

// ----- main ----------------------------------------------------------------

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Mirror the desktop app's startup: install the rustls
    // CryptoProvider before any TLS work. Same code path as
    // `lib::run()`. Without this, the first connect_async panics.
    if let Err(e) = drift_lab_lib::realtime::init() {
        eprintln!("realtime init failed: {e}");
        return ExitCode::from(2);
    }

    let url = env::var("SUPABASE_URL").unwrap_or_default();
    let key = env::var("SUPABASE_REALTIME_API_KEY").unwrap_or_default();
    let channel = env::var("SUPABASE_REALTIME_CHANNEL").unwrap_or_default();

    if url.trim().is_empty() {
        fail_line("SUPABASE_URL is not set (check drift-lab/.env)");
        return ExitCode::from(1);
    }
    if key.trim().is_empty() {
        fail_line("SUPABASE_REALTIME_API_KEY is not set (check drift-lab/.env)");
        return ExitCode::from(1);
    }

    let effective_channel = if channel.trim().is_empty() {
        drift_lab_lib::realtime::domain::defaults::DEFAULT_CHANNEL.to_string()
    } else {
        channel.trim().to_string()
    };

    println!("{}", dim(&format!("target:  {url} (apikey redacted)")));
    println!("{}", dim(&format!("topic:   realtime:{effective_channel}")));
    println!(
        "{}",
        dim(&format!(
            "budget:  {}s",
            drift_lab_lib::realtime::domain::defaults::TEST_CONNECTION_BUDGET_SECS
        ))
    );
    println!();

    // Wire the SAME adapters + use case the desktop app uses. Use case
    // now takes a resolved `RealtimeConfig` value directly (no
    // repository indirection) — we build one from env vars here, the
    // desktop command projects the active profile.
    let settings = RealtimeConfig {
        url: url.clone(),
        default_channel: effective_channel.clone(),
        ..Default::default()
    };
    let vault = EnvVault { key: key.clone() };
    let transport = TungsteniteTransport::new();
    let use_case = TestConnectionUseCase::new(settings, &vault, &transport);

    // Progress callback: just prints to stdout. The production wiring
    // emits these on the Tauri event bus; here we render them inline.
    let on_progress: Box<dyn Fn(TestStage) + Send + Sync> = Box::new(|stage| match stage {
        TestStage::Connecting => stage_line("Connecting (WSS handshake)…"),
        TestStage::Joining => stage_line("Sending phx_join…"),
        TestStage::AwaitingReply => stage_line("Awaiting phx_reply{ok}…"),
    });

    // Cancellation hooked up but unused — Ctrl-C handling would be a
    // nice-to-have. Today the user can just let the 5 s budget expire.
    let cancel = CancellationToken::new();
    // Keep one clone alive so the token doesn't get dropped early; once
    // we add Ctrl-C handling, this is where it'd land.
    let _cancel_handle = Arc::new(cancel.clone());

    let inputs = TestInputs {
        supabase_url: None, // already in the repo above
        api_key: None,      // already in the vault
        channel: None,      // use the saved one
    };

    let test_outcome = use_case.execute(inputs, on_progress, cancel).await;
    match test_outcome {
        Ok(TestConnectionOutcome::Ok) => {
            println!();
            ok_line(&format!(
                "SUCCESS — realtime reachable, JWT accepted, channel '{effective_channel}' joinable."
            ));
        }
        Ok(TestConnectionOutcome::Failed(e)) => {
            println!();
            fail_line(&format!("FAILED: {e}"));
            return ExitCode::from(1);
        }
        Err(e) => {
            println!();
            fail_line(&format!("FAILED (unconfigured): {e}"));
            return ExitCode::from(1);
        }
    }

    // ----- end-to-end pub+sub self-test (default ON) -----
    // Opens TWO WSS connections to the same channel:
    //   * subscriber: joins, waits for one broadcast, runs it through
    //                 the production `Aggregator::ingest_value`, asserts
    //                 the aggregator picked up at least one event.
    //   * publisher: joins on a separate socket and sends ONE broadcast
    //                with the exact shape the Python publisher emits.
    // Catches every silent-drop bug the desktop chart would also hit —
    // including the "integer time" parse failure that bit us originally.
    // Disable via SUPABASE_REALTIME_SKIP_SELF_TEST=1.
    if env::var("SUPABASE_REALTIME_SKIP_SELF_TEST").is_err() {
        println!();
        stage_line("Pub+sub self-test: publish one event, receive, aggregate, assert…");
        if let Err(e) = pub_sub_self_test(&url, &key, &effective_channel).await {
            fail_line(&format!("self-test FAILED: {e}"));
            return ExitCode::from(1);
        }
        ok_line("Pub+sub round-trip + aggregator parse: OK");
    }

    // ----- optional: subscribe + dump raw frames for N seconds -----
    // Set `SUPABASE_REALTIME_LISTEN_SECONDS=10` in your .env to also
    // listen on the channel for N seconds, printing every raw frame so
    // you can see what an external publisher actually sends.
    let listen_secs: u64 = env::var("SUPABASE_REALTIME_LISTEN_SECONDS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if listen_secs == 0 {
        return ExitCode::SUCCESS;
    }
    println!();
    stage_line(&format!(
        "Listening on '{effective_channel}' for {listen_secs}s — printing every frame…"
    ));
    if let Err(e) = listen_and_dump_frames(&url, &key, &effective_channel, listen_secs).await {
        fail_line(&format!("listen failed: {e}"));
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

// ===========================================================================
// Pub + sub round-trip self-test
// ===========================================================================
//
// Why: the publisher's `time` was an integer of nanoseconds, but the
// aggregator's `RawEvent::time` was `Option<String>`. Serde failed
// silently, `ingest_value` swallowed the error, and the desktop chart
// sat on "Waiting for first broadcast" while events flowed.
//
// This test reproduces the full path Rust-side WITHOUT needing the
// Python publisher: we synthesize one broadcast with the exact wire
// shape the publisher emits, push it through `extract_broadcast_payload`
// (the same function the production transport uses), feed the inner
// payload into `Aggregator::ingest_value`, and assert the counter ticked.
// If this fails, the desktop UI would have failed too — fix the parser.

const SELF_TEST_BUDGET_SECS: u64 = 8;

async fn pub_sub_self_test(
    supabase_url: &str,
    api_key: &str,
    channel: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use drift_lab_lib::event_log::Aggregator;
    use drift_lab_lib::realtime::infra::protocol::{
        extract_broadcast_payload, join_envelope, leave_envelope, topic_for_channel,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::time::Duration;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };

    let topic = topic_for_channel(channel);

    // ---- subscriber: open WSS, join, wait for one matching broadcast ----
    let sub_url = drift_lab_lib::realtime::domain::url::build_wss_url(supabase_url, api_key)
        .map_err(|e| format!("url builder: {e}"))?;
    let sub_request = sub_url.clone().into_client_request()?;
    let (mut sub_ws, _) = connect_async(sub_request).await?;
    let join = join_envelope(&topic, "1", api_key);
    sub_ws.send(Message::Text(join.to_string())).await?;
    println!("  {} subscriber joined", dim("·"));

    // Wait for phx_reply{ok} on our join before we let the publisher
    // send — otherwise the broadcast could arrive before the server has
    // registered our subscription and we'd miss it.
    let join_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let msg = tokio::select! {
            _ = tokio::time::sleep_until(join_deadline) => {
                return Err("subscriber: join didn't ack within 3s".into());
            }
            m = sub_ws.next() => m,
        };
        let Some(msg) = msg else {
            return Err("subscriber: server closed before phx_reply".into());
        };
        if let Message::Text(t) = msg? {
            if let Ok(frame) = serde_json::from_str::<Value>(&t) {
                if frame.get("event").and_then(|x| x.as_str()) == Some("phx_reply")
                    && frame.get("ref").and_then(|x| x.as_str()) == Some("1")
                {
                    break;
                }
            }
        }
    }
    println!("  {} subscriber received phx_reply{{ok}}", dim("·"));

    // ---- publisher: open SECOND WSS, join, send one broadcast ----
    // Supabase Realtime's `broadcast.self=false` (we set this in the
    // join config) means a publisher does NOT receive its own
    // broadcasts. So we need two sockets.
    let pub_request = sub_url.into_client_request()?;
    let (mut pub_ws, _) = connect_async(pub_request).await?;
    pub_ws
        .send(Message::Text(join_envelope(&topic, "2", api_key).to_string()))
        .await?;

    // The inner payload — verbatim shape from the Python publisher
    // (integer ns `time`, snake_case fields, frames array). One frame
    // is enough to prove the parser accepts it AND produces a sample.
    let inner_payload = serde_json::json!({
        "count": 100,
        "cpu": 1.5,
        "duration_ns": 1_000_000_000_i64,
        "frames": [
            {
                "file": "/app/smoke_test.py",
                "is_native": false,
                "is_system": false,
                "language": "python",
                "line": 1,
                "module": "smoke_test",
                "name": "self_test_frame",
                "qualified_name": "smoke_test.self_test_frame"
            }
        ],
        "memory_bytes": 1_000_000_i64,
        "memory_peak_bytes": 1_000_000_i64,
        "period_ns": 10_000_000_i64,
        "pod": "self-test-pod",
        "service": "drift-lab-self-test",
        // Integer ns since epoch — the shape that caused the original
        // silent-drop. If RawTime no longer tolerates this, the
        // assertion below will fail loudly instead of silently.
        "time": 1_779_236_876_872_834_300_i64,
        "type": "wall_trace",
    });
    let broadcast = serde_json::json!({
        "topic": topic,
        "event": "broadcast",
        "join_ref": "2",
        "ref": "3",
        "payload": {
            "type": "broadcast",
            "event": "profiler-event",
            "payload": inner_payload,
        }
    });
    pub_ws
        .send(Message::Text(broadcast.to_string()))
        .await?;
    println!("  {} publisher sent broadcast", dim("·"));

    // ---- subscriber: read until our broadcast arrives ----
    let mut aggregator = Aggregator::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(SELF_TEST_BUDGET_SECS);
    let got_broadcast;
    loop {
        let msg = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                return Err(format!(
                    "self-test: no broadcast received within {SELF_TEST_BUDGET_SECS}s"
                ).into());
            }
            m = sub_ws.next() => m,
        };
        let Some(msg) = msg else {
            return Err("subscriber socket closed before broadcast arrived".into());
        };
        let Message::Text(t) = msg? else { continue };
        let Ok(frame) = serde_json::from_str::<Value>(&t) else { continue };
        // The SAME extraction the production transport uses. If this
        // returns None, our broadcast was filtered before the
        // aggregator saw it — and the same filter would drop it in
        // the desktop app.
        if let Some(payload) = extract_broadcast_payload(&frame, &topic, "profiler-event") {
            aggregator.ingest_value(payload);
            got_broadcast = true;
            break;
        }
    }
    let _ = got_broadcast; // assert below; suppress dead-store warning

    // Clean up both sockets — phx_leave then close.
    let _ = sub_ws
        .send(Message::Text(leave_envelope(&topic, "1", "leave").to_string()))
        .await;
    let _ = sub_ws.close(None).await;
    let _ = pub_ws
        .send(Message::Text(leave_envelope(&topic, "2", "leave").to_string()))
        .await;
    let _ = pub_ws.close(None).await;

    // ---- assert the production aggregator picked it up ----
    if !got_broadcast {
        return Err("self-test: matching broadcast never arrived".into());
    }
    if aggregator.total_events() != 1 {
        return Err(format!(
            "self-test: aggregator.total_events()={}, expected 1. \
             The broadcast arrived but Aggregator::ingest_value dropped it — \
             the publisher's wire shape doesn't match RawEvent.",
            aggregator.total_events()
        )
        .into());
    }
    let report = aggregator.snapshot("smoke-self-test");
    if report.total_calls == 0 {
        return Err(
            "self-test: aggregator ingested the event but produced 0 samples — \
             frames may not be parsing"
                .into(),
        );
    }
    println!(
        "  {} aggregator: total_events={}, total_calls={}, services={}",
        dim("·"),
        aggregator.total_events(),
        report.total_calls,
        report.services.join(",")
    );
    Ok(())
}

// ===========================================================================
// Subscribe + dump every raw frame
// ===========================================================================

/// Open a WSS, join the channel, and print every inbound frame for
/// `listen_secs` seconds. Bespoke (doesn't go through the production
/// transport) because we want to see EVERYTHING — including frames the
/// production code would silently filter (heartbeat replies, presence
/// diffs, broadcasts whose inner event doesn't match the filter). That
/// way the user can hand me the log and we can see exactly what the
/// publisher emits and decide whether the production pipeline is
/// dropping it correctly or by mistake.
async fn listen_and_dump_frames(
    supabase_url: &str,
    api_key: &str,
    channel: &str,
    listen_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::time::Duration;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };

    let wss_url = drift_lab_lib::realtime::domain::url::build_wss_url(supabase_url, api_key)
        .map_err(|e| format!("url builder: {e}"))?;
    let topic = format!("realtime:{channel}");
    let request = wss_url
        .into_client_request()
        .map_err(|e| format!("client_request: {e}"))?;
    let (mut ws, _) = connect_async(request)
        .await
        .map_err(|e| format!("connect_async: {e}"))?;

    // phx_join — same envelope the production code sends.
    let join = serde_json::json!({
        "topic": topic,
        "event": "phx_join",
        "ref": "1",
        "join_ref": "1",
        "payload": {
            "config": {
                "broadcast": { "ack": false, "self": false },
                "presence":  { "key": "" },
                "postgres_changes": [],
                "private": false
            },
            "access_token": api_key
        }
    });
    ws.send(Message::Text(join.to_string())).await?;

    // Frame counters per category — printed in the summary.
    let mut total: u32 = 0;
    let mut on_topic: u32 = 0;
    let mut broadcasts: u32 = 0;
    let mut heartbeat_replies: u32 = 0;
    let mut other: u32 = 0;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(listen_secs);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let _ = heartbeat.tick().await; // skip immediate fire
    let mut heartbeat_ref: u64 = 2;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = heartbeat.tick() => {
                heartbeat_ref += 1;
                let hb = serde_json::json!({
                    "topic": "phoenix",
                    "event": "heartbeat",
                    "payload": {},
                    "ref": heartbeat_ref.to_string(),
                });
                ws.send(Message::Text(hb.to_string())).await?;
            }
            msg = ws.next() => {
                let Some(msg) = msg else { break };
                let msg = msg?;
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Binary(b) => {
                        println!("{} binary frame ({} bytes) — skipping", dim("·"), b.len());
                        continue;
                    }
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(c) => {
                        println!("{} server closed: {:?}", dim("·"), c);
                        break;
                    }
                    Message::Frame(_) => continue,
                };
                total += 1;
                // Categorise + print. Pretty-print parsed JSON so the user
                // can see the structure; fall back to raw text if it's not
                // valid JSON.
                match serde_json::from_str::<Value>(&text) {
                    Ok(v) => {
                        let topic_field = v.get("topic").and_then(|x| x.as_str()).unwrap_or("?");
                        let event_field = v.get("event").and_then(|x| x.as_str()).unwrap_or("?");
                        let category = if topic_field == "phoenix" && event_field == "phx_reply" {
                            heartbeat_replies += 1;
                            "heartbeat_reply"
                        } else if topic_field == topic {
                            on_topic += 1;
                            if event_field == "broadcast" {
                                broadcasts += 1;
                                "broadcast"
                            } else {
                                event_field
                            }
                        } else {
                            other += 1;
                            "other"
                        };
                        println!(
                            "{} #{total:03} {} topic={} event={}",
                            paint("1;36", "←"),
                            paint("2", &format!("[{category}]")),
                            topic_field,
                            event_field
                        );
                        match serde_json::to_string_pretty(&v) {
                            Ok(pretty) => {
                                // Indent each line so the JSON is visually
                                // grouped under the header.
                                for line in pretty.lines() {
                                    println!("    {line}");
                                }
                            }
                            Err(_) => println!("    {text}"),
                        }
                    }
                    Err(e) => {
                        println!("{} #{total:03} NON-JSON frame: {e}", paint("1;31", "←"));
                        println!("    {text}");
                    }
                }
            }
        }
    }

    // Best-effort phx_leave + Close.
    let leave = serde_json::json!({
        "topic": topic, "event": "phx_leave",
        "join_ref": "1", "ref": "leave", "payload": {},
    });
    let _ = ws.send(Message::Text(leave.to_string())).await;
    let _ = ws.close(None).await;

    println!();
    println!("{}", paint("1;34", "─── listen summary ─────────────────────────"));
    println!("  total frames received:      {total}");
    println!("  on our topic ({topic}): {on_topic}");
    println!("  broadcasts on our topic:    {broadcasts}");
    println!("  heartbeat phx_replies:      {heartbeat_replies}");
    println!("  other (off-topic, etc.):    {other}");
    if broadcasts == 0 && listen_secs > 0 {
        println!();
        println!(
            "{} no broadcasts arrived on '{channel}' during the {listen_secs}s window.",
            paint("1;33", "!")
        );
        println!(
            "  Check: is your Python publisher running and pointing at the same project +"
        );
        println!(
            "  channel? Its SUPABASE_REALTIME_CHANNEL must equal '{channel}'."
        );
    }
    Ok(())
}
