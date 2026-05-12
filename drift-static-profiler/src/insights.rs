//! Insights / findings — structured, severity-scored issues attached to
//! each `CallTreeNode`. See `INSIGHTS_PLAN.md` for the full design.
//!
//! Detectors are pure functions called from `tree::build_inner` next to
//! the existing Phase D risk-flag computation. The result is pushed into
//! `CallTreeNode.findings`, and the existing booleans
//! (`n_plus_one_risk`, `blocking_in_async`, `is_recursive`) become
//! **derived** from those findings so older consumers and flame-mode
//! `'smells'` keep working unchanged.
//!
//! Hot-zone detection needs the whole tree built first (pagerank
//! percentile is a graph-wide quantity), so it runs as a single
//! post-build pass `attach_hot_zones` invoked from `Report::build`.

use crate::categories::Category;
use crate::graph::ExternalCall;
use crate::Symbol;
use serde::{Deserialize, Serialize};

// ───────────────────────────────────────────────────────────────────────────
// Public types
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    NPlusOne,
    BlockingInAsync,
    Recursive,
    SmellyLoop,
    NoisyLog,
    OutdatedPackage,
    MemoryExplosion,
    HotZone,
}

impl FindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FindingKind::NPlusOne => "n_plus_one",
            FindingKind::BlockingInAsync => "blocking_in_async",
            FindingKind::Recursive => "recursive",
            FindingKind::SmellyLoop => "smelly_loop",
            FindingKind::NoisyLog => "noisy_log",
            FindingKind::OutdatedPackage => "outdated_package",
            FindingKind::MemoryExplosion => "memory_explosion",
            FindingKind::HotZone => "hot_zone",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// Method name; or "import" / "loop" / etc. for non-call evidence.
    pub call: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    /// 0..1. Higher = more confident this is a real problem.
    pub confidence: f64,
    /// Call-site or relevant line within the symbol (not symbol-start).
    pub line: usize,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

/// Top-N pointer for `Summary.findings_top` — same role as `pagerank_top`,
/// just shaped for findings. Resolved by the viewer via `nodeIndex.byId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingTopRef {
    /// Same value as `CallTreeNode.id` — `file::class::name`.
    pub node_id: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub line: usize,
}

// ───────────────────────────────────────────────────────────────────────────
// Per-node detector entrypoint (called from tree::build_inner)
// ───────────────────────────────────────────────────────────────────────────

/// Inputs the per-node detectors need that aren't on `Symbol` /
/// `&[ExternalCall]`. Currently empty; we'll grow it as detectors land.
pub struct Ctx {}

impl Default for Ctx {
    fn default() -> Self {
        Self {}
    }
}

/// Collect every node-local finding for a single symbol. Called once per
/// node from `tree::build_inner` alongside the existing Phase D booleans.
///
/// **Pure function** — no global state, no IO. Tree-cross-referencing
/// detectors (hot_zone, log-on-hot-path) live in `attach_*` instead.
pub fn collect_node_findings(
    sym: &Symbol,
    externals: &[ExternalCall],
    _ctx: &Ctx,
) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(detect_n_plus_one(sym, externals));
    out.extend(detect_blocking_in_async(sym, externals));
    out.extend(detect_noisy_log_in_loop(sym, externals));
    out
}

/// Log calls inside a loop: every iteration writes a line. On a hot
/// request path this is a classic cause of disk + log-pipeline pressure.
pub fn detect_noisy_log_in_loop(sym: &Symbol, externals: &[ExternalCall]) -> Vec<Finding> {
    let offenders: Vec<&ExternalCall> = externals
        .iter()
        .filter(|e| e.in_loop && is_method_call(&e.name) && matches!(e.category, Category::Log))
        .collect();
    if offenders.is_empty() {
        return Vec::new();
    }
    let evidence: Vec<Evidence> = offenders
        .iter()
        .map(|e| Evidence {
            call: match &e.receiver {
                Some(r) => format!("{r}.{}", e.name),
                None => e.name.clone(),
            },
            line: e.line,
            category: Some(e.category),
        })
        .collect();
    let first_line = offenders[0].line;
    let n = offenders.len();
    let message = format!(
        "{n} log call(s) inside a loop in `{}`. Each iteration writes a line; expect log-pipeline cost proportional to N.",
        sym.name,
    );
    let remediation = Some(
        "Aggregate or sample: log once per batch with a count, or use a rate-limited logger / sampler."
            .to_string(),
    );
    vec![Finding {
        kind: FindingKind::NoisyLog,
        // Default Medium — log-in-loop is bad but rarely a P1 by itself.
        // bump_by_impact in attach_hot_log_findings can raise it when the
        // symbol is on a hot path.
        severity: Severity::Medium,
        confidence: 0.85,
        line: first_line,
        message,
        evidence,
        remediation,
    }]
}

/// Detect db/network/io calls in an async function that aren't awaited —
/// the classic "I made it async but it's still blocking" antipattern.
/// Same predicate that today drives `CallTreeNode.blocking_in_async`.
pub fn detect_blocking_in_async(sym: &Symbol, externals: &[ExternalCall]) -> Vec<Finding> {
    if !sym.is_async {
        return Vec::new();
    }
    let offenders: Vec<&ExternalCall> = externals
        .iter()
        .filter(|e| {
            !e.in_await
                && is_method_call(&e.name)
                && matches!(
                    e.category,
                    Category::Db | Category::Network | Category::Io
                )
        })
        .collect();
    if offenders.is_empty() {
        return Vec::new();
    }

    let confidence = offenders
        .iter()
        .map(|e| match e.tier {
            crate::categories::ClassifyTier::ImportedModule => 0.95_f64,
            crate::categories::ClassifyTier::ReceiverPattern => 0.80,
            crate::categories::ClassifyTier::MethodSignature => 0.65,
        })
        .fold(0.0_f64, f64::max);

    let evidence: Vec<Evidence> = offenders
        .iter()
        .map(|e| Evidence {
            call: match &e.receiver {
                Some(r) => format!("{r}.{}", e.name),
                None => e.name.clone(),
            },
            line: e.line,
            category: Some(e.category),
        })
        .collect();

    let first_line = offenders[0].line;
    let n = offenders.len();
    let summary_calls = offenders
        .iter()
        .take(3)
        .map(|e| match &e.receiver {
            Some(r) => format!("{r}.{}()", e.name),
            None => format!("{}()", e.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if n > 3 { format!(", +{} more", n - 3) } else { String::new() };

    let message = format!(
        "{n} I/O call(s) in async `{}` are not awaited: {summary_calls}{suffix}. These block the event loop — defeating the point of `async`.",
        sym.name,
    );
    let remediation = Some(
        "Use the async client (e.g. `httpx.AsyncClient` instead of `requests`, `aiomysql` instead of `pymysql`) and `await` each call."
            .to_string(),
    );

    vec![Finding {
        kind: FindingKind::BlockingInAsync,
        severity: Severity::High,
        confidence,
        line: first_line,
        message,
        evidence,
        remediation,
    }]
}


/// True for callees that look like method names rather than constructors.
/// Excludes PascalCase callees because those are class instantiations
/// (`httpx.AsyncClient()`, `new HttpClient()`) — they don't perform
/// blocking I/O or N+1 queries on their own.
fn is_method_call(name: &str) -> bool {
    !name
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
}

/// Detect `db` / `cache` calls inside loops — the classic N+1 pattern.
/// Same predicate that today drives `CallTreeNode.n_plus_one_risk`; we
/// group all such calls in this symbol into ONE finding with multi-line
/// evidence so the UI shows the cluster, not one row per call.
pub fn detect_n_plus_one(sym: &Symbol, externals: &[ExternalCall]) -> Vec<Finding> {
    let offenders: Vec<&ExternalCall> = externals
        .iter()
        .filter(|e| {
            e.in_loop
                && is_method_call(&e.name)
                && matches!(e.category, Category::Db | Category::Cache)
        })
        .collect();
    if offenders.is_empty() {
        return Vec::new();
    }

    // Highest classifier confidence wins. Tier B (imported_module) is
    // most reliable, Tier D (method_signature) least.
    let confidence = offenders
        .iter()
        .map(|e| match e.tier {
            crate::categories::ClassifyTier::ImportedModule => 0.95_f64,
            crate::categories::ClassifyTier::ReceiverPattern => 0.80,
            crate::categories::ClassifyTier::MethodSignature => 0.65,
        })
        .fold(0.0_f64, f64::max);

    let evidence: Vec<Evidence> = offenders
        .iter()
        .map(|e| Evidence {
            call: match &e.receiver {
                Some(r) => format!("{r}.{}", e.name),
                None => e.name.clone(),
            },
            line: e.line,
            category: Some(e.category),
        })
        .collect();

    // Anchor the finding at the first offender's line (call-site, not
    // symbol-start), so the Insights tab and code-jump land directly on
    // the offending call.
    let first_line = offenders[0].line;
    let n = offenders.len();
    let call_summary = offenders
        .iter()
        .take(3)
        .map(|e| match &e.receiver {
            Some(r) => format!("{r}.{}()", e.name),
            None => format!("{}()", e.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if n > 3 { format!(", +{} more", n - 3) } else { String::new() };
    let message = format!(
        "{n} {} call(s) inside a loop in `{}`: {call_summary}{suffix}. Each loop iteration issues a separate query — classic N+1.",
        if matches!(offenders[0].category, Category::Cache) { "cache" } else { "db" },
        sym.name,
    );
    let remediation = Some(
        "Batch the calls outside the loop: collect inputs first, then issue ONE query (e.g. SQLAlchemy `bulk_save_objects`, Django `bulk_create`, TypeORM `save([...])`)."
            .to_string(),
    );

    vec![Finding {
        kind: FindingKind::NPlusOne,
        // Base severity is High because db/cache in a loop is almost
        // always a real problem. The caller can still bump it via
        // bump_by_impact when the node is on a hot path.
        severity: Severity::High,
        confidence,
        line: first_line,
        message,
        evidence,
        remediation,
    }]
}

// ───────────────────────────────────────────────────────────────────────────
// Derived-boolean helpers
// ───────────────────────────────────────────────────────────────────────────
//
// The existing CallTreeNode booleans (n_plus_one_risk, blocking_in_async,
// is_recursive) stay as cached, derived values computed from findings so
// older consumers and flame-mode 'smells' keep working unchanged. These
// helpers centralize the "which kind maps to which bool" decision.

pub fn has_kind(findings: &[Finding], kind: FindingKind) -> bool {
    findings.iter().any(|f| f.kind == kind)
}

// ───────────────────────────────────────────────────────────────────────────
// Severity helper
// ───────────────────────────────────────────────────────────────────────────

/// Bump a detector's base severity by impact signals already on the node.
/// `pagerank_p90` is computed once by the caller from the graph's pageranks.
///
/// Boost rule (deterministic, no randomness):
///   boost = (percent_total >= 20) + (pagerank >= p90) + (call_site_count >= 10)
///
///   Low + 3 → High
///   Low + 2 → Medium
///   Medium + 2..=3 → High
///   anything else → unchanged
pub fn bump_by_impact(
    percent_total: f64,
    pagerank: f64,
    call_site_count: usize,
    base: Severity,
    pagerank_p90: f64,
) -> Severity {
    let boost = (percent_total >= 20.0) as u8
        + (pagerank >= pagerank_p90 && pagerank_p90 > 0.0) as u8
        + (call_site_count >= 10) as u8;
    match (base, boost) {
        (Severity::Low, 3) => Severity::High,
        (Severity::Low, 2) => Severity::Medium,
        (Severity::Medium, 2..=3) => Severity::High,
        (s, _) => s,
    }
}

/// Compute the 90th-percentile pagerank across the project's symbols.
/// Used by `bump_by_impact`. Returns 0.0 if the input is empty so the
/// bump rule degrades gracefully (no nodes get the pagerank boost).
pub fn compute_pagerank_p90<I>(scores: I) -> f64
where
    I: IntoIterator<Item = f64>,
{
    let mut v: Vec<f64> = scores.into_iter().collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() as f64) * 0.90).floor() as usize;
    let i = idx.min(v.len() - 1);
    v[i]
}

// ───────────────────────────────────────────────────────────────────────────
// Post-build pass: hot_zone cross-reference
// ───────────────────────────────────────────────────────────────────────────

/// Walk every tree in `entries` and push `HotZone` findings onto nodes
/// that satisfy the hot-zone criteria. Runs **after** all per-node
/// detectors have populated their findings, so it can read them too.
///
/// No-op for step 1+2. Real implementation lands in step 11.
pub fn attach_hot_zones(_entries: &mut [crate::tree::CallTreeNode], _pagerank_p90: f64) {
    // No-op for step 1+2. Real implementation lands in step 11.
}

/// Walk every tree in `entries` and bump `NoisyLog` severity when the
/// finding sits on a hot-path symbol (pagerank ≥ p90). Same data the
/// hot_zone detector will use in step 11 — we expose it here so the
/// noisy-log finding gets the right severity even before hot_zone lands.
pub fn attach_hot_log_findings(entries: &mut [crate::tree::CallTreeNode], pagerank_p90: f64) {
    if pagerank_p90 <= 0.0 {
        return;
    }
    fn walk(node: &mut crate::tree::CallTreeNode, p90: f64) {
        let on_hot_path = node.pagerank >= p90;
        if on_hot_path {
            for f in node.findings.iter_mut() {
                if f.kind == FindingKind::NoisyLog && f.severity == Severity::Medium {
                    f.severity = Severity::High;
                }
            }
        }
        for c in node.children.iter_mut() {
            walk(c, p90);
        }
    }
    for e in entries.iter_mut() {
        walk(e, pagerank_p90);
    }
}

/// Walk every tree in `entries` and push a `Recursive` finding onto each
/// node where `is_recursive` is true. We do this as a post-build pass
/// because recursion is a graph-wide property — `Symbol` alone doesn't
/// know about SCC membership.
pub fn attach_recursive_findings(entries: &mut [crate::tree::CallTreeNode]) {
    fn walk(node: &mut crate::tree::CallTreeNode) {
        if node.is_recursive && !has_kind(&node.findings, FindingKind::Recursive) {
            // Build the finding inline — we have all the info we need on
            // the node itself, no need to look up the Symbol again.
            node.findings.push(Finding {
                kind: FindingKind::Recursive,
                severity: Severity::Medium,
                confidence: 1.0,
                line: node.line,
                message: format!(
                    "`{}` participates in a recursion cycle (mutual or self). Verify there is a base case and a bounded recursion depth.",
                    node.name,
                ),
                evidence: Vec::new(),
                remediation: Some(
                    "Confirm termination invariants. If the recursion depth scales with input size, consider an explicit loop or tail-recursion equivalent."
                        .to_string(),
                ),
            });
        }
        for c in node.children.iter_mut() {
            walk(c);
        }
    }
    for e in entries.iter_mut() {
        walk(e);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Summary rollup helpers
// ───────────────────────────────────────────────────────────────────────────

/// Walk every tree in `entries` and produce a top-N list of findings for
/// `Summary.findings_top`. Sorted by `severity DESC` then by source order
/// (deterministic). `cap` limits the list length.
pub fn collect_findings_top(
    entries: &[crate::tree::CallTreeNode],
    cap: usize,
) -> Vec<FindingTopRef> {
    let mut out: Vec<FindingTopRef> = Vec::new();
    for e in entries {
        walk_for_top(e, &mut out);
    }
    out.sort_by(|a, b| severity_rank(b.severity).cmp(&severity_rank(a.severity)));
    out.truncate(cap);
    out
}

fn walk_for_top(node: &crate::tree::CallTreeNode, out: &mut Vec<FindingTopRef>) {
    for f in &node.findings {
        out.push(FindingTopRef {
            node_id: node.id.0.clone(),
            kind: f.kind,
            severity: f.severity,
            line: f.line,
        });
    }
    for c in &node.children {
        walk_for_top(c, out);
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Low => 0,
        Severity::Medium => 1,
        Severity::High => 2,
    }
}

/// Count findings per kind across every node in every tree.
pub fn collect_findings_by_kind(
    entries: &[crate::tree::CallTreeNode],
) -> std::collections::BTreeMap<String, usize> {
    let mut map: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in entries {
        walk_for_kinds(e, &mut map);
    }
    map
}

fn walk_for_kinds(
    node: &crate::tree::CallTreeNode,
    map: &mut std::collections::BTreeMap<String, usize>,
) {
    for f in &node.findings {
        *map.entry(f.kind.as_str().to_string()).or_default() += 1;
    }
    for c in &node.children {
        walk_for_kinds(c, map);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_kind_as_str_matches_serde_snake_case() {
        // Verify the manual as_str() matches what serde will emit.
        let cases = [
            (FindingKind::NPlusOne, "n_plus_one"),
            (FindingKind::BlockingInAsync, "blocking_in_async"),
            (FindingKind::Recursive, "recursive"),
            (FindingKind::SmellyLoop, "smelly_loop"),
            (FindingKind::NoisyLog, "noisy_log"),
            (FindingKind::OutdatedPackage, "outdated_package"),
            (FindingKind::MemoryExplosion, "memory_explosion"),
            (FindingKind::HotZone, "hot_zone"),
        ];
        for (k, expected) in cases {
            assert_eq!(k.as_str(), expected);
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{expected}\""), "serde mismatch for {k:?}");
        }
    }

    #[test]
    fn pagerank_p90_basic() {
        // 10 evenly-spaced values; p90 should be near the 9th (index 9 of 10).
        let v: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let p90 = compute_pagerank_p90(v.iter().copied());
        assert_eq!(p90, 10.0, "p90 of 1..=10 should pick the top value");
    }

    #[test]
    fn pagerank_p90_empty() {
        assert_eq!(compute_pagerank_p90(std::iter::empty::<f64>()), 0.0);
    }

    #[test]
    fn pagerank_p90_single() {
        assert_eq!(compute_pagerank_p90(std::iter::once(0.42)), 0.42);
    }

    #[test]
    fn bump_low_to_high_with_three_signals() {
        let bumped = bump_by_impact(25.0, 0.99, 12, Severity::Low, 0.50);
        assert_eq!(bumped, Severity::High, "3 signals should promote Low → High");
    }

    #[test]
    fn bump_low_to_medium_with_two_signals() {
        let bumped = bump_by_impact(25.0, 0.99, 0, Severity::Low, 0.50);
        assert_eq!(bumped, Severity::Medium, "2 signals should promote Low → Medium");
    }

    #[test]
    fn bump_medium_to_high_with_two_signals() {
        let bumped = bump_by_impact(25.0, 0.99, 0, Severity::Medium, 0.50);
        assert_eq!(bumped, Severity::High);
    }

    #[test]
    fn bump_keeps_high_high() {
        let bumped = bump_by_impact(0.0, 0.0, 0, Severity::High, 0.50);
        assert_eq!(bumped, Severity::High, "High stays High regardless of impact");
    }

    #[test]
    fn bump_zero_threshold_ignored() {
        // pagerank threshold of 0 (empty graph case) shouldn't count toward boost
        let bumped = bump_by_impact(0.0, 0.0, 0, Severity::Low, 0.0);
        assert_eq!(bumped, Severity::Low);
    }

    #[test]
    fn collect_node_findings_is_noop_for_now() {
        // Sanity: step-1 invariant — collect_node_findings returns empty until
        // detectors land in step 4+.
        let sym = Symbol {
            name: "f".into(),
            kind: crate::SymbolKind::Function,
            file: std::path::PathBuf::from("x.py"),
            line: 1,
            line_end: 5,
            byte_start: 0,
            byte_end: 100,
            parent: None,
            loc: 5,
            complexity: 1,
            nesting_depth: 0,
            parameter_count: 0,
            is_async: false,
            loop_ranges: vec![],
            await_ranges: vec![],
        };
        let out = collect_node_findings(&sym, &[], &Ctx::default());
        assert!(out.is_empty(), "step-1 invariant: detectors are no-ops");
    }
}
