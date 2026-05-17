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

/// Rough fix-cost estimate — the second axis alongside severity.
/// Mirrors SonarQube's remediation-time signal and Sentry's "is this a
/// quick win" surface. Values are intentionally coarse; the goal is to
/// split "immediate fixes" (Trivial / Small) from "full refactor"
/// (Large) without pretending we can predict engineering minutes.
///
///   Trivial — < ~10 min: add `await`, rename, one-line replace
///   Small   — ~30 min:   extract helper, swap library call, one method
///   Medium  — ~half day: rewrite a method, redesign one interaction
///   Large   — ≥ a day:   multi-file rewrite, architecture-shape change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Trivial,
    Small,
    Medium,
    Large,
}

impl Effort {
    /// Rank for sorting: lower = easier to fix.
    pub fn rank(self) -> u8 {
        match self {
            Effort::Trivial => 0,
            Effort::Small => 1,
            Effort::Medium => 2,
            Effort::Large => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// High-complexity symbol called from many sites — refactor lever.
    /// Time saved per microsecond multiplies by `call_site_count`.
    /// Inspired by pprof's "hot function" + V8's high-% functions.
    ExpensiveCompute,
    /// Repeated invocations of a non-trivial, side-effect-free symbol —
    /// a memoization / lru_cache candidate. SonarQube tag analog:
    /// `clumsy` (unnecessary repetition).
    MissingCaching,
    /// Many log statements clustered on a hot-path symbol — the cost
    /// scales with traffic. Better-named cousin of "overkill logs".
    /// SonarQube tag analog: `pitfall` (works now, will hurt at scale).
    LogAmplification,
    /// A SQL string matches a known **query-shape** anti-pattern —
    /// `SELECT *`, missing `WHERE` on DELETE/UPDATE, implicit columns
    /// in INSERT, leading-wildcard `LIKE`, etc. Fires on embedded SQL
    /// literals (`cursor.execute("…")`) AND on statements inside `.sql`
    /// files alike — the rule recognizes the AST shape, not the source.
    /// Carries the rule id (e.g. `SQL001`) in `evidence[0].call`.
    /// Detector lives in `src/sql_lint.rs` under the `SQL*` prefix.
    SqlAntipattern,
    /// A DDL statement matches a known **migration-safety** hazard —
    /// `CREATE INDEX` without `CONCURRENTLY`, `DROP COLUMN`, `ALTER
    /// COLUMN … TYPE`, `ADD CONSTRAINT … FOREIGN KEY` without
    /// `NOT VALID`, `ADD COLUMN … NOT NULL` without `DEFAULT`, etc.
    /// Distinct from [`Self::SqlAntipattern`] because the concern is
    /// "this schema change will cause an outage during deploy", not
    /// "this query is inefficient" — different audience (ops vs app
    /// dev), different severity calibration, different remediation
    /// playbook. Catalog sources: squawk (clean-room) + strong_migrations.
    /// Carries the rule id (e.g. `MIG_CREATE_INDEX_NOT_CONCURRENT`) in
    /// `evidence[0].call`. Detector lives in `src/sql_lint.rs` under
    /// the `MIG_*` prefix.
    MigrationSafety,
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
            FindingKind::ExpensiveCompute => "expensive_compute",
            FindingKind::MissingCaching => "missing_caching",
            FindingKind::LogAmplification => "log_amplification",
            FindingKind::SqlAntipattern => "sql_antipattern",
            FindingKind::MigrationSafety => "migration_safety",
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
    /// Rough fix-cost estimate. Composes with severity into "immediate
    /// fixes" (high × Trivial/Small) and "refactor candidates" (Large
    /// or multi-finding clusters). Defaults to `Medium` for old fixtures.
    #[serde(default = "default_effort")]
    pub effort: Effort,
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

fn default_effort() -> Effort { Effort::Medium }

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

/// Per-root rollup for `Summary.roots_overview`.
///
/// Answers "what's the shape of this entry point?" at a glance, the way
/// pprof's `top -cum` answers "what's the biggest function?". Each row
/// is per *initial root* (entry point), and the breakdowns are over
/// that root's transitive subtree.
///
/// All fields are derived from data already on `CallTreeNode` — this is
/// a viewer convenience, not a new analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootOverview {
    /// Same as the matching `CallTreeNode.id` — the viewer uses this
    /// to deep-link into `/scan/:fixtureKey/node/:nodeId`.
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,
    pub kind: crate::SymbolKind,
    /// Total reachable symbol count from this root (its `subtree_size`).
    pub subtree_size: usize,
    /// Share of all roots' combined subtree_size — like pprof's `% cum`.
    pub percent_of_all_roots: f64,
    /// Categories transitively reached, copied from the node's
    /// `categories_reached` so the viewer can render the chip row
    /// without walking the tree again.
    pub categories_reached: std::collections::BTreeMap<String, usize>,
    /// Counts of findings in this root's subtree broken down by
    /// severity (high/medium/low) — the per-root health view.
    pub findings_by_severity: std::collections::BTreeMap<String, usize>,
    /// Total findings in this root's subtree.
    pub findings_total: usize,
    /// In-graph callers of THIS root. For a "true entry point"
    /// (HTTP handler, main, cron) this is empty — that's the signal.
    /// For non-entry roots (analyze-root output) this lists who calls in.
    pub callers: Vec<CallerSummary>,
    /// First-level callees of this root — "where does this root go
    /// first?". Capped at 5 to keep the JSON small; the viewer can drill
    /// for more via the node detail page.
    pub first_callees: Vec<CalleeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerSummary {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalleeSummary {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,
    pub subtree_size: usize,
}

// ───────────────────────────────────────────────────────────────────────────
// Derived rollups: immediate fixes + refactor candidates
// ───────────────────────────────────────────────────────────────────────────

/// One row in `Summary.refactor_candidates`. A node-level aggregate: this
/// SYMBOL needs a serious look, not one one-line patch. Composed when a
/// node carries multiple findings or a Large-effort finding, or is just
/// a "god function" (loc ≥ 100) with at least one issue.
///
/// Modeled on SonarQube's "technical debt by file" view + Sentry's
/// per-function aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorCandidate {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,
    /// How many distinct findings live on this node.
    pub findings_count: usize,
    /// Sorted unique kinds present on this node.
    pub kinds: Vec<FindingKind>,
    /// Worst severity any finding on this node carries.
    pub worst_severity: Severity,
    /// Heaviest effort any finding on this node carries — typically the
    /// driver of "this needs more than a patch".
    pub max_effort: Effort,
    pub complexity: usize,
    pub loc: usize,
    pub percent_total: f64,
    /// Short human reason — pre-rendered so the viewer doesn't have to.
    pub why: String,
}

/// Walk every tree and produce the refactor-candidate list. Sorted by
/// (findings_count DESC, loc + complexity DESC) so the worst cluster is
/// first.
pub fn collect_refactor_candidates(
    entries: &[crate::tree::CallTreeNode],
    cap: usize,
) -> Vec<RefactorCandidate> {
    let mut out: Vec<RefactorCandidate> = Vec::new();
    fn walk(node: &crate::tree::CallTreeNode, out: &mut Vec<RefactorCandidate>) {
        let f_count = node.findings.len();
        let has_large = node.findings.iter().any(|f| matches!(f.effort, Effort::Large));
        let god_function_with_finding = node.loc >= 100 && f_count >= 1;
        let cluster = f_count >= 2;
        if cluster || has_large || god_function_with_finding {
            let worst_sev = node
                .findings
                .iter()
                .map(|f| f.severity)
                .max_by_key(|s| match s {
                    Severity::Low => 0,
                    Severity::Medium => 1,
                    Severity::High => 2,
                })
                .unwrap_or(Severity::Low);
            let max_eff = node
                .findings
                .iter()
                .map(|f| f.effort)
                .max_by_key(|e| e.rank())
                .unwrap_or(Effort::Medium);
            let mut kinds: Vec<FindingKind> = node.findings.iter().map(|f| f.kind).collect();
            kinds.sort_by_key(|k| k.as_str());
            kinds.dedup();
            let why = if cluster && has_large {
                format!("{} findings ({}) including a Large-effort one — full refactor",
                    f_count,
                    kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "))
            } else if cluster {
                format!("{} findings clustered on one symbol ({})",
                    f_count,
                    kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "))
            } else if has_large {
                "single Large-effort finding (e.g. high-complexity rewrite)".to_string()
            } else {
                format!("god function: {} LOC with {} finding(s)", node.loc, f_count)
            };
            out.push(RefactorCandidate {
                node_id: node.id.0.clone(),
                name: node.name.clone(),
                file: node.file.clone(),
                line: node.line,
                parent_class: node.parent_class.clone(),
                findings_count: f_count,
                kinds,
                worst_severity: worst_sev,
                max_effort: max_eff,
                complexity: node.complexity,
                loc: node.loc,
                percent_total: node.percent_total,
                why,
            });
        }
        for c in &node.children {
            walk(c, out);
        }
    }
    for e in entries {
        walk(e, &mut out);
    }
    out.sort_by(|a, b| {
        b.findings_count.cmp(&a.findings_count)
            .then_with(|| (b.loc + b.complexity).cmp(&(a.loc + a.complexity)))
            .then_with(|| a.file.cmp(&b.file))
    });
    out.truncate(cap);
    out
}

/// Pull the "quick wins" out of the flat finding list: severity ≥ Medium
/// AND effort ≤ Small. Sorted by (severity DESC, effort ASC) — biggest
/// impact at lowest cost first.
///
/// This is the "what should I do RIGHT NOW" list. SonarQube users
/// recognize this as the "5-min fix" / "10-min fix" lane.
pub fn collect_immediate_fixes(
    entries: &[crate::tree::CallTreeNode],
    cap: usize,
) -> Vec<ImmediateFix> {
    let mut out: Vec<ImmediateFix> = Vec::new();
    fn walk(node: &crate::tree::CallTreeNode, out: &mut Vec<ImmediateFix>) {
        for f in &node.findings {
            let is_immediate = !matches!(f.severity, Severity::Low)
                && matches!(f.effort, Effort::Trivial | Effort::Small);
            if is_immediate {
                out.push(ImmediateFix {
                    node_id: node.id.0.clone(),
                    name: node.name.clone(),
                    file: node.file.clone(),
                    line: f.line,
                    parent_class: node.parent_class.clone(),
                    kind: f.kind,
                    severity: f.severity,
                    effort: f.effort,
                    message: f.message.clone(),
                });
            }
        }
        for c in &node.children {
            walk(c, out);
        }
    }
    for e in entries {
        walk(e, &mut out);
    }
    out.sort_by(|a, b| {
        severity_rank(b.severity).cmp(&severity_rank(a.severity))
            .then_with(|| a.effort.rank().cmp(&b.effort.rank()))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    // Per-kind cap — diversity gate so no single rule pack monopolizes
    // the top-N. Reason: a repo with 300 high-severity MIG findings on
    // .sql migrations would otherwise displace every code-level finding
    // (n_plus_one, missing_caching, expensive_compute, …) from the
    // user-visible "what should I fix now?" list — the desktop app's
    // 24-row Insights pane would show 24 migrations and zero code.
    // Algorithm: keep severity ordering (high beats medium), but admit
    // a finding only if its kind's quota isn't exhausted. Quota is
    // `max(3, cap/4)` so the top-N is guaranteed to be at least
    // 4-kind-diverse when 4+ kinds have qualifying findings.
    out = enforce_per_kind_cap(out, |f| f.kind, per_kind_quota(cap));
    out.truncate(cap);
    out
}

/// Per-kind quota that admits at least 3 findings of any present kind
/// but caps any single kind at one-quarter of the overall cap.
///
/// Examples:
///   - cap 24 → quota 6   (4 kinds × 6 = 24)
///   - cap 50 → quota 12  (4 kinds × 12 = 48)
///   - cap 12 → quota 3   (4 kinds × 3 = 12; floor kicks in)
fn per_kind_quota(cap: usize) -> usize {
    (cap / 4).max(3)
}

/// Diversity gate. Walks `items` in their pre-sorted order (caller is
/// responsible for sorting first) and admits each item only when its
/// kind's accumulated count is below `quota`. Returns the diversified
/// list — same order as the input, just with items past their kind's
/// quota dropped.
///
/// Generic over the item + key extractor so the same primitive serves
/// `collect_immediate_fixes`, `collect_findings_top`, and any future
/// "diverse top-N" rollup. SRP-clean and unit-testable in isolation.
fn enforce_per_kind_cap<T, K, F>(items: Vec<T>, key: F, quota: usize) -> Vec<T>
where
    K: std::hash::Hash + Eq,
    F: Fn(&T) -> K,
{
    if quota == 0 {
        return Vec::new();
    }
    let mut by_kind: std::collections::HashMap<K, usize> = std::collections::HashMap::new();
    items
        .into_iter()
        .filter(|it| {
            let n = by_kind.entry(key(it)).or_insert(0);
            if *n < quota {
                *n += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

/// Quick-win pointer row for `Summary.immediate_fixes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmediateFix {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,
    pub kind: FindingKind,
    pub severity: Severity,
    pub effort: Effort,
    pub message: String,
}

/// Build a `RootOverview` for every entry in `entries`, sorted by
/// `subtree_size` descending (biggest reach first — same ordering the
/// viewer's existing Roots tab uses).
pub fn collect_roots_overview(
    entries: &[crate::tree::CallTreeNode],
) -> Vec<RootOverview> {
    let total_subtree: usize = entries.iter().map(|e| e.subtree_size).sum();

    let mut out: Vec<RootOverview> = entries
        .iter()
        .map(|e| {
            let mut by_sev: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let total = count_findings_by_severity(e, &mut by_sev);
            let callers = e
                .callers
                .iter()
                .map(|c| CallerSummary {
                    node_id: c.id.0.clone(),
                    name: c.name.clone(),
                    file: c.file.clone(),
                    line: c.line,
                    parent_class: c.parent_class.clone(),
                })
                .collect();
            let first_callees = e
                .children
                .iter()
                .take(5)
                .map(|c| CalleeSummary {
                    node_id: c.id.0.clone(),
                    name: c.name.clone(),
                    file: c.file.clone(),
                    line: c.line,
                    parent_class: c.parent_class.clone(),
                    subtree_size: c.subtree_size,
                })
                .collect();
            RootOverview {
                node_id: e.id.0.clone(),
                name: e.name.clone(),
                file: e.file.clone(),
                line: e.line,
                parent_class: e.parent_class.clone(),
                kind: e.kind.clone(),
                subtree_size: e.subtree_size,
                percent_of_all_roots: if total_subtree > 0 {
                    (e.subtree_size as f64 / total_subtree as f64) * 100.0
                } else {
                    0.0
                },
                categories_reached: e.categories_reached.clone(),
                findings_by_severity: by_sev,
                findings_total: total,
                callers,
                first_callees,
            }
        })
        .collect();

    out.sort_by(|a, b| b.subtree_size.cmp(&a.subtree_size));
    out
}

fn count_findings_by_severity(
    node: &crate::tree::CallTreeNode,
    out: &mut std::collections::BTreeMap<String, usize>,
) -> usize {
    let mut total = 0usize;
    for f in &node.findings {
        let key = match f.severity {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        };
        *out.entry(key.to_string()).or_default() += 1;
        total += 1;
    }
    for c in &node.children {
        total += count_findings_by_severity(c, out);
    }
    total
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
///
/// Synthetic symbols (like the `<module>` one tags.rs synthesizes for
/// files with module-level executable code) are profiler-internal: their
/// loc/complexity are file-wide proxies, not real code-unit metrics.
/// Running detectors on them would produce false positives — e.g.
/// `expensive_compute` firing on a 90-line script's `<module>` just
/// because the file is long. Skip them here. Findings in their SUBTREE
/// (real functions they call) still roll up correctly because the
/// tree-walking rollups visit every descendant.
pub fn collect_node_findings(
    sym: &Symbol,
    externals: &[ExternalCall],
    _ctx: &Ctx,
) -> Vec<Finding> {
    if is_synthetic_symbol(&sym.name) {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(detect_n_plus_one(sym, externals));
    out.extend(detect_blocking_in_async(sym, externals));
    out.extend(detect_noisy_log_in_loop(sym, externals));
    out.extend(detect_expensive_compute(sym));
    out
}

/// True for profiler-internal synthetic symbol names. The leading `<`
/// can't appear in any identifier in any of the seven supported
/// languages, so a single-character check is sufficient and future-proof
/// for new synthetics like `<class-body>` or `<lambda-N>` if we add them.
pub fn is_synthetic_symbol(name: &str) -> bool {
    name.starts_with('<')
}

/// Detect heavyweight pure-compute symbols that are good refactor levers:
///   - high cyclomatic complexity (many decision points → hard to test, easy to slow)
///   - OR very long bodies (loc) → split into smaller units
///   - OR deep nesting → readability + bug surface
///
/// We deliberately do NOT consider `call_site_count` here — that's the
/// IMPACT multiplier, applied by `bump_severities_by_impact` post-build.
/// Keeping detection and severity separate matches pprof's design
/// (cost dimensions are layered orthogonally on top of "this function").
pub fn detect_expensive_compute(sym: &Symbol) -> Vec<Finding> {
    // Thresholds picked to match common static-analysis defaults
    // (Sonar's `cognitive complexity`, ESLint's `complexity`, etc.):
    //   complexity ≥ 10  → "high"
    //   complexity ≥ 15  → "very high" (already a Sonar default warning)
    //   loc        ≥ 80  → "god function" candidate
    //   nesting    ≥ 4   → "too nested"
    let high_complexity = sym.complexity >= 10;
    let very_high_complexity = sym.complexity >= 15;
    let long_body = sym.loc >= 80;
    let deep_nesting = sym.nesting_depth >= 4;

    if !(high_complexity || long_body || deep_nesting) {
        return Vec::new();
    }

    let base = if very_high_complexity || sym.loc >= 150 {
        Severity::High
    } else if high_complexity || long_body {
        Severity::Medium
    } else {
        Severity::Low
    };

    // Build a concrete reason string so the UI doesn't have to invent one.
    let mut reasons: Vec<String> = Vec::new();
    if very_high_complexity {
        reasons.push(format!("complexity {} (≥15: very high)", sym.complexity));
    } else if high_complexity {
        reasons.push(format!("complexity {} (≥10: high)", sym.complexity));
    }
    if long_body {
        reasons.push(format!("{} lines of code", sym.loc));
    }
    if deep_nesting {
        reasons.push(format!("nesting depth {} (≥4)", sym.nesting_depth));
    }
    let message = format!(
        "`{}` is a heavyweight unit — {}. Likely refactor lever: each call site pays this cost.",
        sym.name,
        reasons.join(", "),
    );
    let remediation = Some(
        "Split into smaller helpers, extract pure sub-functions, or memoize if the inputs are stable. Each microsecond saved multiplies by call_site_count."
            .to_string(),
    );

    // Effort scales with the body: tiny-but-branchy → Small (extract
    // helpers); long body or deep nesting → Large (real rewrite).
    let effort = if sym.loc >= 150 || sym.nesting_depth >= 5 {
        Effort::Large
    } else if sym.loc >= 80 || sym.complexity >= 15 {
        Effort::Medium
    } else {
        Effort::Small
    };

    vec![Finding {
        kind: FindingKind::ExpensiveCompute,
        severity: base,
        effort,
        // Confidence is naturally lower than data-bearing findings (n_plus_one,
        // outdated_package): high complexity is a smell, not always a bug.
        confidence: 0.70,
        line: sym.line,
        message,
        evidence: Vec::new(),
        remediation,
    }]
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
        // Trivial: move the log outside the loop, or drop the level.
        effort: Effort::Trivial,
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
        // Usually Trivial: swap to the async client + add `await`.
        // The whole repair is local to a few lines.
        effort: Effort::Trivial,
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
        // Usually a Small fix: replace the loop with a bulk_* API or
        // collect-then-call. Only Medium when the loop body is complex.
        effort: Effort::Small,
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

/// Walk every tree and bump each finding's severity by the node's own
/// impact signals (`percent_total`, `pagerank`, `call_site_count`).
/// Runs AFTER per-node detectors AND after Phase C populated the
/// percentages — without this pass, every finding stays at its base
/// severity regardless of where it sits in the call graph.
///
/// Matches the design every top profiler uses:
///   - pprof: red+thick = "high cum impact"
///   - V8: "callers < 2% hidden" → only impactful frames shown
///   - PyCharm: red function = "expensive on hot path"
///
/// We delegate per-finding policy to `bump_by_impact` (a node + base
/// severity + p90 → new severity), so the rule stays in one place.
pub fn bump_severities_by_impact(
    entries: &mut [crate::tree::CallTreeNode],
    pagerank_p90: f64,
) {
    if pagerank_p90 <= 0.0 {
        // No graph data → nothing to bump.
        return;
    }
    fn walk(node: &mut crate::tree::CallTreeNode, p90: f64) {
        for f in node.findings.iter_mut() {
            // `HotZone` findings are themselves IMPACT findings — bumping
            // them by their own impact would double-count. Skip.
            if f.kind == FindingKind::HotZone {
                continue;
            }
            f.severity = bump_by_impact(
                node.percent_total,
                node.pagerank,
                node.call_site_count,
                f.severity,
                p90,
            );
        }
        for c in node.children.iter_mut() {
            walk(c, p90);
        }
    }
    for e in entries.iter_mut() {
        walk(e, pagerank_p90);
    }
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

/// Walk every tree and flag symbols that look like memoization candidates:
///   - called frequently (`call_site_count` ≥ MIN)
///   - non-trivial body  (`complexity` ≥ MIN_COMPLEX or `loc` ≥ MIN_LOC)
///   - no side-effecting externals (no db/network/io/queue calls)
///   - not a constructor (PascalCase) and not recursive
///
/// The "pure-ish" heuristic is deliberately lenient: we can't prove
/// purity statically, so we lower confidence to 0.55 and write the
/// assumption into the message so the user can sanity-check.
///
/// Inspired by SonarQube's `clumsy` tag (unnecessary repetition) and
/// Sentry's "wider frame = more frequent" surface for cache candidates.
pub fn attach_missing_caching_findings(entries: &mut [crate::tree::CallTreeNode]) {
    const MIN_CALL_SITES: usize = 5;
    const MIN_COMPLEX: usize = 5;
    const MIN_LOC: usize = 20;

    fn looks_pure(node: &crate::tree::CallTreeNode) -> bool {
        node.external_calls.iter().all(|e| {
            !matches!(
                e.category,
                Category::Db | Category::Network | Category::Io | Category::Queue
            )
        })
    }
    fn is_constructor(name: &str) -> bool {
        name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
    }

    fn walk(node: &mut crate::tree::CallTreeNode) {
        // One finding per qualifying symbol — skip if anything is wrong.
        let qualifies = node.call_site_count >= MIN_CALL_SITES
            && (node.complexity >= MIN_COMPLEX || node.loc >= MIN_LOC)
            && !node.is_recursive
            && !is_constructor(&node.name)
            && looks_pure(node)
            && !has_kind(&node.findings, FindingKind::MissingCaching);
        if qualifies {
            let why = format!(
                "called {} times across the project; complexity {}, {} lines, no I/O externals — looks pure",
                node.call_site_count, node.complexity, node.loc,
            );
            node.findings.push(Finding {
                kind: FindingKind::MissingCaching,
                // Default Medium — wins are real but conditional on
                // inputs actually being cacheable. bump_by_impact will
                // promote to High when the symbol is on a hot path.
                severity: Severity::Medium,
                // Memoization is typically a 1-2 hour addition: pick a
                // cache (lru_cache / @functools.lru_cache / Caffeine /
                // moka), key the inputs, ship.
                effort: Effort::Small,
                confidence: 0.55,
                line: node.line,
                message: format!(
                    "`{}` is a repeated, non-trivial pure-ish unit ({why}). Memoize / lru_cache to amortize the cost.",
                    node.name,
                ),
                evidence: Vec::new(),
                remediation: Some(
                    "Add a memoization layer keyed by the inputs. \
                     Python: `@functools.lru_cache(maxsize=...)`. \
                     JS: a Map-backed memo. \
                     Java: Caffeine or Guava's CacheBuilder. \
                     Rust: a HashMap behind a Once/RwLock or the `cached` crate."
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

/// Walk every tree and flag symbols whose log volume scales with traffic:
///   - at least 3 log calls in this symbol
///   - on a hot path (pagerank ≥ p90)  OR  called from many sites
///
/// "Log amplification" is a better name than "overkill logs": it names
/// the actual harm — cost scaling with call volume — instead of editing
/// taste. Distinct from `noisy_log` which fires for the in-loop pattern.
/// Inspired by SonarQube's `pitfall` tag (works now, will hurt at scale).
pub fn attach_log_amplification_findings(
    entries: &mut [crate::tree::CallTreeNode],
    pagerank_p90: f64,
) {
    const MIN_LOG_CALLS: usize = 3;
    const MIN_CALL_SITES: usize = 10;

    fn walk(
        node: &mut crate::tree::CallTreeNode,
        p90: f64,
    ) {
        let log_count = node
            .external_calls
            .iter()
            .filter(|e| matches!(e.category, Category::Log))
            .count();
        let on_hot_path = (p90 > 0.0 && node.pagerank >= p90)
            || node.call_site_count >= MIN_CALL_SITES;
        let already = has_kind(&node.findings, FindingKind::LogAmplification);
        if log_count >= MIN_LOG_CALLS && on_hot_path && !already {
            let evidence: Vec<Evidence> = node
                .external_calls
                .iter()
                .filter(|e| matches!(e.category, Category::Log))
                .take(5)
                .map(|e| Evidence {
                    call: match &e.receiver {
                        Some(r) => format!("{r}.{}", e.name),
                        None => e.name.clone(),
                    },
                    line: e.line,
                    category: Some(e.category),
                })
                .collect();
            let reason = if p90 > 0.0 && node.pagerank >= p90 {
                "high-pagerank symbol"
            } else {
                "high call-site count"
            };
            node.findings.push(Finding {
                kind: FindingKind::LogAmplification,
                severity: Severity::Medium,
                // Quick: drop a few logs to DEBUG, or sample.
                effort: Effort::Trivial,
                confidence: 0.80,
                line: node.line,
                message: format!(
                    "`{}` emits {} log call(s) on a {} — cost amplifies with traffic.",
                    node.name, log_count, reason,
                ),
                evidence,
                remediation: Some(
                    "Move chatty logs to DEBUG (so prod can disable them), aggregate per-batch, or rate-limit via a sampler."
                        .to_string(),
                ),
            });
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
        // Synthetic `<module>` shouldn't get recursive findings even if
        // it somehow ends up in an SCC — it's not a "function in a
        // cycle" the user can refactor.
        if is_synthetic_symbol(&node.name) {
            for c in node.children.iter_mut() {
                walk(c);
            }
            return;
        }
        if node.is_recursive && !has_kind(&node.findings, FindingKind::Recursive) {
            // Build the finding inline — we have all the info we need on
            // the node itself, no need to look up the Symbol again.
            node.findings.push(Finding {
                kind: FindingKind::Recursive,
                severity: Severity::Medium,
                // Recursion is rarely a quick fix: even confirming a
                // base case + bounded depth requires reading the code.
                effort: Effort::Medium,
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
    // Per-kind diversity gate — same reasoning as `collect_immediate_fixes`:
    // one rule pack with hundreds of high-severity findings must not push
    // every other kind out of the visible top-N. See `enforce_per_kind_cap`
    // for the algorithm.
    out = enforce_per_kind_cap(out, |f| f.kind, per_kind_quota(cap));
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

    // ── Per-kind diversity gate ─────────────────────────────────────
    //
    // The desktop app (drift-lab) caps at 24 visible findings; without
    // these tests, a single rule pack with hundreds of high-severity
    // findings (e.g. 315 migration_safety on a .sql-heavy repo) would
    // displace every code-level finding from the user's view. Pin the
    // diversity invariants so a future change can't regress silently.

    #[test]
    fn per_kind_quota_examples() {
        // Documented mapping: cap 24 → 6 per kind, cap 50 → 12 per kind,
        // cap 12 → 3 per kind (floor kicks in).
        assert_eq!(per_kind_quota(24), 6);
        assert_eq!(per_kind_quota(50), 12);
        assert_eq!(per_kind_quota(12), 3);
        assert_eq!(per_kind_quota(0), 3, "floor still applies at cap=0");
    }

    #[test]
    fn enforce_per_kind_cap_admits_up_to_quota_per_kind() {
        // 10 of kind A followed by 10 of kind B. Quota 3 → 3 As + 3 Bs.
        let items: Vec<(&str, usize)> = (0..10).map(|i| ("A", i)).chain((0..10).map(|i| ("B", i))).collect();
        let out = enforce_per_kind_cap(items, |it| it.0, 3);
        let by_kind: std::collections::HashMap<&str, usize> = out.iter().fold(
            std::collections::HashMap::new(),
            |mut m, it| { *m.entry(it.0).or_insert(0) += 1; m },
        );
        assert_eq!(by_kind.get("A").copied().unwrap_or(0), 3);
        assert_eq!(by_kind.get("B").copied().unwrap_or(0), 3);
        assert_eq!(out.len(), 6);
    }

    #[test]
    fn enforce_per_kind_cap_preserves_sort_order_within_admitted_set() {
        // Input is pre-sorted by some criterion; the gate must keep
        // FIRST-N of each kind (the highest-priority ones). 0..3 of A
        // and 0..3 of B should appear in their original positions.
        let items: Vec<(&str, usize)> = vec![
            ("A", 0), ("B", 0), ("A", 1), ("B", 1),
            ("A", 2), ("B", 2), ("A", 3), ("B", 3),
        ];
        let out = enforce_per_kind_cap(items, |it| it.0, 2);
        // Expect: A0, B0, A1, B1 — kind quota 2 means the third A
        // and third B are dropped.
        assert_eq!(out, vec![("A", 0), ("B", 0), ("A", 1), ("B", 1)]);
    }

    #[test]
    fn collect_immediate_fixes_diversifies_when_one_kind_dominates() {
        // Build a synthetic tree where one node has 100 high-severity
        // migration_safety findings (the db-mcp case) plus 5 medium-severity
        // missing_caching findings. Without the diversity gate, the top-24
        // would be all migration_safety. With the gate, missing_caching
        // must still appear.
        use crate::tree::CallTreeNode;
        use crate::graph::SymbolId;
        use crate::SymbolKind;
        use std::collections::BTreeMap;

        let mut findings = Vec::new();
        // 100 high-severity / trivial-effort migration_safety.
        for i in 0..100 {
            findings.push(Finding {
                kind: FindingKind::MigrationSafety,
                severity: Severity::High,
                effort: Effort::Trivial,
                confidence: 0.9,
                line: i + 1,
                message: format!("migration {i}"),
                evidence: vec![Evidence { call: "MIG_X".into(), line: i + 1, category: None }],
                remediation: None,
            });
        }
        // 5 medium-severity / small-effort missing_caching.
        for i in 0..5 {
            findings.push(Finding {
                kind: FindingKind::MissingCaching,
                severity: Severity::Medium,
                effort: Effort::Small,
                confidence: 0.7,
                line: 1000 + i,
                message: format!("cache {i}"),
                evidence: vec![],
                remediation: None,
            });
        }
        let node = CallTreeNode {
            id: SymbolId("synth::1".into()),
            name: "synth".into(),
            kind: SymbolKind::Function,
            file: "synth.rs".into(),
            line: 1,
            depth: 0,
            parent_class: None,
            children: vec![],
            truncated_reason: None,
            callers: vec![],
            callers_count: 0,
            callees_count: 0,
            subtree_size: 1,
            category_self: None,
            categories_reached: BTreeMap::new(),
            external_calls: vec![],
            complexity: 1,
            loc: 1,
            nesting_depth: 0,
            parameter_count: 0,
            is_async: false,
            call_site_count: 0,
            is_recursive: false,
            pagerank: 0.0,
            percent_total: 0.0,
            percent_parent: 0.0,
            n_plus_one_risk: false,
            blocking_in_async: false,
            findings,
            entry_labels: vec![],
        };
        let fixes = collect_immediate_fixes(&[node], 24);
        let by_kind: std::collections::HashMap<FindingKind, usize> = fixes
            .iter()
            .fold(std::collections::HashMap::new(), |mut m, f| {
                *m.entry(f.kind).or_insert(0) += 1;
                m
            });
        // Quota for cap=24 is 6 → at most 6 migration_safety in the top 24.
        assert!(
            by_kind.get(&FindingKind::MigrationSafety).copied().unwrap_or(0) <= 6,
            "migration_safety must be capped at 6 (cap/4) when 100 exist; got {by_kind:?}",
        );
        // All 5 missing_caching findings must survive (under quota of 6).
        assert_eq!(
            by_kind.get(&FindingKind::MissingCaching).copied().unwrap_or(0),
            5,
            "all 5 missing_caching findings must survive — diversity gate must not drop them; got {by_kind:?}",
        );
    }
}
