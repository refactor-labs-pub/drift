//! Per-finding LLM suggestion driver.
//!
//! Reads a saved scan's `summary.findings_top` + `immediate_fixes` +
//! `refactor_candidates`, opens the surrounding source window for each via
//! the `read_file_lines` tool, and asks the LLM for a specific code change.
//! One LLM round-trip per finding, streamed back to the UI as `scan://
//! suggestion` events.
//!
//! **Why no agent loop here**: the static analyzer already decided what
//! deserves attention. The LLM's only job is to translate one finding into
//! one concrete suggestion. There is no tool dispatch, no turn budget — just
//! `system prompt + finding context + code window → answer`. This keeps the
//! responsibilities aligned with what the user asked for: "the use of the
//! LLM is only to give the suggestion over the code … only on the summary
//! of the static analysis."
//!
//! Streaming: each finding produces one `Provider::stream()` call whose text
//! deltas are concatenated into a single answer before emission. We don't
//! forward partial deltas because suggestion bodies are small (≤ 600 tokens)
//! and the UI renders better with one settled answer per finding than with a
//! growing partial string per row.

use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use drift_static_profiler::insights::{FindingTopRef, ImmediateFix, RefactorCandidate};
use drift_static_profiler::report::Report;

use crate::agent::provider::Provider;
use crate::agent::types::{Message, MessageContent};
use crate::scan::storage;
use crate::scan::types::{topic, ScanSuggestion, ScanSuggestionDone};
use crate::tools::read_file_lines;

const SYSTEM_PROMPT: &str =
    "You are a senior code reviewer reading the output of a static call-graph analyzer. \
     For each finding you receive, propose ONE specific code change that fixes the smell. \
     Output exactly:\n\
     1. A one-sentence rationale, prefixed with `Why: `.\n\
     2. A single fenced code block containing the corrected snippet — keep the surrounding \
        function signature intact so the user can paste it back.\n\
     Do not invent issues outside the finding. Do not output multiple options. Be concrete.";

/// Maximum number of findings we send to the LLM. We aggregate up to this
/// many across (immediate_fixes, refactor_candidates, findings_top) so a
/// scan with hundreds of findings still finishes in bounded time.
const MAX_FINDINGS: usize = 24;

/// Window around the finding line that we hand to the LLM. The default
/// matches the spec: anchor ± 30 lines.
const CONTEXT_LINES: u32 = 30;

/// Drive the entire suggestion pass for one saved scan. Spawns a Tokio task
/// so the command returns immediately; results stream via Tauri events.
pub fn start_suggestions<R: Runtime>(
    app: AppHandle<R>,
    scan_id: String,
    provider: Arc<dyn Provider>,
) {
    tauri::async_runtime::spawn(async move {
        let result = run(&app, &scan_id, provider.as_ref()).await;
        if let Err(e) = result {
            tracing::warn!(scan_id = %scan_id, "suggestion driver failed: {e:#}");
            let _ = app.emit(
                topic::SUGGESTION_DONE,
                ScanSuggestionDone { scan_id, total: 0, failed: 0 },
            );
        }
    });
}

async fn run<R: Runtime>(
    app: &AppHandle<R>,
    scan_id: &str,
    provider: &dyn Provider,
) -> Result<()> {
    let envelope = storage::load_envelope(scan_id)
        .with_context(|| format!("loading scan {scan_id}"))?;
    let report = &envelope.report;
    let items = collect_findings(report);

    let mut emitted = 0usize;
    let mut failed = 0usize;
    for (i, item) in items.into_iter().enumerate() {
        match suggest_one(app, scan_id, i, &item, provider).await {
            Ok(_) => emitted += 1,
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    scan_id = %scan_id,
                    file = %item.file,
                    line = item.line,
                    "suggestion for finding failed: {e:#}"
                );
            }
        }
    }

    let _ = app.emit(
        topic::SUGGESTION_DONE,
        ScanSuggestionDone {
            scan_id: scan_id.to_string(),
            total: emitted + failed,
            failed,
        },
    );
    Ok(())
}

/// Internal canonical form so we can iterate the three finding lanes
/// (immediate_fixes → refactor_candidates → findings_top) uniformly without
/// repeating the LLM round-trip code. Single source of truth for the
/// suggester's view of "what's worth fixing".
#[derive(Debug, Clone, Serialize)]
struct FindingItem {
    source: &'static str,
    kind: String,
    severity: String,
    name: String,
    file: String,
    line: usize,
    message: String,
}

/// Flatten the report's findings into a single ranked list. Order:
///   1. immediate_fixes  (high severity + trivial/small effort)
///   2. refactor_candidates  (multi-finding hotspots)
///   3. findings_top  (raw severity-ranked findings — fallback)
///
/// We dedupe by (file, line, kind) so the same hotspot doesn't get three
/// suggestions just because it surfaced in every lane.
fn collect_findings(report: &Report) -> Vec<FindingItem> {
    let s = &report.summary;
    let mut out: Vec<FindingItem> = Vec::new();
    let mut seen: std::collections::HashSet<(String, usize, String)> = Default::default();

    let mut push = |item: FindingItem| {
        let key = (item.file.clone(), item.line, item.kind.clone());
        if seen.insert(key) {
            out.push(item);
        }
    };

    for f in &s.immediate_fixes {
        push(from_immediate(f));
    }
    for c in &s.refactor_candidates {
        push(from_refactor(c));
    }
    for t in &s.findings_top {
        push(from_top_ref(t));
    }

    out.truncate(MAX_FINDINGS);
    out
}

fn from_immediate(f: &ImmediateFix) -> FindingItem {
    FindingItem {
        source: "immediate_fix",
        kind: format!("{:?}", f.kind).to_lowercase(),
        severity: format!("{:?}", f.severity).to_lowercase(),
        name: qualified_name(f.parent_class.as_deref(), &f.name),
        file: f.file.clone(),
        line: f.line,
        message: f.message.clone(),
    }
}

fn from_refactor(c: &RefactorCandidate) -> FindingItem {
    FindingItem {
        source: "refactor_candidate",
        kind: c
            .kinds
            .first()
            .map(|k| format!("{:?}", k).to_lowercase())
            .unwrap_or_else(|| "refactor".to_string()),
        severity: format!("{:?}", c.worst_severity).to_lowercase(),
        name: qualified_name(c.parent_class.as_deref(), &c.name),
        file: c.file.clone(),
        line: c.line,
        message: c.why.clone(),
    }
}

fn from_top_ref(t: &FindingTopRef) -> FindingItem {
    // findings_top only carries node_id + kind + severity + line. We split
    // the node_id on `::` to recover a presentation-friendly file/name pair.
    // file = first segment, class = penultimate, name = last.
    let parts: Vec<&str> = t.node_id.split("::").collect();
    let file = parts.first().copied().unwrap_or_default().to_string();
    let name = parts.last().copied().unwrap_or("").to_string();
    let parent = if parts.len() >= 3 {
        Some(parts[parts.len() - 2].to_string())
    } else {
        None
    };
    FindingItem {
        source: "finding_top",
        kind: format!("{:?}", t.kind).to_lowercase(),
        severity: format!("{:?}", t.severity).to_lowercase(),
        name: qualified_name(parent.as_deref(), &name),
        file,
        line: t.line,
        message: format!("{:?} ({:?})", t.kind, t.severity),
    }
}

fn qualified_name(parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(p) if !p.is_empty() => format!("{p}.{name}"),
        _ => name.to_string(),
    }
}

async fn suggest_one<R: Runtime>(
    app: &AppHandle<R>,
    scan_id: &str,
    index: usize,
    item: &FindingItem,
    provider: &dyn Provider,
) -> Result<()> {
    // Resolve the file relative to the scan's source_root when needed.
    // The static profiler stores file paths relative to the scan's source
    // root; the suggester needs the absolute path to actually open the file.
    let absolute = resolve_path(scan_id, &item.file)?;

    // Read the +/- 30 line window via the same tool the LLM would call.
    // Reusing the tool keeps a single code path for "open a file window".
    let window = read_file_lines::run(read_file_lines::Args {
        path: absolute.clone(),
        line: item.line as u32,
        context: Some(CONTEXT_LINES),
    })
    .await
    .with_context(|| format!("reading window for {}:{}", item.file, item.line))?;

    let user_prompt = build_user_prompt(item, &window);
    let messages = vec![Message {
        role: crate::agent::types::Role::User,
        content: vec![MessageContent::text(user_prompt)],
    }];

    let mut stream = provider
        .stream(SYSTEM_PROMPT, &messages, &[])
        .await
        .context("opening provider stream")?;
    let mut buffer = String::new();
    while let Some(next) = stream.next().await {
        let (msg, _usage) = next.context("provider stream item")?;
        if let Some(m) = msg {
            buffer.push_str(&m.flat_text());
        }
    }

    let _ = app.emit(
        topic::SUGGESTION,
        ScanSuggestion {
            scan_id: scan_id.to_string(),
            index,
            source: item.source,
            kind: item.kind.clone(),
            severity: item.severity.clone(),
            file: item.file.clone(),
            line: item.line,
            name: item.name.clone(),
            suggestion: buffer,
        },
    );
    Ok(())
}

/// Try to resolve `file` into an absolute path on disk. We trust the
/// scan's `generator.source_root` for the prefix when the recorded file is
/// relative — the static profiler emits relative paths.
fn resolve_path(scan_id: &str, file: &str) -> Result<String> {
    if std::path::Path::new(file).is_absolute() {
        return Ok(file.to_string());
    }
    let envelope = storage::load_envelope(scan_id)?;
    let root = envelope
        .report
        .generator
        .source_root
        .clone()
        .context("scan has no source_root — cannot resolve relative finding path")?;
    Ok(std::path::Path::new(&root)
        .join(file)
        .display()
        .to_string())
}

fn build_user_prompt(item: &FindingItem, window: &read_file_lines::Output) -> String {
    let mut buf = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(buf, "Finding from drift-static-profiler:");
    let _ = writeln!(buf, "- kind: {}", item.kind);
    let _ = writeln!(buf, "- severity: {}", item.severity);
    let _ = writeln!(buf, "- symbol: {}", item.name);
    let _ = writeln!(buf, "- file: {}:{}", item.file, item.line);
    let _ = writeln!(buf, "- analyzer message: {}", item.message);
    let _ = writeln!(buf);
    let _ = writeln!(
        buf,
        "Source window (lines {}–{} of {}):",
        window.start_line, window.end_line, window.total_lines
    );
    let _ = writeln!(buf, "```");
    for (i, line) in window.lines.iter().enumerate() {
        let n = window.start_line as usize + i;
        let marker = if n == item.line { ">" } else { " " };
        let _ = writeln!(buf, "{marker} {n:>5} | {line}");
    }
    let _ = writeln!(buf, "```");
    let _ = writeln!(
        buf,
        "Propose ONE specific code change. Output `Why: <one sentence>` then a single \
         fenced code block with the corrected snippet."
    );
    buf
}
