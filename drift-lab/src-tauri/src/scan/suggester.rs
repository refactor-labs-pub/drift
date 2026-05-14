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
//! **Per-finding invocation**: the UI exposes a "Study this" button per
//! finding row. Each click runs *one* finding end-to-end — there is no bulk
//! / "do them all" mode. The registry key is `(scan_id, index)` so the
//! user can study several findings concurrently (one stream per finding)
//! without one Stop click cancelling another.
//!
//! Streaming: each finding fires three event waves so the UI can render
//! suggestions live, OpenAI-chat-completions-style:
//!   1. `scan://suggestion-start` — row metadata, sent before the LLM stream
//!      opens so the UI shows the badges + a spinner immediately.
//!   2. `scan://suggestion-delta` — each non-empty text chunk from the
//!      provider stream, in order. The frontend appends to its accumulator.
//!   3. `scan://suggestion` — the final settled body. The frontend uses it to
//!      reconcile (in case any deltas were dropped) and clear the spinner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio_util::sync::CancellationToken;

use drift_static_profiler::insights::{FindingTopRef, ImmediateFix, RefactorCandidate};
use drift_static_profiler::report::Report;

use crate::agent::provider::Provider;
use crate::agent::types::{Message, MessageContent};
use crate::scan::storage;
use crate::scan::types::{
    topic, ScanSuggestion, ScanSuggestionDelta, ScanSuggestionDone, ScanSuggestionStart,
};
use crate::tools::read_file_lines;

/// Per-(scan, index) cancellation registry. Holds the `CancellationToken`
/// for the in-flight per-finding suggestion driver. The Stop button on the
/// UI translates into [`Self::cancel`]; the driver task removes its own
/// entry on completion via [`Self::clear`].
///
/// Each entry is keyed by `(scan_id, finding_index)` so the user can have
/// multiple "Study this" streams active at once without cross-cancellation.
#[derive(Default)]
pub struct SuggestionRegistry {
    inner: Mutex<HashMap<(String, usize), CancellationToken>>,
}

impl SuggestionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new cancel token for `(scan_id, index)`. Returns `None`
    /// if a token already exists (another driver is mid-flight for the same
    /// finding) — caller should NOT spawn another task in that case.
    pub fn register_if_absent(
        &self,
        scan_id: &str,
        index: usize,
    ) -> Option<CancellationToken> {
        let mut g = self.inner.lock().ok()?;
        let key = (scan_id.to_string(), index);
        if g.contains_key(&key) {
            return None;
        }
        let token = CancellationToken::new();
        g.insert(key, token.clone());
        Some(token)
    }

    /// Trigger cancellation for `(scan_id, index)`. Returns true if a
    /// session was actually live and got signalled, false if no session
    /// existed.
    pub fn cancel(&self, scan_id: &str, index: usize) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        match g.remove(&(scan_id.to_string(), index)) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Remove the registry entry. Called by the driver task on completion
    /// (whether success, error, or cancel) so a subsequent start can
    /// register a fresh token.
    fn clear(&self, scan_id: &str, index: usize) {
        if let Ok(mut g) = self.inner.lock() {
            g.remove(&(scan_id.to_string(), index));
        }
    }
}

/// Prompt contract — locks the output to a shape the UI can render as a
/// reasoning-first code review: two prose sections explaining the problem
/// and the chosen solution, followed by a GitHub-style unified diff. The
/// renderer is partial-tolerant: any prefix of the contract still parses
/// to a valid (possibly incomplete) view, so streaming token-by-token
/// "just works".
///
/// Format (strict):
///   1. `problem_description_reasoning:` header on its own line.
///   2. One or more paragraphs explaining *why this is a problem* — what
///      the analyzer flagged, why the pattern is risky/slow/buggy.
///   3. A blank line.
///   4. `solution_description_reasoning:` header on its own line.
///   5. One or more paragraphs explaining *why the proposed change works*
///      — what it does, why it's the right fix vs. alternatives.
///   6. A blank line.
///   7. A fenced unified-diff block:
///        ```diff
///        @@ -<old_start>,<old_len> +<new_start>,<new_len> @@
///         <context line>
///        -<removed line>
///        +<added line>
///        ```
///
/// Constraints we ask of the model:
///   - Minimal diff: 1–2 lines of context above and below the change.
///   - DON'T dump the whole function — only the changing region plus context.
///   - Use line numbers from the window header the user prompt provides.
const SYSTEM_PROMPT: &str =
    "You are a senior code reviewer reading the output of a static call-graph analyzer. \
     For each finding you receive, explain your reasoning and then propose ONE specific \
     code change that fixes the smell.\n\
     \n\
     Output format — strict, no preamble, no closing remarks:\n\
     1. The literal header line `problem_description_reasoning:` on its own line.\n\
     2. One or more paragraphs explaining WHY this is a problem — what the analyzer \
        flagged, why the current pattern is risky, slow, or buggy in this context.\n\
     3. A blank line.\n\
     4. The literal header line `solution_description_reasoning:` on its own line.\n\
     5. One or more paragraphs explaining WHY your proposed change fixes the problem \
        — what the change does, why this approach is right vs. alternatives, and any \
        trade-offs to be aware of.\n\
     6. A blank line.\n\
     7. A fenced unified-diff block. Begin with ```diff and end with ``` on their own \
        lines. Inside the block, use exactly the unified-diff conventions:\n\
        - `@@ -<old_start>,<old_len> +<new_start>,<new_len> @@` for the hunk header. \
          Derive line numbers from the source window in the user message.\n\
        - ` ` (single space) prefix for context lines that don't change.\n\
        - `-` prefix for lines being removed.\n\
        - `+` prefix for lines being added.\n\
     \n\
     Diff scoping rules:\n\
     - Show ONLY the lines that change, plus 1–2 lines of context above and below.\n\
     - Do NOT include the whole function unless the entire body is changing.\n\
     - Preserve original indentation byte-for-byte in `-`/` ` lines so the diff \
       is a real patch the user could apply.\n\
     \n\
     Do not invent issues outside the finding. Do not propose multiple options.";

/// Maximum number of findings we expose to the UI. We aggregate up to this
/// many across (immediate_fixes, refactor_candidates, findings_top) so the
/// finding list stays a sensible, scrollable length even on large scans.
const MAX_FINDINGS: usize = 24;

/// Window around the finding line that we hand to the LLM. The default
/// matches the spec: anchor ± 30 lines.
const CONTEXT_LINES: u32 = 30;

/// Drive a single-finding suggestion pass. Spawns a Tokio task so the
/// command returns immediately; results stream via Tauri events.
///
/// `cancel` is the per-(scan_id, index) token from [`SuggestionRegistry`].
/// `registry` is held so the task can free its slot on completion regardless
/// of how it finished (success / error / cancel). Without this cleanup a
/// fresh start for the same `(scan_id, index)` would silently no-op.
pub fn start_finding_suggestion<R: Runtime>(
    app: AppHandle<R>,
    scan_id: String,
    index: usize,
    provider: Arc<dyn Provider>,
    cancel: CancellationToken,
    registry: Arc<SuggestionRegistry>,
) {
    tauri::async_runtime::spawn(async move {
        let result = run_one(&app, &scan_id, index, provider.as_ref(), &cancel).await;
        registry.clear(&scan_id, index);
        if let Err(e) = result {
            tracing::warn!(scan_id = %scan_id, index, "suggestion driver failed: {e:#}");
            let _ = app.emit(
                topic::SUGGESTION_DONE,
                ScanSuggestionDone { scan_id, total: 0, failed: 1 },
            );
        }
    });
}

async fn run_one<R: Runtime>(
    app: &AppHandle<R>,
    scan_id: &str,
    index: usize,
    provider: &dyn Provider,
    cancel: &CancellationToken,
) -> Result<()> {
    let envelope = storage::load_envelope(scan_id)
        .with_context(|| format!("loading scan {scan_id}"))?;
    let report = &envelope.report;
    let items = collect_findings(report);
    let Some(item) = items.get(index) else {
        anyhow::bail!("finding index {index} out of range (have {})", items.len());
    };

    // If a Stop click landed before we even opened the stream, bail out
    // cleanly with a `suggestion-done` so the UI flips its run-state flag.
    if cancel.is_cancelled() {
        let _ = app.emit(
            topic::SUGGESTION_DONE,
            ScanSuggestionDone {
                scan_id: scan_id.to_string(),
                total: 0,
                failed: 0,
            },
        );
        return Ok(());
    }

    let result = suggest_one(app, scan_id, index, item, provider, cancel).await;

    let (emitted, failed) = match result {
        Ok(()) => (1usize, 0usize),
        Err(e) => {
            tracing::warn!(
                scan_id = %scan_id,
                index,
                file = %item.file,
                line = item.line,
                "suggestion for finding failed: {e:#}"
            );
            // The row may have been opened via `SUGGESTION_START` already;
            // without a closing `SUGGESTION` event the UI spinner would
            // hang forever. Emit a synthetic final event so the row
            // reconciles to a non-streaming state with an error body.
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
                    suggestion: format!("⚠ failed to generate suggestion: {e:#}"),
                },
            );
            (0, 1)
        }
    };

    // SUGGESTION_DONE always fires — cancelled, errored, or completed. The
    // UI uses this to flip the row's run-state flag and re-enable the
    // Study This button.
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
pub struct FindingItem {
    pub source: &'static str,
    pub kind: String,
    pub severity: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Flatten the report's findings into a single ranked list. Order:
///   1. immediate_fixes  (high severity + trivial/small effort)
///   2. refactor_candidates  (multi-finding hotspots)
///   3. findings_top  (raw severity-ranked findings — fallback)
///
/// We dedupe by (file, line, kind) so the same hotspot doesn't get three
/// suggestions just because it surfaced in every lane.
pub fn collect_findings(report: &Report) -> Vec<FindingItem> {
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
    cancel: &CancellationToken,
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

    // 1. Seed the UI row with metadata so the user sees the badges and a
    //    spinner the instant the row exists — before the model has produced
    //    anything. Mirrors the chat-UI pattern of rendering the bubble first
    //    and then growing the text inside it.
    let _ = app.emit(
        topic::SUGGESTION_START,
        ScanSuggestionStart {
            scan_id: scan_id.to_string(),
            index,
            source: item.source,
            kind: item.kind.clone(),
            severity: item.severity.clone(),
            file: item.file.clone(),
            line: item.line,
            name: item.name.clone(),
        },
    );

    // 2. Stream the provider. Every chunk that carries non-empty text is
    //    forwarded to the UI as one `scan://suggestion-delta` event. We also
    //    accumulate into `buffer` so the final event below is the canonical,
    //    settled body the UI reconciles against.
    //
    //    The `tokio::select!` race between `cancel.cancelled()` and
    //    `stream.next()` is what makes a Stop click feel instant: when the
    //    cancel future fires, the stream future is dropped, which drops the
    //    underlying reqwest connection, which sends a TCP FIN to the
    //    provider. The driver finalizes the row below with whatever has been
    //    captured so far.
    //
    //    `biased;` makes the cancel branch checked first inside select! —
    //    avoids the "one more chunk after Stop" jitter when both futures
    //    happen to be ready in the same poll.
    let mut stream = provider
        .stream(SYSTEM_PROMPT, &messages, &[])
        .await
        .context("opening provider stream")?;
    let mut buffer = String::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            next = stream.next() => {
                match next {
                    Some(item_res) => {
                        let (msg, _usage) = item_res.context("provider stream item")?;
                        if let Some(m) = msg {
                            let chunk = m.flat_text();
                            if chunk.is_empty() {
                                continue;
                            }
                            buffer.push_str(&chunk);
                            let _ = app.emit(
                                topic::SUGGESTION_DELTA,
                                ScanSuggestionDelta {
                                    scan_id: scan_id.to_string(),
                                    index,
                                    delta: chunk,
                                },
                            );
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // 3. Final settled body. Even on a mid-stream cancel we emit this so the
    //    UI clears `isStreaming` on the row (otherwise the row's spinner
    //    would hang forever). The body is exactly what we captured — no
    //    "(stopped)" marker, the partial-diff renderer already conveys
    //    incompleteness by virtue of the parser not seeing a closing fence.
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
        "Produce the output in the exact strict format from the system prompt:\n\
         1. `problem_description_reasoning:` header, then paragraphs.\n\
         2. Blank line.\n\
         3. `solution_description_reasoning:` header, then paragraphs.\n\
         4. Blank line.\n\
         5. A fenced ```diff block with `@@` hunk header, `-` for removed lines, \
            `+` for added lines, single-space prefix for context. Use the line \
            numbers shown above. Keep the diff minimal — only changed lines plus \
            1–2 lines of context."
    );
    buf
}
