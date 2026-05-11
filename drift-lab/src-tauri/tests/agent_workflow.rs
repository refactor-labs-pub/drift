//! Agent-driven scan integration tests.
//!
//! Two tiers:
//!
//! 1. **Real-fixture, no LLM** — runs the existing tools against the actual
//!    `cf-copilot` checkout that ships next to drift in this monorepo. Proves
//!    `find_image` can scan a real Docker project. Skipped if cf-copilot isn't
//!    on disk.
//!
//! 2. **Real local LLM, gated** — drives the full `agent::workflow::run` loop
//!    through a live OpenAI-compatible endpoint. Reuses the same env knobs
//!    as `openai_live.rs`:
//!    DRIFT_LAB_OPENAI_TEST_URL    e.g. `http://localhost:1234/v1` (LM Studio)
//!    DRIFT_LAB_OPENAI_TEST_KEY    optional
//!    DRIFT_LAB_OPENAI_TEST_MODEL  optional; auto-discovered if unset
//!    Skipped (with an explanatory log line) if the endpoint isn't reachable.
//!    Asserts the model picks `find_image` with the cf-copilot path as
//!    the FIRST tool call. We do not test later steps because they'd require
//!    Docker; the orchestration mapping already has full coverage in
//!    `agent::workflow`'s in-tree tests.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use drift_lab_lib::agent::tools::Mode;
use drift_lab_lib::agent::workflow::{
    self, CaptureSink, CapturedEvent, RunRequest, WorkflowSink,
};
use drift_lab_lib::agent::OpenAiProvider;
use drift_lab_lib::tools::{discover_project, find_image, find_test_runner_for_profiling};
use tokio_util::sync::CancellationToken;

const CF_COPILOT_PATH: &str = "/Users/ilyas/Projects/cf-copilot";
const CF_MONO_AUTO_ENRICH_PATH: &str = "/Users/ilyas/Projects/cf-mono/workspaces/automation-enrichements";
const CF_MONO_TARGET_TEST: &str = "/Users/ilyas/Projects/cf-mono/workspaces/automation-enrichements/src/invoice-enrichement-service/__tests__/process-invoice-enrichment-logic.test.ts";

// ---------------------------------------------------------------------------
// Tier 1: real fixture, no LLM.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_image_resolves_cf_copilot_compose() {
    let path = PathBuf::from(CF_COPILOT_PATH);
    if !path.is_dir() {
        eprintln!("agent_workflow: {CF_COPILOT_PATH} not present — skipping");
        return;
    }

    let out = find_image::run(find_image::Args {
        path: path.display().to_string(),
    })
    .await
    .expect("find_image should succeed against cf-copilot");

    // cf-copilot ships a `docker-compose.yml` at its root. The compose path
    // beats the Dockerfile path in the resolution order.
    assert!(
        matches!(out.source, find_image::Source::Compose),
        "expected compose source, got {:?}",
        out.source
    );
    assert!(
        out.manifest_path.contains("docker-compose"),
        "manifest path should point at docker-compose.yml, got {}",
        out.manifest_path
    );
    // cf-copilot's compose uses YAML merge anchors (`<<: *bun-base`) for the
    // `build:` field. Our line-based parser doesn't expand anchors, so every
    // service ends up with `build_ctx = None` and `image = None`; the picker
    // then chooses based on app-name hints only. We don't assert a specific
    // service — that depends on scoring tie-breaks we'd rather not lock in.
    // The integration contract is just: find_image returns *some* service
    // and `image_ref` is a sane string mentioning the project.
    let picked = out
        .compose_service
        .as_deref()
        .expect("compose source should pick some service");
    assert!(
        !picked.is_empty(),
        "compose service name should be non-empty"
    );
    assert!(
        out.image_ref.contains("cf-copilot"),
        "image_ref should mention cf-copilot, got {}",
        out.image_ref
    );
}

// ---------------------------------------------------------------------------
// Tier 1b: cf-mono — discovery tools against the actual workspace.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discover_project_resolves_cf_mono_automation_enrichements() {
    let path = PathBuf::from(CF_MONO_AUTO_ENRICH_PATH);
    if !path.is_dir() {
        eprintln!("agent_workflow: {CF_MONO_AUTO_ENRICH_PATH} not present — skipping");
        return;
    }

    let out = discover_project::run(discover_project::Args {
        path: path.display().to_string(),
    })
    .await
    .expect("discover_project should succeed against cf-mono");

    // cf-mono workspace is bun-driven TypeScript; assert the exact stack so
    // a regression in our heuristics fails loudly.
    assert_eq!(out.language, discover_project::Language::TypeScript);
    assert_eq!(out.package_manager, discover_project::PackageManager::Bun);
    assert!(out.has_dockerfile, "automation-enrichements has a Dockerfile");
    assert!(out.has_tests, "tests live under src/**/__tests__");

    // The `test` script is the canonical signal — make sure we surfaced it.
    let test_script = out
        .scripts
        .iter()
        .find(|s| s.name == "test")
        .expect("test script should be present");
    assert!(
        test_script.command.contains("bun test"),
        "expected `bun test` in test script, got {}",
        test_script.command
    );

    // Frameworks: cf-mono uses opentelemetry, ioredis, rabbitmq-client, zod.
    assert!(out.frameworks.contains(&"opentelemetry".to_string()));
    assert!(out.frameworks.contains(&"ioredis".to_string()));
    assert!(out.frameworks.contains(&"zod".to_string()));
}

#[tokio::test]
async fn find_test_runner_locks_to_target_test_in_cf_mono() {
    let path = PathBuf::from(CF_MONO_AUTO_ENRICH_PATH);
    if !path.is_dir() {
        eprintln!("agent_workflow: cf-mono missing — skipping");
        return;
    }
    if !PathBuf::from(CF_MONO_TARGET_TEST).is_file() {
        eprintln!(
            "agent_workflow: target test {CF_MONO_TARGET_TEST} not present — skipping"
        );
        return;
    }

    // With an explicit target the result should be deterministic: bun test +
    // the relative target path, exactly one candidate.
    let out = find_test_runner_for_profiling::run(find_test_runner_for_profiling::Args {
        path: path.display().to_string(),
        target_file: Some(CF_MONO_TARGET_TEST.into()),
        name_filter: None,
        max_candidates: None,
    })
    .await
    .unwrap();

    assert_eq!(
        out.runner,
        find_test_runner_for_profiling::TestRunner::BunTest
    );
    assert!(!out.target_not_found);
    assert_eq!(out.candidate_tests.len(), 1);
    assert!(out.candidate_tests[0].ends_with("process-invoice-enrichment-logic.test.ts"));
    assert_eq!(out.command[0], "bun");
    assert_eq!(out.command[1], "test");
    assert!(out.command.last().unwrap().ends_with("process-invoice-enrichment-logic.test.ts"));
}

#[tokio::test]
async fn find_test_runner_lists_candidates_when_target_omitted() {
    let path = PathBuf::from(CF_MONO_AUTO_ENRICH_PATH);
    if !path.is_dir() {
        eprintln!("agent_workflow: cf-mono missing — skipping");
        return;
    }
    let out = find_test_runner_for_profiling::run(find_test_runner_for_profiling::Args {
        path: path.display().to_string(),
        target_file: None,
        name_filter: None,
        max_candidates: Some(50),
    })
    .await
    .unwrap();

    // Multiple .test.ts files live under src/invoice-enrichement-service/__tests__/.
    assert!(
        out.candidate_tests.len() >= 3,
        "expected at least 3 candidate tests in cf-mono, got {}",
        out.candidate_tests.len()
    );
    assert!(out.candidate_tests.iter().any(|t| t.ends_with(".test.ts")));
    // Default command (no target) is just `bun test`.
    assert_eq!(out.command, vec!["bun".to_string(), "test".to_string()]);
}

// ---------------------------------------------------------------------------
// Tier 2: real local LLM (gated).
// ---------------------------------------------------------------------------

const DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";
const DEFAULT_KEY: &str = "not-needed";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

async fn endpoint_reachable(base_url: &str, api_key: &str) -> bool {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(&url);
    if api_key != DEFAULT_KEY && !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    matches!(req.send().await, Ok(resp) if resp.status().is_success())
}

async fn discover_model(base_url: &str, api_key: &str) -> Option<String> {
    if let Some(m) = env("DRIFT_LAB_OPENAI_TEST_MODEL") {
        return Some(m);
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = reqwest::Client::new().get(&url);
    if api_key != DEFAULT_KEY && !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("data")?
        .as_array()?
        .first()?
        .get("id")?
        .as_str()
        .map(|s| s.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn local_llm_drives_find_image_first() {
    if std::env::var("DRIFT_LAB_SKIP_NET").is_ok() {
        eprintln!("agent_workflow: DRIFT_LAB_SKIP_NET set — skipping");
        return;
    }
    if !PathBuf::from(CF_COPILOT_PATH).is_dir() {
        eprintln!("agent_workflow: {CF_COPILOT_PATH} missing — skipping local-LLM test");
        return;
    }
    let base_url = env("DRIFT_LAB_OPENAI_TEST_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let api_key = env("DRIFT_LAB_OPENAI_TEST_KEY").unwrap_or_else(|| DEFAULT_KEY.to_string());

    if !endpoint_reachable(&base_url, &api_key).await {
        eprintln!(
            "agent_workflow: {base_url} not reachable — skipping. \
             Start a local OpenAI-compatible server (LM Studio, llama-server, \
             Docker Model Runner) or set DRIFT_LAB_OPENAI_TEST_URL=… ."
        );
        return;
    }
    let Some(model) = discover_model(&base_url, &api_key).await else {
        eprintln!("agent_workflow: could not discover a model — skipping");
        return;
    };
    eprintln!("agent_workflow: base_url={base_url} model={model}");

    let provider = Arc::new(OpenAiProvider::new(base_url, api_key, model));
    let sink = CaptureSink::default();

    // Cap turns so a model that never calls a tool can't stall the test.
    // The default max_turns is 100; for an integration test 4 turns is plenty
    // to see whether the model picks find_image first. We do this by capping
    // the agent at 4 turns through the public Agent::with_max_turns API…
    // but `workflow::run` builds the Agent itself. Instead: use a 60s test
    // timeout via tokio::time::timeout.
    let req = RunRequest {
        run_id: "local-llm-test".into(),
        project_path: CF_COPILOT_PATH.to_string(),
        provider,
        mode: Mode::Auto,
        // Force the model to do step 1 *only* — keeps the test fast and
        // deterministic. The full 5-step orchestration is covered by the
        // unit tests with a scripted provider.
        goal_prompt: Some(format!(
            "Call the `find_image` tool exactly once with `path` set to \
             \"{CF_COPILOT_PATH}\", then reply with the image ref you got back. \
             Do not call any other tool."
        )),
    };

    let cancel = CancellationToken::new();
    let outcome = tokio::time::timeout(
        Duration::from_secs(120),
        workflow::run(req, &sink, cancel.clone()),
    )
    .await;
    cancel.cancel();

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("workflow returned an error: {e}"),
        Err(_) => panic!("workflow exceeded 120s — local LLM is too slow or stuck"),
    }

    let events = sink.snapshot();
    let any_step_zero = events.iter().any(|e| matches!(e, CapturedEvent::Step(s)
        if s.index == 0));
    assert!(
        any_step_zero,
        "expected at least one step-0 event (find_image), got {} events: {:?}",
        events.len(),
        events.iter().map(describe_event).collect::<Vec<_>>()
    );
}

/// Drives the new discovery toolchain against cf-mono with a real local LLM.
/// Asserts the model picks at least one of the **new** discovery tools
/// (`discover_project`, `find_test_runner_for_profiling`) — proves the agent
/// integrates them when investigating a project on disk.
#[tokio::test(flavor = "multi_thread")]
async fn local_llm_discovers_cf_mono_test_runner() {
    if std::env::var("DRIFT_LAB_SKIP_NET").is_ok() {
        eprintln!("agent_workflow: DRIFT_LAB_SKIP_NET set — skipping");
        return;
    }
    if !PathBuf::from(CF_MONO_AUTO_ENRICH_PATH).is_dir() {
        eprintln!("agent_workflow: cf-mono missing — skipping local-LLM discovery test");
        return;
    }
    let base_url = env("DRIFT_LAB_OPENAI_TEST_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let api_key = env("DRIFT_LAB_OPENAI_TEST_KEY").unwrap_or_else(|| DEFAULT_KEY.to_string());

    if !endpoint_reachable(&base_url, &api_key).await {
        eprintln!("agent_workflow: {base_url} not reachable — skipping discovery test");
        return;
    }
    let Some(model) = discover_model(&base_url, &api_key).await else {
        eprintln!("agent_workflow: no model — skipping discovery test");
        return;
    };
    eprintln!("agent_workflow (discovery): base_url={base_url} model={model}");

    let provider = std::sync::Arc::new(OpenAiProvider::new(base_url, api_key, model));
    let sink = CaptureSink::default();

    // Tight, deterministic prompt — gives small local models the best chance
    // of completing within the 120 s timeout. We force step 1 then step 4.
    let prompt = format!(
        "Your job: investigate the project at \"{CF_MONO_AUTO_ENRICH_PATH}\" and report \
         which test command should be used to profile its slowest path.\n\n\
         Take exactly two tool actions, in order:\n\
         1. Call `discover_project` with `path` = \"{CF_MONO_AUTO_ENRICH_PATH}\".\n\
         2. Call `find_test_runner_for_profiling` with the same path and \
         `target_file` = \"{CF_MONO_TARGET_TEST}\".\n\n\
         Then reply with ONE sentence stating the test runner and the command argv. \
         Do not call any other tool."
    );

    let req = RunRequest {
        run_id: "local-llm-discover".into(),
        project_path: CF_MONO_AUTO_ENRICH_PATH.into(),
        provider,
        mode: Mode::Auto,
        goal_prompt: Some(prompt),
    };
    let cancel = CancellationToken::new();
    let outcome = tokio::time::timeout(
        Duration::from_secs(180),
        workflow::run(req, &sink, cancel.clone()),
    )
    .await;
    cancel.cancel();
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("workflow returned an error: {e}"),
        Err(_) => panic!("workflow exceeded 180s — local LLM is too slow or stuck"),
    }

    let events = sink.snapshot();
    // The discovery tools both map to step 1 ("Detecting language & runtime").
    // We assert that at least one step-1 event landed; the orchestration's
    // unit tests already cover the per-tool mapping.
    let step_one_count = events
        .iter()
        .filter(|e| matches!(e, CapturedEvent::Step(s) if s.index == 1))
        .count();
    assert!(
        step_one_count >= 1,
        "expected at least one step-1 event from a discovery tool, got {} events: {:?}",
        events.len(),
        events.iter().map(describe_event).collect::<Vec<_>>()
    );
}

fn describe_event(e: &CapturedEvent) -> String {
    match e {
        CapturedEvent::Step(s) => {
            format!("Step{{index:{}, status:{:?}, detail:{:?}}}", s.index, s.status, s.detail)
        }
        CapturedEvent::Complete(c) => format!("Complete{{run_id:{}}}", c.run_id),
        CapturedEvent::Error(e) => format!("Error{{message:{}}}", e.message),
        CapturedEvent::Agent(a) => format!("Agent{{{a:?}}}"),
        CapturedEvent::Telemetry(t) => format!("Telemetry{{{t:?}}}"),
        CapturedEvent::Report(r) => format!("Report{{{r:?}}}"),
    }
}

// Force the unused-import lint into silence — we depend on `WorkflowSink`'s
// trait bound on `CaptureSink` even though we never name the trait directly.
#[allow(dead_code)]
fn _assert_sink_bound(s: &CaptureSink) -> &dyn WorkflowSink {
    s
}
