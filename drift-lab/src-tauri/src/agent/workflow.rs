//! Agent-driven scan workflow — bridges `AgentEvent`s into the 6-step
//! timeline the UI renders.
//!
//! The deterministic [`crate::workflow`] runs hardcoded sleeps. This module
//! does the same dance but lets the LLM pick the tools, and surfaces the
//! model's prose as the step `detail` so the UI shows *why* the agent did
//! what it did.
//!
//! UI stages (must match `desktop-ui/src/store/runStore.ts` `DEFAULT_STEPS`):
//!
//!   index 0 → Understanding code
//!             — list_directory / read_file_excerpt / discover_project
//!   index 1 → Locating how to run
//!             — check_docker / find_image / (any CI-config read counts here too)
//!   index 2 → Setting up runtime
//!             — ensure_image / detect_runtime / (run_on_host fallback when wired)
//!   index 3 → Running + profiling
//!             — find_test_runner_for_profiling / install_profiler / drive_load /
//!               run_profiling
//!   index 4 → Building thesis
//!             — analyze_samples + the LLM synthesis turn
//!   index 5 → Reporting
//!             — final RunReport / RunComplete emission
//!
//! The agent's internal *recipe* (in `build_goal_prompt`) is 10 steps long
//! and finer-grained than this — the UI just bundles related sub-steps so
//! the operator gets a "what's happening now" overview without a wall of
//! ticks. Mid-stage prose flows through the ReasoningLog stream on the left.
//!
//! The mapping is *advisory* — the loop accepts the model calling tools out
//! of order, it just emits step events under whichever index the most-recent
//! tool maps to. This keeps the UI honest if the model recovers from an
//! error by re-running an earlier stage.

use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_loop::{Agent, AgentEvent};
use super::provider::Provider;
use super::types::Message;

/// Static playbooks the agent reads on every run. Edit the MD files under
/// `drift-lab/agent-skills/` — `include_str!` bakes them into the binary so
/// they ship with the app, no runtime read needed.
///
/// **Order matters**. Project-orientation comes first: until the agent knows
/// where the canonical Dockerfile lives (often *not* at the root in
/// monorepos), `find_image` will return "not found" and the language-detection
/// playbook can't help.
const PROJECT_ORIENTATION_SKILL: &str =
    include_str!("../../../agent-skills/project-orientation.md");
const LANGUAGE_DETECTION_SKILL: &str =
    include_str!("../../../agent-skills/language-detection.md");
use crate::events::{
    RunComplete, RunError, RunReport, StepStatus, StepUpdate, TelemetrySample, VisibilityMap,
};
use crate::telemetry;
use crate::tools::analyze_samples::{Issue, Severity};

/// Goal prompt fed to the agent when a scan starts. Always anchors on the
/// literal `project_path` the user picked and lists the **expected tool order**
/// so even small / non-tool-tuned local models can follow the recipe. An
/// optional `focus` line carries the user's preset / free-text prompt — it
/// scopes *what* to look for without replacing the path or recipe.
///
/// The earlier shape let the caller pass an opaque `goal_prompt` that
/// completely replaced the default — which meant picking any preset dropped
/// the project path from the prompt and the model would emit no tool calls.
/// Always-include-the-path is the contract now; `focus` is the only knob.
pub fn build_goal_prompt(project_path: &str, focus: Option<&str>) -> String {
    let focus_line = match focus.map(str::trim).filter(|s| !s.is_empty()) {
        Some(f) => format!("Focus for this run: {f}\n\n"),
        None => String::new(),
    };
    format!(
        "You are Drift Lab's profiling agent. Goal: understand the service at \
         `{project_path}` deeply enough to tell the operator exactly which \
         code paths are costing them money in the cloud — runtime, memory, \
         IO, DB.\n\n\
         {focus_line}\
         This is **not a demo**. Use the LLM (you) to actually understand \
         the project — read the code, follow the imports, reason about the \
         framework. Don't shotgun through tools. If at any stage you can't \
         make progress, call `ask_user` with a concrete question and wait \
         for the answer rather than guessing.\n\n\
         ## The 10-step recipe\n\n\
         1. **Understand the code.** `list_directory` at \"{project_path}\" \
         once, then `read_file_excerpt` on the manifest (`package.json` / \
         `Cargo.toml` / `pyproject.toml` / `go.mod` / `pom.xml`). Run \
         `discover_project` (path: \"{project_path}\") to confirm language \
         + scripts. Skim a couple of entrypoint files so you know what the \
         service actually does. The orientation + language playbooks in \
         the system prompt are mandatory reading.\n\
         2. **Check existing profiling setup.** Look for already-wired \
         observability: `pyroscope`, `parca`, `prometheus`, `grafana`, \
         `sentry`, `datadog`, `newrelic`, or a `profiling/` / `metrics/` \
         directory. If something is already there, plan to read its output \
         instead of installing our own. Otherwise we'll inject a profiler \
         in step 6.\n\
         3. **Find how to run it.** `check_docker` first (if Docker isn't \
         reachable, stop and report the `hint` verbatim). Then check CI \
         configs (`.github/workflows/*.yml`, `Makefile`, etc.) for the \
         canonical Docker build command — `docker build -f <path>` tells \
         you which Dockerfile is the production one. Call `find_image` \
         with the directory that contains that Dockerfile.\n\
         4. **No Dockerfile? Run locally instead.** If no Dockerfile \
         exists anywhere in the project (CI doesn't reference one, none \
         in any subdir), `ask_user` for the exact command to start the \
         service (\"how do you usually run this?\") and skip ahead to \
         step 6. Don't fabricate a Dockerfile.\n\
         5. **Build the Docker image.** `ensure_image` with the fields \
         from `find_image` plus `project_path: \"{project_path}\"`. Use \
         the `resolved_image` from the result going forward (not the \
         original `image_ref`).\n\
         6. **Run + watch telemetry.** `detect_runtime` on the resolved \
         image, `find_test_runner_for_profiling` to pick what to exercise, \
         `install_profiler` to inject py-spy / async-profiler / perf. The \
         telemetry sidecar (CPU, memory, IO) starts streaming \
         automatically as soon as a tool returns a `container_id`.\n\
         7. **Capture + parse the profile.** `drive_load` if profiling a \
         running server, or `run_profiling` if exercising a test. Then \
         `analyze_samples` on the resulting sample file to get ranked \
         hotspots with categories (db / network / cpu / lock / gc / \
         serde / filesystem).\n\
         8. **Build a thesis.** With the ranked issues + the live \
         telemetry shape in hand, explain in plain prose what's clogging \
         the runtime: \"this service is DB-bound — 35% in `psycopg.execute` \
         under `/checkout` — but the GC pressure is also non-trivial at \
         12%\". Don't just list — reason.\n\
         9. **Suggest hotspots → money.** For each critical / high issue, \
         tie it to actual cloud cost: \"this 30% CPU on JSON serialisation \
         means a 30% over-provisioning on the worker tier\". The user \
         cares about $$, not %%. The synthesis turn at the end of the \
         run takes care of the JSON-formatted advice; your job in this \
         step is to phrase the per-issue takeaway.\n\
         10. **Summarise.** Two or three sentences for the operator: what \
         the service is, what's worst, what's worth fixing first. No \
         further tool calls after this.\n\n\
         ## Stuck? Use `ask_user`\n\n\
         If a stage doesn't progress — multiple Dockerfiles and CI \
         doesn't pick one, the test runner crashes for an unobvious \
         reason, the image won't build, the profiler can't attach — \
         call `ask_user` with a specific question. Don't guess and don't \
         silently bail. Example questions:\n\
         - \"I found `Dockerfile.dev` and `Dockerfile.prod` — which one \
         should I profile?\"\n\
         - \"The test suite has 200 tests; which one represents your \
         hottest production path?\"\n\
         - \"py-spy needs `sys_ptrace` — can I run the container with \
         `--cap-add SYS_PTRACE`?\"\n\n\
         Before each tool call, write ONE short sentence explaining what \
         you expect to find. If a tool errors, explain the failure in \
         prose and try the next best path. Always finish with the step-10 \
         summary — no further tool calls after."
    )
}

/// Back-compat shim. New code should call [`build_goal_prompt`] directly.
pub fn default_goal_prompt(project_path: &str) -> String {
    build_goal_prompt(project_path, None)
}

/// Prompts the user can pick from on the home screen. Keep these short — the
/// LLM gets the project path appended automatically by the workflow.
pub const PROMPT_PRESETS: &[(&str, &str)] = &[
    (
        "Profile a slow endpoint",
        "Profile this service end-to-end. Drive load against its main HTTP \
         endpoints and identify the slowest functions. Focus on database, \
         network, and serialisation hotspots.",
    ),
    (
        "Profile a specific test",
        "Investigate the project's test suite, pick the single most relevant \
         test for performance analysis, and run it under the profiler. Report \
         which functions dominate that test's runtime.",
    ),
    (
        "Find startup-time bottlenecks",
        "Profile the service from cold start. Identify which modules and \
         functions dominate the first 5 seconds of execution — module \
         loading, dependency injection, schema validation, etc.",
    ),
];

/// Translate a tool name into the timeline index it should appear under.
/// Returns `None` for tools that shouldn't advance the timeline (notably
/// `ask_user`, which is a meta-tool: it pauses the workflow waiting for a
/// human answer, but the current stage shouldn't tick over while we wait).
pub fn tool_to_step_index(name: &str) -> Option<usize> {
    match name {
        // Stage 0 — Understanding code. The agent skims the project layout,
        // CI configs, and manifests via these read-only tools.
        "list_directory" | "read_file_excerpt" | "discover_project" => Some(0),
        // Stage 1 — Locating how to run. Docker availability + image
        // discovery. `check_docker` lives here so a "not installed" status
        // surfaces under a meaningful banner rather than under stage 0.
        "check_docker" | "find_image" => Some(1),
        // Stage 2 — Setting up runtime. The image is present and we know
        // what's inside it.
        "ensure_image" | "detect_runtime" => Some(2),
        // Stage 3 — Running + profiling. Picking the right test or load,
        // injecting the profiler, capturing samples.
        "find_test_runner_for_profiling"
        | "install_profiler"
        | "drive_load"
        | "run_profiling" => Some(3),
        // Stage 4 — Building thesis. `analyze_samples` parses the raw
        // profile; the workflow's post-loop synthesis turn then extends
        // this stage with the LLM's architecture summary.
        "analyze_samples" => Some(4),
        // `ask_user` is a meta-tool — it parks the agent on a question.
        // We don't want to tick the timeline forward while we wait; the
        // BlockedModal is the user-visible signal instead.
        "ask_user" => None,
        _ => None,
    }
}

/// Sink for events produced while the workflow runs. Production wiring sends
/// them over Tauri; tests collect them into a vector.
pub trait WorkflowSink: Send + Sync {
    fn emit_step(&self, update: StepUpdate);
    fn emit_complete(&self, complete: RunComplete);
    fn emit_error(&self, error: RunError);

    /// Mirror every raw `AgentEvent` the loop produced, so the UI can render
    /// a streaming "what the agent is thinking + doing right now" log
    /// alongside the coarse step timeline. Default is a no-op so existing
    /// sinks (and tests that don't care) keep compiling.
    fn emit_agent_event(&self, _event: &AgentEvent) {}

    /// Forward one container-telemetry snapshot. Emitted at ~2 Hz once a tool
    /// produced a `container_id`; ignored before then.
    fn emit_telemetry(&self, _sample: TelemetrySample) {}

    /// Final structured "visibility map" — issues bucketed by severity plus
    /// the LLM's architecture advice. Emitted just before [`Self::emit_complete`]
    /// when the run produced an `analyze_samples` result.
    fn emit_report(&self, _report: RunReport) {}
}

/// In-process sink that accumulates events into a `Vec`. Used by tests and
/// could be reused for replay/debugging.
#[derive(Default)]
pub struct CaptureSink {
    pub events: std::sync::Mutex<Vec<CapturedEvent>>,
}

#[derive(Debug, Clone)]
pub enum CapturedEvent {
    Step(StepUpdate),
    Complete(RunComplete),
    Error(RunError),
    /// Raw agent event — surfaced so tests can assert against the streaming
    /// reasoning, not just the coarse step events.
    Agent(AgentEvent),
    Telemetry(TelemetrySample),
    Report(RunReport),
}

impl WorkflowSink for CaptureSink {
    fn emit_step(&self, update: StepUpdate) {
        self.events.lock().unwrap().push(CapturedEvent::Step(update));
    }
    fn emit_complete(&self, complete: RunComplete) {
        self.events.lock().unwrap().push(CapturedEvent::Complete(complete));
    }
    fn emit_error(&self, error: RunError) {
        self.events.lock().unwrap().push(CapturedEvent::Error(error));
    }
    fn emit_agent_event(&self, event: &AgentEvent) {
        self.events.lock().unwrap().push(CapturedEvent::Agent(event.clone()));
    }
    fn emit_telemetry(&self, sample: TelemetrySample) {
        self.events.lock().unwrap().push(CapturedEvent::Telemetry(sample));
    }
    fn emit_report(&self, report: RunReport) {
        self.events.lock().unwrap().push(CapturedEvent::Report(report));
    }
}

impl CaptureSink {
    pub fn snapshot(&self) -> Vec<CapturedEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// Inputs for one workflow run.
pub struct RunRequest {
    pub run_id: String,
    pub project_path: String,
    pub provider: Arc<dyn Provider>,
    pub mode: super::tools::Mode,
    /// Override the goal prompt. Default produced by [`default_goal_prompt`].
    pub goal_prompt: Option<String>,
}

/// Drive the agent through a profiling scan, mapping `AgentEvent`s to
/// `StepUpdate`s on `sink`. Returns when the agent emits `Done`,
/// `TurnBudgetExceeded`, `Error`, or the cancel token fires.
///
/// Two side-channels feed the UI alongside the step timeline:
///   * **telemetry** — once a tool reports a `container_id`, a sampler task
///     (see [`crate::telemetry`]) pushes ~2 Hz snapshots through an mpsc;
///     this loop forwards each onto [`WorkflowSink::emit_telemetry`].
///   * **visibility map** — when the run completes cleanly *and*
///     `analyze_samples` produced issues, a final non-streaming LLM turn
///     synthesises 3-5 architecture-advice bullets; the result is emitted
///     via [`WorkflowSink::emit_report`] just before `RunComplete`.
pub async fn run<S: WorkflowSink>(
    req: RunRequest,
    sink: &S,
    cancel: CancellationToken,
) -> Result<(), String> {
    // Always build the full recipe with the project_path anchored in. The
    // user's preset/free-text — if any — folds in as a Focus: line. Replacing
    // the recipe outright (the old behaviour) made the model emit no tool
    // calls because it never saw which directory to investigate.
    let goal = build_goal_prompt(&req.project_path, req.goal_prompt.as_deref());

    tracing::info!(
        target: "drift::workflow",
        run_id = %req.run_id,
        project_path = %req.project_path,
        mode = ?req.mode,
        goal_len = goal.len(),
        "scan workflow starting"
    );

    // System message inside the agent. Goal goes into the user message — that
    // way the model sees it as the explicit instruction to act on. We also
    // repeat the project_path in the system message because some smaller
    // models drop user-message arguments when invoking tools; having the path
    // in the system prompt makes "default to scanning <path>" the safe choice.
    //
    // The orientation + language skills are appended verbatim. They're
    // deliberately long: smaller / non-tool-tuned models otherwise burn 5+
    // calls before they find a Dockerfile in a non-trivial layout, or pick
    // the wrong one (Dockerfile.dev vs the canonical CI build). Loading them
    // once per session is cheap and dramatically tightens early-stage tool
    // use.
    let system = format!(
        "You are an embedded profiling agent operating on the project at \
         `{}`. Use only the provided tools. Call at least `discover_project` \
         or `find_image` before answering — never reply with a final summary \
         without having investigated. Keep prose short. Always finish with \
         a final summary.\n\n\
         ---\n\n{}\n\n---\n\n{}",
        req.project_path,
        PROJECT_ORIENTATION_SKILL,
        LANGUAGE_DETECTION_SKILL
    );

    // Clone the provider so the synthesis turn after the stream loop has a
    // handle. `Agent::new` moves the Arc, so we save one for ourselves.
    let synth_provider = req.provider.clone();
    let agent = Agent::new(req.provider, system).with_mode(req.mode);

    let mut tracker = StepTracker::new(req.run_id.clone());
    let mut stream = Box::pin(agent.reply(goal, vec![], cancel.clone()));

    // Telemetry channel. The sampler task pushes snapshots; the select loop
    // below forwards them. We hand out one cancel token to the sampler so
    // dropping the workflow drops the docker-stats poller too.
    let (telem_tx, mut telem_rx) = mpsc::unbounded_channel::<TelemetrySample>();
    let telem_cancel = CancellationToken::new();
    let mut sampler_container: Option<String> = None;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                tracing::info!(target: "drift::workflow", run_id = %req.run_id, "cancelled by user");
                break;
            }
            Some(sample) = telem_rx.recv() => {
                sink.emit_telemetry(sample);
            }
            item = stream.next() => {
                let Some(item) = item else { break };
                let event = match item {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!(
                            target: "drift::workflow",
                            run_id = %req.run_id,
                            error = %e,
                            "provider/transport error — surfacing to UI and exiting"
                        );
                        sink.emit_error(RunError {
                            run_id: req.run_id.clone(),
                            message: e.to_string(),
                        });
                        telem_cancel.cancel();
                        return Err(e.to_string());
                    }
                };
                // Mirror raw event to the sink BEFORE absorbing — the UI's
                // reasoning log wants to see deltas in real time, not after
                // we collapse them into step updates.
                sink.emit_agent_event(&event);
                log_agent_event(&req.run_id, &event);

                // Lazily start the telemetry sampler the first time any tool
                // returns a container_id. Tools that exec inside the target
                // container (install_profiler, drive_load, run_profiling,
                // exec_in_container) all surface one.
                if sampler_container.is_none() {
                    if let AgentEvent::ToolCompleted { content, is_error: false, .. } = &event {
                        if let Some(cid) = extract_container_id(content) {
                            tracing::info!(
                                target: "drift::workflow",
                                run_id = %req.run_id,
                                container_id = %cid,
                                "spawning telemetry sampler"
                            );
                            telemetry::spawn_sampler(
                                req.run_id.clone(),
                                cid.clone(),
                                telem_tx.clone(),
                                telem_cancel.clone(),
                            );
                            sampler_container = Some(cid);
                        }
                    }
                }

                tracker.absorb(event, sink);
                if tracker.terminal {
                    tracing::info!(target: "drift::workflow", run_id = %req.run_id, "scan workflow completed");
                    break;
                }
            }
        }
    }

    // Shut the sampler down and drain any straggler samples. Dropping our
    // copy of the sender lets the receiver close once the sampler exits.
    telem_cancel.cancel();
    drop(telem_tx);
    while let Ok(sample) = telem_rx.try_recv() {
        sink.emit_telemetry(sample);
    }

    // Synthesise the visibility map + emit the final RunComplete. Only on a
    // clean Done — Error / Approval / TurnBudgetExceeded paths already
    // emitted a RunError from inside `absorb`, so we exit silently here.
    if tracker.completed_successfully {
        let issues = std::mem::take(&mut tracker.last_analysis_issues);
        let issues_found = issues.len() as u32;
        let critical_count = issues
            .iter()
            .filter(|i| matches!(i.severity, Severity::Critical))
            .count() as u32;

        if !issues.is_empty() {
            // Stage 4 — Building thesis. The agent's analyze_samples
            // already advanced us here; mark Active explicitly so the UI
            // shows progress while the LLM synthesis call runs.
            sink.emit_step(StepUpdate {
                run_id: req.run_id.clone(),
                index: 4,
                status: StepStatus::Active,
                detail: Some("Asking the model to summarise the architecture impact…".into()),
                duration_ms: None,
            });
            let thesis_start = Instant::now();
            let map =
                build_visibility_map(&issues, synth_provider.clone(), &cancel).await;
            sink.emit_step(StepUpdate {
                run_id: req.run_id.clone(),
                index: 4,
                status: StepStatus::Done,
                detail: Some(format!(
                    "{} critical · {} warnings · ~{:.0}% CPU reduction available",
                    map.critical.len(),
                    map.warnings.len(),
                    map.estimated_cpu_reduction_pct
                )),
                duration_ms: Some(thesis_start.elapsed().as_millis() as u64),
            });

            // Stage 5 — Reporting. Atomic from the user's perspective —
            // a single emit_report flush + RunComplete.
            sink.emit_step(StepUpdate {
                run_id: req.run_id.clone(),
                index: 5,
                status: StepStatus::Active,
                detail: Some("Packaging the visibility map…".into()),
                duration_ms: None,
            });
            sink.emit_report(RunReport {
                run_id: req.run_id.clone(),
                map,
            });
            sink.emit_step(StepUpdate {
                run_id: req.run_id.clone(),
                index: 5,
                status: StepStatus::Done,
                detail: Some("Report ready.".into()),
                duration_ms: None,
            });
        }

        sink.emit_complete(RunComplete {
            run_id: req.run_id.clone(),
            issues_found,
            critical_count,
        });
    }

    Ok(())
}

/// Best-effort: pull a `container_id` field out of a tool's JSON output.
/// Tools that touch the target container (install_profiler, drive_load,
/// run_profiling, exec_in_container) all surface it on the top level.
fn extract_container_id(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    v.get("container_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Bucket issues into critical / warnings, compute the heuristic CPU-reduction
/// estimate, and call the LLM once for a 3-5 bullet architecture summary.
/// Falls back to a deterministic per-category recap if the synthesis fails
/// — the report is the user-facing deliverable, so we never leave it empty.
async fn build_visibility_map(
    issues: &[Issue],
    provider: Arc<dyn Provider>,
    cancel: &CancellationToken,
) -> VisibilityMap {
    let critical: Vec<Issue> = issues
        .iter()
        .filter(|i| matches!(i.severity, Severity::Critical))
        .take(3)
        .cloned()
        .collect();
    let warnings: Vec<Issue> = issues
        .iter()
        .filter(|i| matches!(i.severity, Severity::High | Severity::Medium))
        .take(10)
        .cloned()
        .collect();

    let estimated_cpu_reduction_pct = critical
        .iter()
        .map(|i| i.self_pct as f32)
        .sum::<f32>()
        .min(50.0);

    let architecture_advice = match synthesise_advice(provider, issues, cancel).await {
        Ok(bullets) if !bullets.is_empty() => bullets,
        Ok(_) | Err(_) => fallback_advice(issues),
    };

    VisibilityMap {
        critical,
        warnings,
        estimated_cpu_reduction_pct,
        architecture_advice,
    }
}

/// Run one non-streaming LLM turn, no tools available, asking for a JSON blob
/// of architectural recommendations.
async fn synthesise_advice(
    provider: Arc<dyn Provider>,
    issues: &[Issue],
    cancel: &CancellationToken,
) -> Result<Vec<String>, String> {
    let bullets = issues
        .iter()
        .take(15)
        .map(|i| {
            format!(
                "- `{}` (category {:?}, severity {:?}, self {:.1}%, total {:.1}%)",
                i.function, i.category, i.severity, i.self_pct, i.total_pct
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system =
        "You are a senior performance engineer. Given a list of profiled hotspots, write \
         3-5 short architectural recommendations. Each bullet must reference a specific \
         function and the % of total CPU it accounts for. Output JSON only, no prose \
         around it: `{ \"advice\": [\"...\", \"...\"] }`.";
    let user = Message::user(format!(
        "Profiling hotspots from a recent run:\n\n{bullets}\n\n\
         Return the 3-5 most impactful architectural changes."
    ));

    let stream = provider
        .stream(system, std::slice::from_ref(&user), &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = Box::pin(stream);
    let mut buf = String::new();
    while let Some(item) = stream.next().await {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        match item {
            Ok((Some(msg), _)) => buf.push_str(&msg.flat_text()),
            Ok((None, _)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }

    if let Some(items) = parse_advice_json(&buf) {
        return Ok(items);
    }
    // The model returned prose around the JSON, or no JSON at all. Try a
    // permissive line-based split as a last resort.
    Ok(split_bullets(&buf))
}

/// Extract `advice: string[]` from any JSON object embedded in `text`.
fn parse_advice_json(text: &str) -> Option<Vec<String>> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let blob = &text[start..=end];
    let v: serde_json::Value = serde_json::from_str(blob).ok()?;
    let arr = v.get("advice")?.as_array()?;
    let items: Vec<String> = arr
        .iter()
        .filter_map(|s| s.as_str().map(str::trim).map(String::from))
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// Permissive newline + bullet-marker split. Used when the model didn't
/// return clean JSON.
fn split_bullets(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '·', '•']).trim())
        .filter(|t| t.len() >= 8 && !t.starts_with('{') && !t.starts_with('}'))
        .map(String::from)
        .take(5)
        .collect()
}

/// Deterministic fallback when the synthesis call fails entirely. Builds one
/// bullet per category found in the top issues, so the report still shows
/// *something* actionable.
fn fallback_advice(issues: &[Issue]) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut by_cat: BTreeMap<String, (f64, String)> = BTreeMap::new();
    for i in issues.iter().take(10) {
        let cat = format!("{:?}", i.category);
        let entry = by_cat.entry(cat).or_insert((0.0, i.function.clone()));
        entry.0 += i.self_pct;
    }
    by_cat
        .into_iter()
        .map(|(cat, (pct, fn_name))| {
            format!(
                "Address {} hotspots (e.g. `{}`) — accounts for ~{:.0}% of total CPU.",
                cat.to_lowercase(),
                fn_name,
                pct
            )
        })
        .take(5)
        .collect()
}

/// Backend log line per agent event. Goes through `tracing` so it lands on
/// stderr when running `drift-lab` from a terminal.
///
/// We intentionally truncate `TextDelta` text and tool arg/result payloads —
/// the LLM can produce kilobytes of prose in a single chunk and the log
/// stream is meant to be skimmable, not exhaustive.
fn log_agent_event(run_id: &str, event: &AgentEvent) {
    match event {
        AgentEvent::TextDelta { text } => {
            tracing::debug!(
                target: "drift::agent",
                run_id,
                delta = %truncate(text, 80),
                "thinking"
            );
        }
        AgentEvent::AssistantMessage { message } => {
            tracing::debug!(
                target: "drift::agent",
                run_id,
                tool_count = message.tool_requests().len(),
                text_len = message.flat_text().len(),
                "assistant turn committed"
            );
        }
        AgentEvent::ToolDispatched { id, name, arguments } => {
            tracing::info!(
                target: "drift::agent",
                run_id,
                tool = %name,
                tool_id = %id,
                args = %truncate(&arguments.to_string(), 240),
                "→ dispatching tool"
            );
        }
        AgentEvent::ToolCompleted { id, content, is_error } => {
            if *is_error {
                tracing::warn!(
                    target: "drift::agent",
                    run_id,
                    tool_id = %id,
                    error_preview = %truncate(content, 240),
                    "← tool returned error"
                );
            } else {
                tracing::info!(
                    target: "drift::agent",
                    run_id,
                    tool_id = %id,
                    result_len = content.len(),
                    result_preview = %truncate(content, 240),
                    "← tool succeeded"
                );
            }
        }
        AgentEvent::ToolNeedsApproval { name, .. } => {
            tracing::warn!(
                target: "drift::agent",
                run_id,
                tool = %name,
                "tool needs approval — denied in current mode"
            );
        }
        AgentEvent::Usage(u) => {
            tracing::debug!(
                target: "drift::agent",
                run_id,
                input_tokens = ?u.input_tokens,
                output_tokens = ?u.output_tokens,
                total_tokens = ?u.total_tokens,
                "usage"
            );
        }
        AgentEvent::TurnBudgetExceeded { max_turns } => {
            tracing::warn!(
                target: "drift::agent",
                run_id,
                max_turns,
                "turn budget exceeded — stopping"
            );
        }
        AgentEvent::Error { message } => {
            tracing::error!(target: "drift::agent", run_id, %message, "agent error");
        }
        AgentEvent::Done => {
            tracing::info!(target: "drift::agent", run_id, "✓ scan complete");
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let cut = s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(n);
        format!("{}…(+{} chars)", &s[..cut], s.len() - cut)
    }
}

/// Internal state the workflow maintains as `AgentEvent`s flow in. Exposes a
/// single `absorb()` entry point so the loop above doesn't carry state.
struct StepTracker {
    run_id: String,
    /// Buffered text from the assistant since the last tool call. This is
    /// what we hand to the UI as the step's `detail` when a tool starts.
    thinking_buffer: String,
    /// Index → start time, so we can compute durationMs on completion.
    starts: std::collections::HashMap<usize, Instant>,
    /// Whether we've already finalised the run.
    terminal: bool,
    /// True only when the agent emitted `Done` cleanly. Used by `run()` to
    /// decide whether to synthesise the visibility map / emit `RunComplete`
    /// (we don't on Error / Approval / TurnBudgetExceeded paths).
    completed_successfully: bool,
    /// The last tool we dispatched — so when `ToolCompleted` arrives we know
    /// which step index to mark done. Tools may run sequentially or in
    /// parallel; we track the last *dispatched* call.
    pending_index: Option<usize>,
    /// Name of the last-dispatched tool. Some tools (notably `check_docker`)
    /// always return `Ok`, but the *payload* tells us whether the user can
    /// continue — we use the name to peek at the right field on completion.
    pending_tool: Option<String>,
    /// Full issue list from the last successful `analyze_samples` call. Read
    /// by `run()` after the agent finishes, to build the visibility map and
    /// populate `RunComplete`.
    last_analysis_issues: Vec<Issue>,
}

impl StepTracker {
    fn new(run_id: String) -> Self {
        Self {
            run_id,
            thinking_buffer: String::new(),
            starts: std::collections::HashMap::new(),
            terminal: false,
            completed_successfully: false,
            pending_index: None,
            pending_tool: None,
            last_analysis_issues: Vec::new(),
        }
    }

    fn absorb<S: WorkflowSink>(&mut self, event: AgentEvent, sink: &S) {
        match event {
            AgentEvent::TextDelta { text } => {
                // Accumulate prose. We don't emit per-token step updates —
                // too noisy. The *next* ToolDispatched flushes this buffer
                // into the active step's detail.
                self.thinking_buffer.push_str(&text);
            }

            AgentEvent::AssistantMessage { .. } => {
                // No-op: text already flowed via TextDeltas; tool requests
                // arrive as ToolDispatched/Completed.
            }

            AgentEvent::ToolDispatched { name, arguments, .. } => {
                let Some(index) = tool_to_step_index(&name) else {
                    tracing::debug!(
                        target: "drift::workflow",
                        tool = %name,
                        "tool not mapped to a timeline step (reasoning aid)"
                    );
                    return;
                };
                self.starts.insert(index, Instant::now());
                self.pending_index = Some(index);
                self.pending_tool = Some(name.clone());

                let thinking = std::mem::take(&mut self.thinking_buffer);
                let detail = if thinking.trim().is_empty() {
                    format!("Calling {name}…")
                } else {
                    format!("{} — calling {}…", thinking.trim(), name)
                };
                tracing::info!(
                    target: "drift::workflow",
                    run_id = %self.run_id,
                    step_index = index,
                    tool = %name,
                    args = %truncate(&arguments.to_string(), 240),
                    thinking_chars = thinking.len(),
                    "step → ACTIVE"
                );
                sink.emit_step(StepUpdate {
                    run_id: self.run_id.clone(),
                    index,
                    status: StepStatus::Active,
                    detail: Some(detail),
                    duration_ms: None,
                });
            }

            AgentEvent::ToolCompleted { content, is_error, .. } => {
                let Some(index) = self.pending_index.take() else {
                    tracing::debug!(
                        target: "drift::workflow",
                        "tool completion with no pending index — likely a reasoning-aid tool"
                    );
                    return;
                };
                let tool_name = self.pending_tool.take().unwrap_or_default();
                let duration_ms = self
                    .starts
                    .remove(&index)
                    .map(|t| t.elapsed().as_millis() as u64);
                // `check_docker` always returns Ok with structured data, even
                // when Docker isn't usable. Promote a non-ready status to an
                // effective error so the UI's timeline shows it as a blocker
                // (with the install/start hint as detail) instead of a green
                // check that contradicts the actual state.
                let effective_error = is_error
                    || (tool_name == "check_docker" && !check_docker_is_ready(&content));
                // Capture analyze_samples issues so `run()` can build the
                // visibility map after the loop ends. Only on success.
                if index == 4 && !is_error {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(arr) = v.get("issues") {
                            if let Ok(parsed) =
                                serde_json::from_value::<Vec<Issue>>(arr.clone())
                            {
                                self.last_analysis_issues = parsed;
                            }
                        }
                    }
                }
                // For `check_docker`, prefer the structured hint as the
                // user-visible detail — it's already actionable prose.
                let summary = if tool_name == "check_docker" {
                    check_docker_summary(&content).unwrap_or_else(|| {
                        summarise_tool_output(index, &content, effective_error)
                    })
                } else {
                    summarise_tool_output(index, &content, effective_error)
                };
                let status = if effective_error { StepStatus::Error } else { StepStatus::Done };
                tracing::info!(
                    target: "drift::workflow",
                    run_id = %self.run_id,
                    step_index = index,
                    duration_ms = ?duration_ms,
                    is_error = effective_error,
                    summary = %summary,
                    "step → {}",
                    if effective_error { "ERROR" } else { "DONE" }
                );
                sink.emit_step(StepUpdate {
                    run_id: self.run_id.clone(),
                    index,
                    status,
                    detail: Some(summary),
                    duration_ms,
                });
            }

            AgentEvent::ToolNeedsApproval { name, .. } => {
                // In Default mode the destructive stages need approval. Surface
                // that to the timeline so the user can re-run with `auto`.
                let Some(index) = tool_to_step_index(&name) else { return };
                sink.emit_step(StepUpdate {
                    run_id: self.run_id.clone(),
                    index,
                    status: StepStatus::Error,
                    detail: Some(format!(
                        "Approval required for `{name}` — re-run with autonomous mode."
                    )),
                    duration_ms: None,
                });
                self.terminal = true;
                sink.emit_error(RunError {
                    run_id: self.run_id.clone(),
                    message: format!("approval required for {name}"),
                });
            }

            AgentEvent::Usage(_) => { /* token accounting — ignored here */ }

            AgentEvent::TurnBudgetExceeded { max_turns } => {
                self.terminal = true;
                sink.emit_error(RunError {
                    run_id: self.run_id.clone(),
                    message: format!("hit the {max_turns}-turn budget without completing"),
                });
            }

            AgentEvent::Error { message } => {
                self.terminal = true;
                sink.emit_error(RunError {
                    run_id: self.run_id.clone(),
                    message,
                });
            }

            AgentEvent::Done => {
                // Mark the loop terminal and defer the actual RunComplete
                // emission to `run()`, which also runs the visibility-map
                // synthesis turn (async) before signalling completion.
                self.terminal = true;
                self.completed_successfully = true;
            }
        }
    }
}

/// True when a `check_docker` payload reports `status: "ready"`. Anything else
/// (not_installed, daemon_unreachable, garbled JSON) is treated as a blocker.
fn check_docker_is_ready(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(str::to_string))
        .as_deref()
        == Some("ready")
}

/// Pull the user-facing line out of a `check_docker` payload. When the daemon
/// isn't ready the tool returns a `hint` field — that's the actionable line we
/// want on the timeline. When ready, fall through to the generic summariser.
fn check_docker_summary(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    let status = v.get("status").and_then(|s| s.as_str())?;
    if status == "ready" {
        return None;
    }
    let hint = v.get("hint").and_then(|s| s.as_str()).unwrap_or("");
    if hint.is_empty() {
        Some(format!("Docker status: {status}"))
    } else {
        Some(hint.to_string())
    }
}

/// Compress one tool's JSON output into one human line for the timeline.
/// We do not parse the structure deeply — just look for a few hint fields
/// that the existing `tools::*::Output` types tend to expose. The match
/// indexes mirror the 6-stage UI timeline.
fn summarise_tool_output(index: usize, content: &str, is_error: bool) -> String {
    if is_error {
        return content.lines().next().unwrap_or(content).to_string();
    }
    let v: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return content.chars().take(120).collect(),
    };
    match index {
        // Stage 0 — Understanding code. Covers discover_project (has
        // `language` / `frameworks`), list_directory (`entries`), and
        // read_file_excerpt (`bytes_read` / `lines`).
        0 => {
            if let Some(lang) = v.get("language").and_then(|s| s.as_str()) {
                let frameworks = v
                    .get("frameworks")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .take(3)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty());
                return match frameworks {
                    Some(f) => format!("Language: {lang} · Frameworks: {f}"),
                    None => format!("Language: {lang}"),
                };
            }
            if let Some(entries) = v.get("entries").and_then(|a| a.as_array()) {
                let n = entries.len();
                let truncated = v.get("truncated").and_then(|b| b.as_bool()).unwrap_or(false);
                return if truncated {
                    format!("Listed {n}+ entries (truncated)")
                } else {
                    format!("Listed {n} entries")
                };
            }
            if let Some(lines) = v.get("lines").and_then(|a| a.as_array()) {
                return format!("Read {} lines", lines.len());
            }
            "Inspecting project".into()
        }
        // Stage 1 — Locating how to run. `check_docker` reports `status`,
        // `find_image` reports `image_ref`. Disjoint payloads.
        1 => {
            if let Some(status) = v.get("status").and_then(|s| s.as_str()) {
                let server = v
                    .get("server_version")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if v.get("server_version").is_some() || status == "not_installed" {
                    return match status {
                        "ready" if !server.is_empty() => format!("Docker ready (daemon v{server})"),
                        "ready" => "Docker ready".into(),
                        "daemon_unreachable" => {
                            "Docker is installed but the daemon isn't running".into()
                        }
                        "not_installed" => "Docker is not installed on this machine".into(),
                        other => format!("Docker status: {other}"),
                    };
                }
            }
            v.get("image_ref")
                .and_then(|s| s.as_str())
                .map(|s| format!("Found {s}"))
                .unwrap_or_else(|| "Image located".into())
        }
        // Stage 2 — Setting up runtime. `ensure_image` shape has
        // `resolved_image`; `detect_runtime` has `language` /
        // `recommended_profiler`.
        2 => {
            if let Some(resolved) = v.get("resolved_image").and_then(|s| s.as_str()) {
                let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
                let strategy = v.get("strategy").and_then(|s| s.as_str()).unwrap_or("");
                return match (status, strategy) {
                    ("already_present", _) => format!("Image already on daemon: {resolved}"),
                    ("discovered_existing", _) => {
                        format!("Reusing existing local image: {resolved}")
                    }
                    ("built", "compose-build") => format!("Built via compose: {resolved}"),
                    ("built", _) => format!("Built image: {resolved}"),
                    ("pulled", _) => format!("Pulled image: {resolved}"),
                    ("failed", _) => v
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("ensure_image failed")
                        .to_string(),
                    (other, _) => format!("ensure_image: {other}"),
                };
            }
            let lang = v.get("language").and_then(|s| s.as_str()).unwrap_or("?");
            let prof = v
                .get("recommended_profiler")
                .and_then(|s| s.as_str())
                .unwrap_or("?");
            format!("Language: {lang} · Profiler: {prof}")
        }
        // Stage 3 — Running + profiling. find_test_runner reports
        // `candidates`; install_profiler reports `version`; drive_load
        // reports `requests_sent`; run_profiling reports `samples_captured`.
        3 => {
            if let Some(samples) = v.get("samples_captured").and_then(|n| n.as_u64()) {
                return format!("{samples} samples captured");
            }
            if let Some(reqs) = v.get("requests_sent").and_then(|n| n.as_u64()) {
                return format!("{reqs} requests driven");
            }
            if let Some(version) = v.get("version").and_then(|s| s.as_str()) {
                return format!("Profiler installed ({version})");
            }
            if let Some(candidates) = v.get("candidates").and_then(|a| a.as_array()) {
                return format!("{} test candidate(s) found", candidates.len());
            }
            "Profiling stage complete".into()
        }
        // Stage 4 — Building thesis (analyze_samples). The synthesis turn
        // overwrites this stage's detail after the post-loop emit.
        4 => {
            let issues = v.get("issues").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0);
            let crit = v.get("critical_count").and_then(|n| n.as_u64()).unwrap_or(0);
            format!("{issues} issues · {crit} critical")
        }
        _ => "Done".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::MessageStream;
    use crate::agent::tools::Mode;
    use crate::agent::types::{Message, MessageContent, ProviderError, Role, ToolDef, Usage};
    use async_trait::async_trait;
    use futures_util::stream;
    use std::sync::Mutex;

    type ScriptedChunk = Result<(Option<Message>, Option<Usage>), ProviderError>;
    type ScriptedTurns = Vec<Vec<ScriptedChunk>>;

    /// Scripted provider — same shape as the one in `agent_loop` tests. Each
    /// outer-loop turn pops the next pre-baked stream off `turns`.
    struct ScriptedProvider {
        turns: Mutex<ScriptedTurns>,
    }

    impl ScriptedProvider {
        fn new(turns: ScriptedTurns) -> Self {
            Self { turns: Mutex::new(turns) }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn name(&self) -> &str { "scripted" }
        async fn stream(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolDef],
        ) -> Result<MessageStream, ProviderError> {
            let turn = self.turns.lock().unwrap().remove(0);
            Ok(Box::pin(stream::iter(turn)))
        }
    }

    fn text_chunk(s: &str) -> Result<(Option<Message>, Option<Usage>), ProviderError> {
        Ok((Some(Message::assistant_text(s)), None))
    }

    fn tool_chunk(
        id: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<(Option<Message>, Option<Usage>), ProviderError> {
        Ok((
            Some(Message {
                role: Role::Assistant,
                content: vec![MessageContent::ToolRequest {
                    id: id.into(),
                    name: name.into(),
                    arguments: args,
                }],
            }),
            None,
        ))
    }

    #[test]
    fn goal_prompt_always_includes_project_path_even_with_focus() {
        // Regression: presets used to *replace* the goal entirely, dropping
        // the project_path. The model then had nothing to investigate and
        // emitted zero tool calls.
        let with_focus = build_goal_prompt("/srv/checkout", Some("Profile a specific test"));
        assert!(with_focus.contains("/srv/checkout"));
        assert!(with_focus.contains("Focus for this run: Profile a specific test"));
        assert!(with_focus.contains("discover_project"));

        let without = build_goal_prompt("/srv/checkout", None);
        assert!(without.contains("/srv/checkout"));
        assert!(!without.contains("Focus for this run"));

        // Whitespace-only focus is treated as no focus.
        let blank = build_goal_prompt("/srv/checkout", Some("   "));
        assert!(!blank.contains("Focus for this run"));
    }

    #[test]
    fn step_index_mapping_covers_canonical_workflow() {
        // Stage 0 — Understanding code (read-only exploration).
        assert_eq!(tool_to_step_index("list_directory"), Some(0));
        assert_eq!(tool_to_step_index("read_file_excerpt"), Some(0));
        assert_eq!(tool_to_step_index("discover_project"), Some(0));
        // Stage 1 — Locating how to run.
        assert_eq!(tool_to_step_index("check_docker"), Some(1));
        assert_eq!(tool_to_step_index("find_image"), Some(1));
        // Stage 2 — Setting up runtime.
        assert_eq!(tool_to_step_index("ensure_image"), Some(2));
        assert_eq!(tool_to_step_index("detect_runtime"), Some(2));
        // Stage 3 — Running + profiling.
        assert_eq!(tool_to_step_index("find_test_runner_for_profiling"), Some(3));
        assert_eq!(tool_to_step_index("install_profiler"), Some(3));
        assert_eq!(tool_to_step_index("drive_load"), Some(3));
        assert_eq!(tool_to_step_index("run_profiling"), Some(3));
        // Stage 4 — Building thesis. analyze_samples lands here; the LLM
        // synthesis turn extends the same stage from `run()` post-loop.
        assert_eq!(tool_to_step_index("analyze_samples"), Some(4));
        // No-ops: meta tools / unmapped primitives.
        assert_eq!(tool_to_step_index("ask_user"), None);
        assert_eq!(tool_to_step_index("list_containers"), None);
    }

    #[test]
    fn goal_prompt_mentions_check_docker_as_first_step() {
        // Regression: stage 1 (detect_runtime) used to be where the agent
        // discovered Docker was unreachable, producing a cryptic 404. The
        // recipe now leads with check_docker so we can surface "install
        // Docker" before any daemon-touching tool runs.
        let p = build_goal_prompt("/srv/checkout", None);
        assert!(p.contains("check_docker"), "goal prompt should call out check_docker");
        let check_idx = p.find("check_docker").unwrap();
        let find_idx = p.find("find_image").unwrap();
        assert!(
            check_idx < find_idx,
            "check_docker should appear before find_image in the recipe"
        );
    }

    #[test]
    fn check_docker_ready_flag_handles_each_status() {
        let ready = r#"{"status":"ready","server_version":"24.0.7","hint":""}"#;
        let missing = r#"{"status":"not_installed","hint":"Install Docker Desktop…"}"#;
        let down = r#"{"status":"daemon_unreachable","hint":"Start Docker Desktop…"}"#;
        assert!(check_docker_is_ready(ready));
        assert!(!check_docker_is_ready(missing));
        assert!(!check_docker_is_ready(down));
        assert!(!check_docker_is_ready("not json"));
    }

    #[test]
    fn check_docker_summary_prefers_hint_when_not_ready() {
        let payload = r#"{"status":"not_installed","hint":"Install Docker Desktop from https://example"}"#;
        let got = check_docker_summary(payload).unwrap();
        assert!(got.contains("Install Docker"));
        // When ready, fall through to the generic summariser.
        let ready = r#"{"status":"ready","server_version":"24.0.7","hint":""}"#;
        assert!(check_docker_summary(ready).is_none());
    }

    #[tokio::test]
    async fn check_docker_not_installed_marks_locate_stage_as_error() {
        // The tool returned Ok with the structured "not_installed" payload —
        // the tracker should promote it to an Error step on stage 1 ("Locating
        // how to run") so the timeline shows a red blocker with the install
        // hint, not a green check that contradicts the actual state.
        let mut tracker = StepTracker::new("r".into());
        let sink = CaptureSink::default();

        tracker.absorb(
            AgentEvent::ToolDispatched {
                id: "cd".into(),
                name: "check_docker".into(),
                arguments: serde_json::json!({}),
            },
            &sink,
        );
        tracker.absorb(
            AgentEvent::ToolCompleted {
                id: "cd".into(),
                content: r#"{"status":"not_installed","binary_path":null,"client_version":null,"server_version":null,"hint":"Docker isn't installed. Install Docker Desktop."}"#.into(),
                is_error: false,
            },
            &sink,
        );

        let steps: Vec<StepUpdate> = sink
            .snapshot()
            .into_iter()
            .filter_map(|e| match e {
                CapturedEvent::Step(s) => Some(s),
                _ => None,
            })
            .collect();
        let done = steps
            .iter()
            .find(|s| matches!(s.status, StepStatus::Error))
            .expect("expected an Error step from a not-installed check_docker");
        assert_eq!(done.index, 1, "check_docker should error under stage 1 (Locating how to run)");
        assert!(
            done.detail.as_deref().unwrap_or("").contains("Install Docker"),
            "step detail should carry the install hint, got: {:?}",
            done.detail
        );
    }

    #[test]
    fn summarise_uses_image_ref_for_locate_stage() {
        // `find_image` now lives on stage 1 (Locating how to run).
        let payload = r#"{"image_ref": "registry/svc:42"}"#;
        let got = summarise_tool_output(1, payload, false);
        assert!(got.contains("registry/svc:42"));
    }

    #[test]
    fn summarise_falls_back_when_payload_is_garbage() {
        // Any stage should degrade to "echo the raw prefix" when the JSON
        // doesn't parse. We use stage 1 here arbitrarily.
        let got = summarise_tool_output(1, "this is not json", false);
        assert!(got.contains("not json"));
    }

    #[test]
    fn summarise_stage_zero_reports_discover_project_language() {
        let payload = r#"{"language": "python", "frameworks": ["fastapi"]}"#;
        let got = summarise_tool_output(0, payload, false);
        assert!(got.to_lowercase().contains("python"), "got: {got}");
        assert!(got.contains("fastapi"));
    }

    #[test]
    fn step_tracker_parses_analyze_samples_issues_for_visibility_map() {
        // The tracker captures the full Issue list from analyze_samples and
        // hands it to `run()` to build the visibility map. Done now only
        // marks the run as ready-to-finalise — RunComplete itself is emitted
        // post-loop by `run()`, so we don't assert on the sink here.
        let mut tracker = StepTracker::new("r".into());
        let sink = CaptureSink::default();

        tracker.absorb(
            AgentEvent::ToolDispatched {
                id: "t1".into(),
                name: "analyze_samples".into(),
                arguments: serde_json::json!({}),
            },
            &sink,
        );
        tracker.absorb(
            AgentEvent::ToolCompleted {
                id: "t1".into(),
                content: serde_json::json!({
                    "issues": [
                        {
                            "rank": 1,
                            "function": "psycopg.execute",
                            "category": "database",
                            "severity": "critical",
                            "self_pct": 30.0,
                            "total_pct": 60.0,
                            "samples": 100,
                            "example_stack": "main;handle;psycopg.execute"
                        },
                        {
                            "rank": 2,
                            "function": "json.dumps",
                            "category": "serde",
                            "severity": "high",
                            "self_pct": 12.5,
                            "total_pct": 22.0,
                            "samples": 42,
                            "example_stack": "main;render;json.dumps"
                        }
                    ],
                    "critical_count": 1
                })
                .to_string(),
                is_error: false,
            },
            &sink,
        );
        tracker.absorb(AgentEvent::Done, &sink);

        assert!(tracker.completed_successfully);
        assert_eq!(tracker.last_analysis_issues.len(), 2);
        assert_eq!(tracker.last_analysis_issues[0].function, "psycopg.execute");
        assert!(matches!(
            tracker.last_analysis_issues[0].severity,
            Severity::Critical
        ));
    }

    #[test]
    fn visibility_map_buckets_issues_and_caps_cpu_reduction() {
        // Two critical hotspots adding to 60% self-time → the heuristic
        // should clamp the headline at 50%. A high-severity issue and a
        // medium one belong in `warnings`.
        let issues = vec![
            test_issue("psycopg.execute", Severity::Critical, 35.0),
            test_issue("redis.get", Severity::Critical, 25.0),
            test_issue("json.dumps", Severity::High, 12.0),
            test_issue("read_file", Severity::Medium, 4.0),
        ];
        let map = futures_executor_block(build_visibility_map(
            &issues,
            std::sync::Arc::new(ScriptedProvider::new(vec![vec![text_chunk(
                r#"{"advice":["Refactor the DB layer to batch SELECTs"]}"#,
            )]])),
            &CancellationToken::new(),
        ));
        assert_eq!(map.critical.len(), 2);
        assert_eq!(map.warnings.len(), 2);
        assert!((map.estimated_cpu_reduction_pct - 50.0).abs() < 0.001);
        assert!(!map.architecture_advice.is_empty());
    }

    fn test_issue(name: &str, severity: Severity, self_pct: f64) -> Issue {
        use crate::tools::analyze_samples::Category;
        Issue {
            rank: 0,
            function: name.into(),
            category: Category::Database,
            severity,
            self_pct,
            total_pct: self_pct * 1.5,
            samples: 0,
            example_stack: name.into(),
        }
    }

    fn futures_executor_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[tokio::test]
    async fn workflow_emits_active_then_done_for_find_image() {
        // The agent thinks ("scanning…"), calls find_image with a path
        // pointing at a tempdir that has a Dockerfile, then on the next turn
        // emits a final answer with no tool calls.
        let dir = std::env::temp_dir().join(format!("drift-wf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM alpine\n").unwrap();

        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                text_chunk("Scanning the project for a Dockerfile."),
                tool_chunk(
                    "1",
                    "find_image",
                    serde_json::json!({"path": dir.display().to_string()}),
                ),
            ],
            vec![text_chunk("All done — no further tools needed.")],
        ]));

        let sink = CaptureSink::default();
        run(
            RunRequest {
                run_id: "test-run".into(),
                project_path: dir.display().to_string(),
                provider,
                mode: Mode::Auto,
                goal_prompt: Some("Test".into()),
            },
            &sink,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let events = sink.snapshot();
        let steps: Vec<&StepUpdate> = events
            .iter()
            .filter_map(|e| match e {
                CapturedEvent::Step(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(steps.len(), 2, "expected one Active + one Done event for find_image's stage");
        assert!(matches!(steps[0].status, StepStatus::Active));
        // find_image now lives on stage 1 (Locating how to run).
        assert_eq!(steps[0].index, 1);
        // The thinking prose flows into the active step's detail.
        assert!(
            steps[0]
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("Scanning the project"),
            "active detail should carry the LLM's thinking, got: {:?}",
            steps[0].detail
        );
        assert!(matches!(steps[1].status, StepStatus::Done));
        assert!(steps[1].detail.as_deref().unwrap_or("").contains("drift-lab/"));

        assert!(matches!(events.last(), Some(CapturedEvent::Complete(_))));
    }

    #[tokio::test]
    async fn workflow_walks_locate_runtime_profiling_thesis_stages() {
        // Cover stages 1-4 (find_image → analyze_samples) to prove the
        // orchestration is end-to-end. We use Mode::Auto so destructive
        // tools run. For tools that need Docker (detect_runtime,
        // install_profiler, run_profiling), the in-process call will fail
        // in a CI sandbox — that's fine: we're verifying the
        // *orchestration*, not the tool implementations.
        let dir = std::env::temp_dir().join(format!("drift-wf-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM alpine\n").unwrap();

        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![text_chunk("Step 1."), tool_chunk("a", "find_image",
                serde_json::json!({"path": dir.display().to_string()}))],
            vec![text_chunk("Step 2."), tool_chunk("b", "detect_runtime",
                serde_json::json!({"image": "drift-lab/anything:latest"}))],
            vec![text_chunk("Step 3."), tool_chunk("c", "install_profiler",
                serde_json::json!({"container_id": "abc", "language": "python"}))],
            vec![text_chunk("Step 4."), tool_chunk("d", "run_profiling",
                serde_json::json!({"container_id": "abc", "duration_seconds": 5}))],
            vec![text_chunk("Step 5."), tool_chunk("e", "analyze_samples",
                serde_json::json!({"sample_path": "/nonexistent"}))],
            vec![text_chunk("All done.")],
        ]));

        let sink = CaptureSink::default();
        run(
            RunRequest {
                run_id: "full".into(),
                project_path: dir.display().to_string(),
                provider,
                mode: Mode::Auto,
                goal_prompt: Some("Test".into()),
            },
            &sink,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let steps: Vec<&StepUpdate> = sink
            .snapshot()
            .into_iter()
            .filter_map(|e| match e {
                CapturedEvent::Step(s) => Some(s),
                _ => None,
            })
            .collect::<Vec<_>>()
            .iter()
            .map(|s| Box::leak(Box::new(s.clone())) as &StepUpdate)
            .collect();

        // Each step appears at least once with status Active. We don't
        // require Done because the destructive stages hit real Docker tool
        // stubs and will fail in a CI sandbox — their step still emits
        // under the right index, just with status Error.
        // Expected stages from the scripted tools:
        //   find_image       → 1 (Locating how to run)
        //   detect_runtime   → 2 (Setting up runtime)
        //   install_profiler → 3 (Running + profiling)
        //   run_profiling    → 3 (same stage)
        //   analyze_samples  → 4 (Building thesis)
        for expected_index in [1usize, 2, 3, 4] {
            assert!(
                steps.iter().any(|s| s.index == expected_index
                    && matches!(s.status, StepStatus::Active)),
                "missing Active event for step {expected_index}; saw: {:?}",
                steps.iter().map(|s| (s.index, s.status)).collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test]
    async fn workflow_mirrors_raw_agent_events_to_sink() {
        // Same setup as the find_image happy-path test, but here we assert
        // that the sink saw the streaming AgentEvents (text deltas + tool
        // dispatch + done), not just the coarse Step updates.
        let dir = std::env::temp_dir().join(format!("drift-wf-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM alpine\n").unwrap();

        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                text_chunk("Scanning for a Dockerfile."),
                tool_chunk(
                    "1",
                    "find_image",
                    serde_json::json!({"path": dir.display().to_string()}),
                ),
            ],
            vec![text_chunk("Done.")],
        ]));

        let sink = CaptureSink::default();
        run(
            RunRequest {
                run_id: "events".into(),
                project_path: dir.display().to_string(),
                provider,
                mode: Mode::Auto,
                goal_prompt: Some("Test".into()),
            },
            &sink,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let events = sink.snapshot();
        // Should have seen at least: TextDelta, ToolDispatched, ToolCompleted, Done.
        let kinds: Vec<&'static str> = events
            .iter()
            .filter_map(|e| match e {
                CapturedEvent::Agent(a) => Some(match a {
                    AgentEvent::TextDelta { .. } => "text_delta",
                    AgentEvent::AssistantMessage { .. } => "assistant_message",
                    AgentEvent::ToolDispatched { .. } => "tool_dispatched",
                    AgentEvent::ToolCompleted { .. } => "tool_completed",
                    AgentEvent::ToolNeedsApproval { .. } => "tool_needs_approval",
                    AgentEvent::Usage(_) => "usage",
                    AgentEvent::TurnBudgetExceeded { .. } => "turn_budget_exceeded",
                    AgentEvent::Error { .. } => "error",
                    AgentEvent::Done => "done",
                }),
                _ => None,
            })
            .collect();
        assert!(kinds.contains(&"text_delta"), "expected text_delta in mirrored stream, got {kinds:?}");
        assert!(kinds.contains(&"tool_dispatched"));
        assert!(kinds.contains(&"tool_completed"));
        assert!(kinds.contains(&"done"));
    }

    #[tokio::test]
    async fn destructive_tool_in_default_mode_surfaces_approval_error() {
        let dir = std::env::temp_dir().join(format!("drift-wf-deny-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM alpine\n").unwrap();

        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            text_chunk("I'll inject the profiler now."),
            tool_chunk(
                "x",
                "install_profiler",
                serde_json::json!({"container_id": "abc", "language": "python"}),
            ),
        ]]));

        let sink = CaptureSink::default();
        run(
            RunRequest {
                run_id: "deny".into(),
                project_path: dir.display().to_string(),
                provider,
                mode: Mode::Default,
                goal_prompt: Some("Test".into()),
            },
            &sink,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        // A destructive tool in Default mode → step marked Error + RunError.
        // install_profiler now lives on stage 3 (Running + profiling).
        let events = sink.snapshot();
        assert!(
            events.iter().any(|e| matches!(e, CapturedEvent::Step(s)
                if s.index == 3 && matches!(s.status, StepStatus::Error))),
            "expected step 3 to surface as Error in Default mode"
        );
        assert!(events.iter().any(|e| matches!(e, CapturedEvent::Error(_))));
    }
}
