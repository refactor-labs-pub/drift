//! Read a `driftdockerprofiler` events.log (one JSON object per line) and
//! aggregate it into a snakeviz-style profiling report.
//!
//! # Event schema this reader expects
//!
//! Every line is one of four `driftdockerprofiler` event types:
//!
//!   - `wall_trace`   / `cpu_trace`  (per-trace mode — one event per
//!                                    unique stack per profile window)
//!   - `wall_profile` / `cpu_profile`(bundle mode — one event per window
//!                                    with `samples[]` inlined)
//!
//! Per-trace events carry the data inline (`frames`, `count`); bundle
//! events fan out into one stack-sample per entry of `samples[]`.
//! Either way each (stack, count) pair represents `count × period_ns`
//! nanoseconds of work attributed to that exact call stack.
//!
//! # Wire shape this reader produces (see [`AggregateReport`])
//!
//!   - `functions[]` — per-function-name rollup with inclusive
//!     (`cumulative_us`) and exclusive (`total_us`) time. Sample-based,
//!     so `ncalls` is "number of samples where this function was the
//!     leaf", and `percall_us` ≈ `period_us`.
//!   - `tree`        — root → child hierarchy. Each unique frame name
//!     becomes a node; `value` is inclusive time, `self_value` is the
//!     time it was the leaf. Suitable for direct rendering as an
//!     icicle / flamegraph.
//!   - `calls[]`     — EMPTY for sampler-based input. The old trace-agent
//!     schema produced paired start/end events with call ids; a stack
//!     sampler has no equivalent concept. Kept on the type for backward
//!     wire-compat with the UI.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Hard cap on the per-call wire array. Kept on the type for forward
/// compatibility; with the sampler input we never populate it.
pub const MAX_RAW_CALLS: usize = 5_000;

// ---------------------------------------------------------------------------
// Wire input — what we deserialize off each JSONL line.

#[derive(Debug, Deserialize, Clone)]
struct RawFrame {
    name: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    /// Phase F1b: fully-qualified name from `frame.f_code.co_qualname`
    /// (Python 3.11+). Absent on older Pythons; the wire schema makes
    /// this field optional. Used for the F3 `node_id` join key.
    #[serde(default)]
    qualified_name: Option<String>,
    /// Phase F1b: containing module from `frame.f_globals['__name__']`.
    /// Forwarded to the UI for the join-key flow.
    #[serde(default)]
    module: Option<String>,
    /// Phase F1a: true iff the agent-skip filter would classify this
    /// frame as stdlib / runtime / profiler-self. Mirrors
    /// `drift-static-profiler::Frame.is_system`.
    #[serde(default)]
    is_system: Option<bool>,
}

/// Inner element of `samples[]` inside a bundle event. The shape mirrors
/// `driftdockerprofiler.profiler_json.Sample`.
#[derive(Debug, Deserialize)]
struct RawSample {
    count: i64,
    #[serde(default)]
    frames: Option<Vec<RawFrame>>,
}

/// Event timestamp as it appears on the wire. Two callers / two shapes:
///
/// * **JSONL file writer** (drift-profiler-python's local file mode)
///   emits an ISO-8601 UTC string with `Z`, e.g. `"2026-05-20T00:27:16.971Z"`.
/// * **Supabase Realtime publisher** emits an integer of nanoseconds
///   since the Unix epoch, e.g. `1779236876872834300`.
///
/// `#[serde(untagged)]` makes serde try each variant in order until one
/// parses. That lets us keep ONE struct shape for both sources — and
/// crucially stops the "wrong type" case from failing the whole
/// `RawEvent` deserialization and silently dropping the broadcast.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTime {
    /// ISO-8601 / RFC 3339, e.g. `"2026-05-20T00:27:16.971Z"`.
    Iso(String),
    /// Nanoseconds since Unix epoch (Python publisher).
    Nanos(i64),
}

impl RawTime {
    /// Project to microseconds since epoch — the unit the aggregator
    /// uses internally. `None` when the ISO variant is unparseable;
    /// integer ns always converts (divided by 1000 with int math).
    fn to_micros(&self) -> Option<i64> {
        match self {
            Self::Iso(s) => parse_iso_us(s),
            Self::Nanos(ns) => Some(ns / 1_000),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawEvent {
    /// `wall_trace` | `cpu_trace` | `wall_profile` | `cpu_profile` |
    /// `function_call`.
    #[serde(rename = "type", default)]
    event_type: String,

    /// Event timestamp. Two shapes seen in the wild:
    /// * ISO-8601 string with `Z` — what the JSONL file writer emits.
    /// * Integer nanoseconds since epoch — what the live Supabase
    ///   Realtime publisher sends (`drift-profiler-python` writer).
    /// [`RawTime`] tolerates both via an untagged enum; the conversion
    /// to microseconds happens in [`RawTime::to_micros`]. Before this,
    /// the wrong-type case (integer when we expected string) made the
    /// whole `RawEvent` fail to deserialize — silently dropping every
    /// broadcast.
    #[serde(default)]
    time: Option<RawTime>,
    #[serde(default)]
    period_ns: Option<i64>,

    // Per-trace fields (wall_trace / cpu_trace).
    #[serde(default)]
    count: Option<i64>,
    #[serde(default)]
    frames: Option<Vec<RawFrame>>,

    // Bundle fields (wall_profile / cpu_profile).
    #[serde(default)]
    samples: Option<Vec<RawSample>>,

    // function_call (deterministic tracer) fields.
    #[serde(default)]
    qualname: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    duration_ns: Option<i64>,

    // Labels — copied through to the report.
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    pod: Option<String>,
    #[serde(default)]
    cpu: Option<f64>,
}

// ---------------------------------------------------------------------------
// Wire output — what the desktop UI consumes. Types unchanged from the
// previous (drift-python) version so the React side keeps working.

/// Kept for wire compat with the UI's TypeScript types. Sampler input
/// never produces these.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    pub call_id: String,
    pub qualname: String,
    pub start_us: i64,
    pub end_us: i64,
    pub duration_us: i64,
    pub status: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub cpu: Option<f64>,
    pub parent_call_id: Option<String>,
    pub depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FunctionStat {
    pub qualname: String,
    pub ncalls: u32,
    /// Exclusive (self) time across all samples where this was the leaf.
    pub total_us: i64,
    /// Inclusive (self + descendants) time — samples where this name
    /// appeared anywhere on the stack, deduped within each stack.
    pub cumulative_us: i64,
    pub percall_us: f64,
    /// Always 0 for sampler input — no error notion.
    pub errors: u32,
    pub cpu_avg: Option<f64>,
    pub file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub name: String,
    /// Inclusive time spent on this path through the tree (μs).
    pub value: i64,
    /// Exclusive time at this node (μs).
    pub self_value: i64,
    pub ncalls: u32,
    pub depth: u32,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub children: Vec<TreeNode>,

    // ---- Phase F3 join keys ------------------------------------------------
    //
    // These fields mirror `drift-static-profiler/schema/profile.schema.json
    // ::CallTreeNode` (id / qualified_name / module / is_system) so a viewer
    // can join a sampled aggregate to a static profile by `node_id` and
    // surface combined facts (sample_count + complexity, sample_count +
    // findings, etc.). Omitted from JSON when None so the wire shape stays
    // identical for profiles that don't carry the optional Frame metadata.

    /// Stable identifier formatted as `file::class::name` — exact format
    /// the static profiler's `CallTreeNode.id` uses. Built by
    /// `make_node_id` from `file`, `name`, and `qualified_name`.
    /// Always populated (falls back to `file::name` when there's no
    /// class information).
    pub node_id: String,
    /// Forwarded from the sample frame's `qualified_name` (F1b).
    /// Omitted when the source frames didn't carry one (Python 3.7-3.10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualname: Option<String>,
    /// Forwarded from the sample frame's `module` (F1b).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Forwarded from the sample frame's `is_system` (F1a). True iff
    /// the agent-skip filter classified this frame as stdlib / runtime /
    /// profiler-self.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_system: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AggregateReport {
    pub source_file: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Wall-clock span from first event to last event (μs).
    pub duration_us: i64,
    pub total_events: u32,
    /// For sampler input: equals `total_events`. Each event is one
    /// stack-sample window; we report it under this name so the UI's
    /// existing "calls" counter stays meaningful.
    pub total_calls: u32,
    /// Always 0 for sampler input.
    pub unmatched_starts: u32,
    /// Always 0 for sampler input.
    pub unmatched_ends: u32,
    pub services: Vec<String>,
    pub pods: Vec<String>,
    pub functions: Vec<FunctionStat>,
    pub tree: TreeNode,
    /// Always empty for sampler input — see module docstring.
    pub calls: Vec<CallRecord>,
    pub calls_truncated: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EventLogMeta {
    pub path: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub modified_iso: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal — one normalised stack sample per (frames, count, period).

struct StackSample {
    frames: Vec<RawFrame>,
    /// Weight in microseconds (= count × period_ns / 1000).
    weight_us: i64,
    /// Raw tick count; used for the per-function leaf-tally.
    tick_count: i64,
    /// 1-min loadavg, if known; rolled into FunctionStat.cpu_avg.
    cpu: Option<f64>,
}

// ---------------------------------------------------------------------------
// Aggregator — incremental state machine.
//
// Same rollup math as the legacy `aggregate(path)` function — but exposed
// as a long-lived struct so the live realtime path can ingest broadcasts
// one at a time and re-emit a fresh snapshot on demand, without ever
// re-reading the JSONL file from disk.
//
// The split is straightforward:
//   * `ingest_event(&RawEvent)` mutates the *cheap* per-event state:
//     samples vector, time bounds, service/pod sets, total counter.
//   * `snapshot(&str)` walks `samples` and builds the per-function rollup
//     + tree. This is the O(N) work the legacy 1 Hz file-tail repeated
//     every tick; the broadcaster will call it at ≤4 Hz instead.
//
// Why keep the work in `snapshot` and not in `ingest`? Two reasons:
//   1. Order-independence — the rollup is a function of the multiset of
//      samples. Doing it per-event would still re-traverse the new sample
//      against the existing maps; structurally not cheaper.
//   2. Coalescing — if 200 events arrive in 10ms, we only need to rebuild
//      the snapshot *once* before re-emitting; not 200 times.
//
// Future optimisation (deferred): cache the per-function map incrementally
// and only rebuild the tree on snapshot. For now `snapshot()` is fast
// enough at trace volumes a single Python service produces.

/// Mutable state for an in-progress aggregate. One instance per stream
/// (or per one-shot file load). Not `Send` on its own; live use wraps it
/// in `Arc<Mutex<…>>`.
#[derive(Default)]
pub struct Aggregator {
    samples: Vec<StackSample>,
    services: BTreeSet<String>,
    pods: BTreeSet<String>,
    min_t: Option<i64>,
    max_t: Option<i64>,
    total_events: u32,
}

impl Aggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of events successfully ingested (mirrors the legacy
    /// `total_events` counter — includes events with no usable stack;
    /// excludes lines that failed to parse).
    pub fn total_events(&self) -> u32 {
        self.total_events
    }

    /// Push one parsed event into the in-progress aggregate. Skips events
    /// the legacy code would silently drop (missing `period_ns`, empty
    /// frames, etc.) so the on-wire counts match byte-for-byte.
    ///
    /// Private because `RawEvent` is an internal wire-shape — callers
    /// from outside this module ingest via [`Aggregator::ingest_value`]
    /// (live broadcast payload) or [`Aggregator::ingest_line`] (one
    /// JSONL line).
    fn ingest_event(&mut self, ev: RawEvent) {
        self.total_events += 1;

        if let Some(t) = ev.time.as_ref().and_then(RawTime::to_micros) {
            if self.min_t.map_or(true, |m| t < m) {
                self.min_t = Some(t);
            }
            if self.max_t.map_or(true, |m| t > m) {
                self.max_t = Some(t);
            }
        }
        if let Some(s) = &ev.service {
            if !s.is_empty() {
                self.services.insert(s.clone());
            }
        }
        if let Some(p) = &ev.pod {
            if !p.is_empty() {
                self.pods.insert(p.clone());
            }
        }

        // function_call events don't carry period_ns; handle them BEFORE
        // the sampler-only period_ns gate.
        if ev.event_type == "function_call" {
            let dur_ns = ev.duration_ns.unwrap_or(0);
            if dur_ns <= 0 {
                return;
            }
            let qualname = match ev.qualname.as_deref() {
                Some(q) if !q.is_empty() => q.to_string(),
                _ => return,
            };
            let qualified_name_for_frame = Some(qualname.clone());
            let frame = RawFrame {
                name: qualname,
                file: ev.file,
                line: ev.line,
                qualified_name: qualified_name_for_frame,
                module: None,
                is_system: None,
            };
            self.samples.push(StackSample {
                weight_us: dur_ns / 1000,
                tick_count: 1,
                cpu: ev.cpu,
                frames: vec![frame],
            });
            return;
        }

        let period_ns = ev.period_ns.unwrap_or(0);
        if period_ns <= 0 {
            return;
        }

        match ev.event_type.as_str() {
            "wall_trace" | "cpu_trace" => {
                let count = ev.count.unwrap_or(0);
                if count <= 0 {
                    return;
                }
                if let Some(frames) = ev.frames {
                    if frames.is_empty() {
                        return;
                    }
                    self.samples.push(StackSample {
                        weight_us: (count.saturating_mul(period_ns)) / 1000,
                        tick_count: count,
                        cpu: ev.cpu,
                        frames,
                    });
                }
            }
            "wall_profile" | "cpu_profile" => {
                let cpu = ev.cpu;
                if let Some(sub_samples) = ev.samples {
                    for s in sub_samples {
                        if s.count <= 0 {
                            continue;
                        }
                        let frames = match s.frames {
                            Some(f) if !f.is_empty() => f,
                            _ => continue,
                        };
                        self.samples.push(StackSample {
                            weight_us: (s.count.saturating_mul(period_ns)) / 1000,
                            tick_count: s.count,
                            cpu,
                            frames,
                        });
                    }
                }
            }
            _ => {} // unknown event type — silently drop, same as legacy
        }
    }

    /// Same as `ingest_event` but accepts a raw JSON value — the shape
    /// the realtime transport produces (one inner `payload.payload` per
    /// broadcast). Lines that fail to deserialize are silently skipped,
    /// matching the file path's behavior.
    pub fn ingest_value(&mut self, v: &serde_json::Value) {
        if let Ok(ev) = serde_json::from_value::<RawEvent>(v.clone()) {
            self.ingest_event(ev);
        }
    }

    /// Same as `ingest_event` but accepts a raw JSONL line. Empty/blank
    /// lines and malformed JSON are silently skipped.
    pub fn ingest_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Ok(ev) = serde_json::from_str::<RawEvent>(trimmed) {
            self.ingest_event(ev);
        }
    }

    /// Build a fresh `AggregateReport` from the accumulated state.
    /// `source_file` is echoed into the report unchanged — for live
    /// streams the caller passes the log path so the UI's "saved to …"
    /// messaging keeps working.
    pub fn snapshot(&self, source_file: &str) -> AggregateReport {
        build_report(
            &self.samples,
            &self.services,
            &self.pods,
            self.min_t,
            self.max_t,
            self.total_events,
            source_file,
        )
    }
}

// ---------------------------------------------------------------------------
// Public API

/// Parse the JSONL file at `path` and return an aggregated profiling report.
/// Thin wrapper around `Aggregator` so the file-load and live-stream paths
/// can't drift.
pub fn aggregate(path: &Path) -> Result<AggregateReport> {
    let file = fs::File::open(path)
        .map_err(|e| anyhow!("open {}: {e}", path.display()))?;
    let reader = BufReader::new(file);

    let mut agg = Aggregator::new();
    for line in reader.lines().map_while(|r| r.ok()) {
        agg.ingest_line(&line);
    }
    Ok(agg.snapshot(&path.to_string_lossy()))
}

// ---------------------------------------------------------------------------
// Rollup — moved out of `aggregate()` so `Aggregator::snapshot` can call
// it on demand. Pure function: same inputs → same `AggregateReport`.
fn build_report(
    samples: &[StackSample],
    services: &BTreeSet<String>,
    pods: &BTreeSet<String>,
    min_t: Option<i64>,
    max_t: Option<i64>,
    total_events: u32,
    source_file: &str,
) -> AggregateReport {

    // ------------------------------------------------------------ per-function rollup
    //
    // For each sample we walk frames leaf-first:
    //   - the leaf gets `total_us` (self time) AND `cumulative_us`,
    //   - every name on the stack gets `cumulative_us` — but ONLY once
    //     per stack (we dedupe with `seen`) so recursive frames don't
    //     double-count.
    let mut stats: HashMap<String, FunctionStat> = HashMap::new();
    let mut cpu_acc: HashMap<String, (f64, u32)> = HashMap::new();
    for sample in samples.iter() {
        let mut seen: HashSet<String> = HashSet::new();
        for (i, frame) in sample.frames.iter().enumerate() {
            let is_first_in_stack = seen.insert(frame.name.clone());
            let entry = stats
                .entry(frame.name.clone())
                .or_insert_with(|| FunctionStat {
                    qualname: frame.name.clone(),
                    ncalls: 0,
                    total_us: 0,
                    cumulative_us: 0,
                    percall_us: 0.0,
                    errors: 0,
                    cpu_avg: None,
                    file: frame.file.clone(),
                    line: frame.line,
                });
            if entry.file.is_none() && frame.file.is_some() {
                entry.file = frame.file.clone();
            }
            if entry.line.is_none() && frame.line.is_some() {
                entry.line = frame.line;
            }
            if is_first_in_stack {
                entry.cumulative_us += sample.weight_us;
            }
            // Leaf accumulation — i == 0 because frames is leaf-first.
            if i == 0 {
                entry.total_us += sample.weight_us;
                entry.ncalls = entry.ncalls.saturating_add(
                    u32::try_from(sample.tick_count).unwrap_or(u32::MAX),
                );
                if let Some(cpu) = sample.cpu {
                    let acc = cpu_acc.entry(frame.name.clone()).or_insert((0.0, 0));
                    acc.0 += cpu;
                    acc.1 += 1;
                }
            }
        }
    }
    for (qn, fs) in stats.iter_mut() {
        if fs.ncalls > 0 {
            fs.percall_us = fs.total_us as f64 / fs.ncalls as f64;
        }
        if let Some((sum, n)) = cpu_acc.get(qn) {
            if *n > 0 {
                fs.cpu_avg = Some(*sum / *n as f64);
            }
        }
    }
    let mut functions: Vec<FunctionStat> = stats.into_values().collect();
    functions.sort_by(|a, b| {
        b.cumulative_us
            .cmp(&a.cumulative_us)
            .then(b.ncalls.cmp(&a.ncalls))
            .then_with(|| a.qualname.cmp(&b.qualname))
    });

    // ------------------------------------------------------------ aggregated tree
    let tree = build_tree_from_samples(samples);

    AggregateReport {
        source_file: source_file.to_string(),
        started_at: min_t.map(us_to_iso),
        ended_at: max_t.map(us_to_iso),
        duration_us: max_t.unwrap_or(0) - min_t.unwrap_or(0),
        total_events,
        total_calls: total_events,
        unmatched_starts: 0,
        unmatched_ends: 0,
        services: services.iter().cloned().collect(),
        pods: pods.iter().cloned().collect(),
        functions,
        tree,
        calls: Vec::new(),
        calls_truncated: false,
    }
}

/// List `*.log` and `*.jsonl` files in `dir`, sorted by mtime desc.
///
/// Returns empty Vec if the directory does not exist — the UI treats that
/// as "no scans yet" rather than an error.
pub fn list_logs(dir: &Path) -> Result<Vec<EventLogMeta>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out: Vec<EventLogMeta> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| anyhow!("read_dir {}: {e}", dir.display()))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "log" && ext != "jsonl" {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified_iso = md.modified().ok().map(|t| {
            let dt: DateTime<Utc> = t.into();
            dt.to_rfc3339()
        });
        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unnamed)")
            .to_string();
        out.push(EventLogMeta {
            path: path.to_string_lossy().into_owned(),
            display_name,
            size_bytes: md.len(),
            modified_iso,
        });
    }
    // newest first
    out.sort_by(|a, b| b.modified_iso.cmp(&a.modified_iso));
    Ok(out)
}

/// Default directory the UI lists from. Created lazily by the calling
/// command if it doesn't exist.
pub fn default_logs_dir() -> Option<PathBuf> {
    dirs_home().map(|home| home.join(".drift").join("event_logs"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// helpers

fn parse_iso_us(s: &str) -> Option<i64> {
    // The Python writer emits ISO-8601 UTC with 'Z'. chrono parses both
    // 'Z' and '+00:00' forms; microsecond precision survives.
    let dt = DateTime::parse_from_rfc3339(s).ok()?;
    let us = dt.timestamp() * 1_000_000 + i64::from(dt.timestamp_subsec_micros());
    Some(us)
}

fn us_to_iso(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let micros = us.rem_euclid(1_000_000) as u32;
    DateTime::<Utc>::from_timestamp(secs, micros * 1_000)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .unwrap_or_default()
}

/// Phase F3: construct a stable identifier for a `TreeNode` matching
/// the static profiler's `CallTreeNode.id` format (`file::class::name`).
/// When `qualified_name` is present and looks class-qualified (contains
/// a `.` but not a CPython `<locals>` segment for closures), the part
/// before the last `.` is treated as the class. Otherwise we fall
/// back to `file::name`. Either way the resulting id is the same
/// string the static profiler would produce for the same symbol, so
/// a viewer can join sampled and static profiles on this field.
///
/// Examples:
///   `OrderService.create` at `/app/orders.py`
///       → `/app/orders.py::OrderService::create`
///   `top_level` (no qualname; Py 3.7-3.10) at `/app/main.py`
///       → `/app/main.py::top_level`
///   `inner` with qualname `outer.<locals>.inner`
///       → `/app/main.py::inner`  (closure — no real class)
fn make_node_id(file: Option<&str>, name: &str, qualified_name: Option<&str>) -> String {
    let file_part = file.unwrap_or("");
    if let Some(qn) = qualified_name.filter(|q| !q.is_empty()) {
        // CPython encodes closure scopes as `outer.<locals>.inner` —
        // these aren't class methods, so treat as a free function.
        if !qn.contains("<locals>") {
            if let Some(idx) = qn.rfind('.') {
                let class = &qn[..idx];
                let method = &qn[idx + 1..];
                if !class.is_empty() && !method.is_empty() {
                    return format!("{}::{}::{}", file_part, class, method);
                }
            }
        }
    }
    format!("{}::{}", file_part, name)
}

/// Build the aggregated tree by walking each stack root → leaf. Each
/// frame on the path accumulates the sample's full weight as inclusive
/// time; the leaf additionally takes the weight as self time.
///
/// `frames` arrives leaf-first (matches the on-the-wire schema); we
/// reverse to root-first for tree insertion.
fn build_tree_from_samples(samples: &[StackSample]) -> TreeNode {
    struct Builder {
        value: i64,
        self_value: i64,
        ncalls: u32,
        file: Option<String>,
        line: Option<u32>,
        // Phase F3 carry-through. Latched on first sighting (`is_none`
        // check before assigning) — multiple samples for the same
        // (file, name) should yield identical metadata; if they
        // disagree, the first wins. Consistent with the existing
        // file/line latching policy above.
        qualname: Option<String>,
        module: Option<String>,
        is_system: Option<bool>,
        children: BTreeMap<String, Builder>,
    }
    impl Builder {
        fn empty() -> Self {
            Self {
                value: 0,
                self_value: 0,
                ncalls: 0,
                file: None,
                line: None,
                qualname: None,
                module: None,
                is_system: None,
                children: BTreeMap::new(),
            }
        }
        fn into_node(self, name: String, depth: u32) -> TreeNode {
            let mut child_nodes: Vec<TreeNode> = self
                .children
                .into_iter()
                .map(|(n, b)| b.into_node(n, depth + 1))
                .collect();
            child_nodes.sort_by(|a, b| b.value.cmp(&a.value));
            // Phase F3: stable id matching the static profiler's
            // `CallTreeNode.id` format (`file::class::name`).
            let node_id = make_node_id(
                self.file.as_deref(),
                &name,
                self.qualname.as_deref(),
            );
            TreeNode {
                name,
                value: self.value,
                self_value: self.self_value,
                ncalls: self.ncalls,
                depth,
                file: self.file,
                line: self.line,
                children: child_nodes,
                node_id,
                qualname: self.qualname,
                module: self.module,
                is_system: self.is_system,
            }
        }
    }

    let mut root = Builder::empty();
    for sample in samples {
        // Reverse leaf-first → root-first for tree walking.
        let root_first: Vec<&RawFrame> = sample.frames.iter().rev().collect();
        if root_first.is_empty() {
            continue;
        }
        let len = root_first.len();
        let mut node: &mut Builder = &mut root;
        for (i, frame) in root_first.iter().enumerate() {
            // borrow-checker dance: must reassign node to the child
            // ref, not nest into a temporary.
            node = node
                .children
                .entry(frame.name.clone())
                .or_insert_with(Builder::empty);
            node.value += sample.weight_us;
            if node.file.is_none() && frame.file.is_some() {
                node.file = frame.file.clone();
            }
            if node.line.is_none() && frame.line.is_some() {
                node.line = frame.line;
            }
            // Phase F3: carry F1a/F1b frame metadata into the tree
            // node. Latched on first sighting — same policy as
            // file/line. Empty string `qualified_name`s are treated
            // as absent (the wire schema drops them too, but a
            // hand-built test fixture might pass through).
            if node.qualname.is_none() {
                if let Some(qn) = frame.qualified_name.as_deref().filter(|q| !q.is_empty()) {
                    node.qualname = Some(qn.to_string());
                }
            }
            if node.module.is_none() {
                if let Some(m) = frame.module.as_deref().filter(|m| !m.is_empty()) {
                    node.module = Some(m.to_string());
                }
            }
            if node.is_system.is_none() {
                if let Some(s) = frame.is_system {
                    node.is_system = Some(s);
                }
            }
            // The leaf in root-first order is the LAST element (index len-1).
            if i == len - 1 {
                node.self_value += sample.weight_us;
                node.ncalls = node.ncalls.saturating_add(1);
            }
        }
    }

    let mut top_children: Vec<TreeNode> = root
        .children
        .into_iter()
        .map(|(name, b)| b.into_node(name, 1))
        .collect();
    let root_value: i64 = top_children.iter().map(|c| c.value).sum();
    let root_ncalls: u32 = top_children.iter().map(|c| c.ncalls).sum();
    top_children.sort_by(|a, b| b.value.cmp(&a.value));
    TreeNode {
        name: "<root>".into(),
        value: root_value,
        self_value: 0,
        ncalls: root_ncalls,
        depth: 0,
        file: None,
        line: None,
        children: top_children,
        // Phase F3: synthetic root has no source location, so the
        // node_id is just the marker. Optional fields stay None.
        node_id: "<root>".into(),
        qualname: None,
        module: None,
        is_system: None,
    }
}

// ---------------------------------------------------------------------------
// tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    fn wall_trace(time: &str, count: i64, frames: &[(&str, &str, u32)]) -> String {
        let frames_json = frames
            .iter()
            .map(|(n, f, l)| format!(r#"{{"name":"{n}","file":"{f}","line":{l}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"type":"wall_trace","time":"{time}","service":"svc","pod":"pod","period_ns":10000000,"duration_ns":1000000000,"count":{count},"cpu":0.5,"memory_bytes":1024,"frames":[{frames_json}]}}"#
        )
    }

    #[test]
    fn empty_file_aggregates_to_zero() {
        let f = write_fixture(&[]);
        let r = aggregate(f.path()).unwrap();
        assert_eq!(r.total_events, 0);
        assert_eq!(r.total_calls, 0);
        assert_eq!(r.functions.len(), 0);
        assert!(r.tree.children.is_empty());
    }

    #[test]
    fn single_wall_trace_event_populates_function_and_tree() {
        // count=5 ticks × period 10ms = 50ms of work
        let f = write_fixture(&[&wall_trace(
            "2026-05-19T12:00:00.000000Z",
            5,
            &[("leaf_fn", "/app/x.py", 10), ("caller_fn", "/app/y.py", 20)],
        )]);
        let r = aggregate(f.path()).unwrap();

        assert_eq!(r.total_events, 1);
        assert_eq!(r.functions.len(), 2);

        let leaf = r
            .functions
            .iter()
            .find(|x| x.qualname == "leaf_fn")
            .unwrap();
        // 5 × 10_000_000 ns / 1000 = 50_000 μs
        assert_eq!(leaf.total_us, 50_000);
        assert_eq!(leaf.cumulative_us, 50_000);
        assert_eq!(leaf.ncalls, 5); // tick count rolled into ncalls

        let caller = r
            .functions
            .iter()
            .find(|x| x.qualname == "caller_fn")
            .unwrap();
        // caller is not leaf → no self time, all inclusive.
        assert_eq!(caller.total_us, 0);
        assert_eq!(caller.cumulative_us, 50_000);

        // Tree: root → caller_fn → leaf_fn
        assert_eq!(r.tree.children.len(), 1);
        assert_eq!(r.tree.children[0].name, "caller_fn");
        assert_eq!(r.tree.children[0].value, 50_000);
        assert_eq!(r.tree.children[0].children.len(), 1);
        assert_eq!(r.tree.children[0].children[0].name, "leaf_fn");
        assert_eq!(r.tree.children[0].children[0].self_value, 50_000);
    }

    #[test]
    fn cpu_trace_events_aggregate_alongside_wall() {
        // Same stack, both event types — should share the tree node.
        let cpu = format!(
            r#"{{"type":"cpu_trace","time":"2026-05-19T12:00:01.000000Z","service":"svc","pod":"pod","period_ns":10000000,"duration_ns":1000000000,"count":3,"cpu":1.2,"memory_bytes":2048,"frames":[{{"name":"leaf_fn","file":"/x","line":1}}]}}"#
        );
        let f = write_fixture(&[
            &wall_trace("2026-05-19T12:00:00.000000Z", 2, &[("leaf_fn", "/x", 1)]),
            &cpu,
        ]);
        let r = aggregate(f.path()).unwrap();
        // 2 + 3 = 5 ticks × 10_000_000 / 1000 = 50_000 μs
        let leaf = r
            .functions
            .iter()
            .find(|x| x.qualname == "leaf_fn")
            .unwrap();
        assert_eq!(leaf.total_us, 50_000);
        assert_eq!(leaf.ncalls, 5);
    }

    #[test]
    fn bundle_mode_expands_into_samples() {
        // wall_profile carrying 2 distinct stacks via `samples[]`
        let line = r#"{"type":"wall_profile","profile_type":"wall","time":"2026-05-19T12:00:00.000000Z","time_ns":0,"service":"svc","pod":"pod","period_ns":10000000,"duration_ns":1000000000,"cpu":0.0,"memory_bytes":0,"sample_type":[{"type":"sample","unit":"count"},{"type":"wall","unit":"nanoseconds"}],"samples":[{"count":4,"value_ns":40000000,"frames":[{"name":"a","file":"/a","line":1}],"labels":{}},{"count":1,"value_ns":10000000,"frames":[{"name":"b","file":"/b","line":2}],"labels":{}}]}"#;
        let f = write_fixture(&[line]);
        let r = aggregate(f.path()).unwrap();

        assert_eq!(r.total_events, 1); // one wire event
        let a = r.functions.iter().find(|x| x.qualname == "a").unwrap();
        let b = r.functions.iter().find(|x| x.qualname == "b").unwrap();
        assert_eq!(a.total_us, 40_000); // 4 × 10ms
        assert_eq!(b.total_us, 10_000); // 1 × 10ms
    }

    #[test]
    fn recursive_frames_dedupe_inclusive_time() {
        // Same name twice in one stack — cumulative should not inflate.
        let f = write_fixture(&[&wall_trace(
            "2026-05-19T12:00:00.000000Z",
            7,
            &[("recur", "/r", 1), ("recur", "/r", 1)],
        )]);
        let r = aggregate(f.path()).unwrap();
        let recur = r.functions.iter().find(|x| x.qualname == "recur").unwrap();
        // weight = 70_000 μs; cumulative counted once not twice.
        assert_eq!(recur.cumulative_us, 70_000);
        // The leaf occurrence still gets self time once.
        assert_eq!(recur.total_us, 70_000);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let f = write_fixture(&[
            "not json",
            "",
            r#"{"type":"???"}"#, // unknown event type → skipped
            &wall_trace("2026-05-19T12:00:00.000000Z", 1, &[("g", "/", 1)]),
        ]);
        let r = aggregate(f.path()).unwrap();
        // 3 non-empty lines parsed; 2 ignored (one malformed JSON dropped by
        // serde, one unknown-type kept but skipped inside the match). The
        // wire-event count is best-effort and includes all parsed-OK lines.
        assert!(r.total_events >= 1);
        let g = r.functions.iter().find(|x| x.qualname == "g").unwrap();
        assert_eq!(g.total_us, 10_000);
    }

    #[test]
    fn services_and_pods_are_collected() {
        let f = write_fixture(&[
            &wall_trace("2026-05-19T12:00:00.000000Z", 1, &[("a", "/", 1)]),
            // Different service via a hand-crafted line
            r#"{"type":"wall_trace","time":"2026-05-19T12:00:01.000000Z","service":"other","pod":"other-pod","period_ns":10000000,"duration_ns":1000000000,"count":1,"cpu":0.0,"memory_bytes":0,"frames":[{"name":"a","file":"/","line":1}]}"#,
        ]);
        let r = aggregate(f.path()).unwrap();
        assert!(r.services.contains(&"svc".to_string()));
        assert!(r.services.contains(&"other".to_string()));
    }

    /// Integration test that aggregates a REAL events.log file produced
    /// by the docker-compose stack. Skipped by default — set the
    /// `DRIFT_LIVE_FILE` env var to a `.jsonl` path to opt in.
    ///
    /// Wire up like so:
    ///
    ///   curl http://localhost:8080/events/log > /tmp/events.jsonl
    ///   DRIFT_LIVE_FILE=/tmp/events.jsonl cargo test live_file_smoke \
    ///     -- --ignored --nocapture
    ///
    /// Asserts the bare minimum: the file parsed, produced events,
    /// produced a non-empty tree, and produced at least one frame.
    #[test]
    #[ignore]
    fn live_file_smoke() {
        let path = std::env::var("DRIFT_LIVE_FILE")
            .expect("set DRIFT_LIVE_FILE=<path/to/events.jsonl>");
        let report = aggregate(Path::new(&path)).expect("aggregate failed");
        println!("=== live_file_smoke ===");
        println!("source           : {}", report.source_file);
        println!("started_at       : {:?}", report.started_at);
        println!("ended_at         : {:?}", report.ended_at);
        println!("duration_us      : {}", report.duration_us);
        println!("total_events     : {}", report.total_events);
        println!("services         : {:?}", report.services);
        println!("pods             : {:?}", report.pods);
        println!("functions        : {} unique", report.functions.len());
        for fs in report.functions.iter().take(5) {
            println!(
                "  {:<35}  cum {:>10} μs  self {:>10} μs  ncalls {:>4}",
                fs.qualname, fs.cumulative_us, fs.total_us, fs.ncalls
            );
        }
        println!(
            "tree             : root value {} μs across {} top-level child(ren)",
            report.tree.value,
            report.tree.children.len(),
        );

        assert!(report.total_events > 0, "no events parsed");
        assert!(!report.functions.is_empty(), "no functions extracted");
        assert!(!report.tree.children.is_empty(), "tree has no children");
        assert!(report.tree.value > 0, "tree root value is zero");
        // sampler input → calls[] empty by design (no per-call concept)
        assert!(
            report.calls.is_empty(),
            "calls[] should be empty for sampler input"
        );
    }

    #[test]
    fn calls_array_stays_empty_for_sampler_input() {
        let f = write_fixture(&[&wall_trace(
            "2026-05-19T12:00:00.000000Z",
            5,
            &[("a", "/", 1)],
        )]);
        let r = aggregate(f.path()).unwrap();
        assert!(r.calls.is_empty());
        assert_eq!(r.unmatched_starts, 0);
        assert_eq!(r.unmatched_ends, 0);
    }

    // ------------------------------------------------------- Phase F3 join keys

    #[test]
    fn node_id_format_falls_back_to_file_name_without_qualname() {
        // No qualified_name available (Py 3.7-3.10 fixture). The id
        // is `file::name` — the static profiler's fallback format
        // when class info isn't known.
        assert_eq!(
            make_node_id(Some("/app/x.py"), "foo", None),
            "/app/x.py::foo"
        );
        // Empty qualified_name treated as absent.
        assert_eq!(
            make_node_id(Some("/a.py"), "f", Some("")),
            "/a.py::f"
        );
    }

    #[test]
    fn node_id_format_uses_qualname_class_when_present() {
        // The static profiler's CallTreeNode.id format is
        // `file::class::name`. With `OrderService.create` we recover
        // the class.
        assert_eq!(
            make_node_id(
                Some("/app/orders.py"),
                "create",
                Some("OrderService.create"),
            ),
            "/app/orders.py::OrderService::create"
        );
    }

    #[test]
    fn node_id_format_treats_locals_qualname_as_free_function() {
        // CPython encodes closures as `outer.<locals>.inner` — those
        // aren't class methods, so the id falls back to `file::name`.
        assert_eq!(
            make_node_id(
                Some("/app/x.py"),
                "inner",
                Some("outer.<locals>.inner"),
            ),
            "/app/x.py::inner"
        );
    }

    #[test]
    fn wall_trace_with_f1_metadata_populates_node_id_qualname_module_is_system() {
        // Hand-craft a wire event with the F1a/F1b fields on the
        // leaf frame, and assert the aggregator forwards them onto
        // the corresponding tree node.
        let line = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.000000Z","service":"svc","pod":"pod","period_ns":10000000,"duration_ns":1000000000,"count":5,"cpu":0.5,"memory_bytes":1024,"frames":[{"name":"create","file":"/app/orders.py","line":23,"qualified_name":"OrderService.create","module":"orders.service","language":"python","is_native":false,"is_system":false}]}"#;
        let f = write_fixture(&[line]);
        let r = aggregate(f.path()).unwrap();
        // First (and only) leaf under root.
        let leaf = &r.tree.children[0];
        assert_eq!(leaf.name, "create");
        assert_eq!(leaf.node_id, "/app/orders.py::OrderService::create");
        assert_eq!(leaf.qualname.as_deref(), Some("OrderService.create"));
        assert_eq!(leaf.module.as_deref(), Some("orders.service"));
        assert_eq!(leaf.is_system, Some(false));
    }

    #[test]
    fn wall_trace_without_f1_metadata_still_populates_node_id() {
        // Legacy 3-field frame (no F1a/F1b enrichment). node_id still
        // populates (`file::name`); the optional fields stay None.
        let line = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.000000Z","service":"svc","pod":"pod","period_ns":10000000,"duration_ns":1000000000,"count":5,"cpu":0.5,"memory_bytes":1024,"frames":[{"name":"create","file":"/app/orders.py","line":23}]}"#;
        let f = write_fixture(&[line]);
        let r = aggregate(f.path()).unwrap();
        let leaf = &r.tree.children[0];
        assert_eq!(leaf.node_id, "/app/orders.py::create");
        assert_eq!(leaf.qualname, None);
        assert_eq!(leaf.module, None);
        assert_eq!(leaf.is_system, None);
    }

    #[test]
    fn function_call_event_populates_qualname_in_tree() {
        // function_call's `qualname` field IS the qualified name
        // (the @trace decorator captures it from `__qualname__`).
        // After F3 the aggregator forwards it into the synthesized
        // frame's qualified_name slot.
        let line = r#"{"type":"function_call","time":"2026-05-19T12:00:00.000000Z","service":"svc","pod":"pod","qualname":"OrderService.create","file":"/app/orders.py","line":23,"duration_ns":3284,"status":"ok"}"#;
        let f = write_fixture(&[line]);
        let r = aggregate(f.path()).unwrap();
        // function_call synthesizes a single-frame stack; the leaf is
        // the qualname.
        let leaf = &r.tree.children[0];
        assert_eq!(leaf.name, "OrderService.create");
        // node_id picks up the class because qualified_name now propagates.
        assert_eq!(leaf.node_id, "/app/orders.py::OrderService::create");
        assert_eq!(leaf.qualname.as_deref(), Some("OrderService.create"));
    }

    /// Regression test for the silent-drop bug: the Supabase Realtime
    /// publisher emits `time` as an integer of nanoseconds since epoch.
    /// Before [`RawTime`] tolerated that, `RawEvent` failed to
    /// deserialize and the broadcast was dropped without a log line —
    /// users saw "Waiting for first broadcast" forever despite events
    /// flowing on the channel.
    ///
    /// The JSON below is the exact `payload` shape the user pasted from
    /// the Supabase channel inspector — extra fields (`is_native`,
    /// `language`, `memory_bytes`, `duration_ns` on a wall_trace, etc.)
    /// are present to prove serde doesn't trip on them either.
    #[test]
    fn ingest_value_accepts_publisher_wire_shape_with_integer_time() {
        let payload = serde_json::json!({
            "count": 1000,
            "cpu": 4.2412109375,
            "duration_ns": 10000000000_i64,
            "frames": [
                {
                    "file": "/usr/local/lib/python3.14/threading.py",
                    "is_native": false,
                    "is_system": true,
                    "language": "python",
                    "line": 373,
                    "module": "threading",
                    "name": "wait",
                    "qualified_name": "Condition.wait"
                },
                {
                    "file": "/usr/local/lib/python3.14/threading.py",
                    "is_native": false,
                    "is_system": true,
                    "language": "python",
                    "line": 670,
                    "module": "threading",
                    "name": "wait",
                    "qualified_name": "Event.wait"
                }
            ],
            "memory_bytes": 61423616_i64,
            "memory_peak_bytes": 60653568_i64,
            "period_ns": 10_000_000_i64,
            "pod": "demo-app-py314-776d947b7-glfmb",
            "service": "test-python-web-server-py314",
            "time": 1_779_236_876_872_834_300_i64,
            "type": "wall_trace"
        });

        let mut agg = Aggregator::new();
        agg.ingest_value(&payload);
        assert_eq!(
            agg.total_events(),
            1,
            "the publisher's integer-`time` payload must be ingested, not silently dropped",
        );
        let report = agg.snapshot("smoke");
        assert!(
            report.total_calls > 0,
            "ingest must produce at least one sample for the wall_trace",
        );
        // service / pod must be picked up so the desktop UI's summary
        // labels light up.
        assert!(report.services.iter().any(|s| s == "test-python-web-server-py314"));
    }

    #[test]
    fn raw_time_accepts_both_string_and_integer() {
        // The two shapes seen in production. Each must convert to
        // microseconds without panicking and without losing precision
        // worse than 1 µs.
        let iso: RawTime =
            serde_json::from_value(serde_json::json!("2026-05-20T00:27:16.971Z")).unwrap();
        assert!(iso.to_micros().is_some());

        let nanos: RawTime =
            serde_json::from_value(serde_json::json!(1_779_236_876_872_834_300_i64)).unwrap();
        // Integer ns → µs is a clean divide-by-1000.
        assert_eq!(nanos.to_micros(), Some(1_779_236_876_872_834));
    }
}
