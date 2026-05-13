use crate::categories::Category;
use crate::docker::EntryDecl;
use crate::graph::CallGraph;
use crate::insights::{self, FindingTopRef, ImmediateFix, RefactorCandidate, RootOverview};
use crate::linguist::{LanguageBreakdownEntry, LanguageStats};
use crate::progress::{NullProgress, Progress};
use crate::tree::CallTreeNode;
use crate::{FileTags, Language};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotPath {
    pub frames: Vec<String>,
    pub depth: usize,
    pub terminal_category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub languages: Vec<String>,
    pub files: usize,
    pub symbols: usize,
    pub edges: usize,
    pub categories: BTreeMap<String, usize>,
    pub top_callers: Vec<TopSymbol>,
    pub top_callees: Vec<TopSymbol>,
    pub hot_paths: Vec<HotPath>,
    // ── Phase B graph-derived rollups ──
    pub dead_code: Vec<TopSymbol>,
    pub pagerank_top: Vec<RankedByScore>,
    pub recursive_symbols: Vec<TopSymbol>,
    // ── Linguist-style language breakdown ──
    /// Per-programming-language byte share of the whole repo (filtered by
    /// the same .gitignore rules used for source discovery). Sorted desc by
    /// bytes. Mirrors GitHub's repo-page language bar.
    pub language_breakdown: Vec<LanguageBreakdownEntry>,
    /// The supported language drift actually profiled — i.e. the
    /// highest-byte language in `language_breakdown` that has a shipped
    /// tree-sitter parser. `None` when no supported language was detected.
    pub profiled_language: Option<String>,
    /// Share of total programming bytes accounted for by `profiled_language`.
    pub profiled_language_percent: Option<f64>,

    // ── Phase E: insights rollups ──
    /// Count of findings per kind across every node in every tree.
    /// Same shape as `categories`: kind-name → count.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub findings_by_kind: BTreeMap<String, usize>,
    /// Top-N findings as `(node_id, kind, severity, line)` triples, sorted
    /// by severity DESC. Same role as `pagerank_top`. The viewer resolves
    /// `node_id` via its existing `nodeIndex.byId` map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings_top: Vec<FindingTopRef>,
    /// Per-entry-point ("initial root") rollup: subtree share, categories
    /// reached, findings by severity, first callees, callers. Mirrors
    /// pprof's `top -cum` at root granularity. Sorted by subtree_size
    /// descending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots_overview: Vec<RootOverview>,
    /// "What can I fix RIGHT NOW?" — high-severity findings with
    /// trivial/small effort. Sorted by (severity DESC, effort ASC).
    /// Modeled on SonarQube's <5-min / <30-min remediation lanes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub immediate_fixes: Vec<ImmediateFix>,
    /// "Where do I need a full refactor?" — symbols with finding
    /// clusters, Large-effort findings, or god-function bodies.
    /// Aggregated per-node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refactor_candidates: Vec<RefactorCandidate>,
    /// Container-deployment entry points discovered from Dockerfile +
    /// docker-compose files at the project root. Each entry records the
    /// declared command (CMD / ENTRYPOINT / compose `command` /
    /// `entrypoint`) and, when we could resolve it, a back-link to the
    /// in-graph symbol it most likely launches. See `docker.rs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_declarations: Vec<EntryDecl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedByScore {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub parent_class: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopSymbol {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub parent_class: Option<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generator {
    pub tool: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    /// RFC 3339 / ISO 8601 UTC timestamp of when the report was assembled.
    /// Matches `Generator.captured_at` in `schema/profile.schema.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: String,
    pub mode: String,
    pub generator: Generator,
    pub summary: Summary,
    pub entries: Vec<CallTreeNode>,
}

impl Report {
    /// Convenience entry point with no progress reporting. Library
    /// consumers and the existing tests stay on this path; the CLI
    /// uses `build_with_progress` to surface what's happening during
    /// the (potentially multi-minute) post-tree-build assembly.
    pub fn build(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: Vec<CallTreeNode>,
        language_stats: &LanguageStats,
        source_root: Option<&Path>,
        entry_declarations: Vec<EntryDecl>,
    ) -> Self {
        Self::build_with_progress(
            all_tags,
            graph,
            entries,
            language_stats,
            source_root,
            entry_declarations,
            &NullProgress,
        )
    }

    /// Assemble the Report and surface per-pass progress through `progress`.
    ///
    /// Pre-`progress` this method ran 6 full-tree attach passes plus the
    /// 12+ sub-passes of `Summary::build` under a single static
    /// "assembling report…" label — on a 700-entry repo that meant
    /// minutes of apparent hang. Each attach pass is `for e in entries:
    /// walk(e)`, which is the textbook shape for a per-entry progress
    /// bar.
    ///
    /// We iterate `entries` ourselves and call each existing
    /// `attach_*` on a single-element `std::slice::from_mut(e)` slice.
    /// This avoids refactoring the insights API (the per-tree walk
    /// stays an implementation detail of each `attach_*`) while still
    /// giving a true per-entry bar in the CLI.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_progress(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: Vec<CallTreeNode>,
        language_stats: &LanguageStats,
        source_root: Option<&Path>,
        entry_declarations: Vec<EntryDecl>,
        progress: &dyn Progress,
    ) -> Self {
        // Phase E2: cross-tree finding passes that need graph-wide info.
        // - attach_recursive_findings: SCC membership lives on the graph,
        //   not on individual Symbols.
        // - attach_hot_zones: pagerank percentile is a graph-wide quantity.
        // Both run AFTER per-node detectors in `tree::build_inner` so they
        // can read those findings too.
        progress.phase("computing pagerank percentile…");
        let pagerank_p90 = insights::compute_pagerank_p90(graph.pagerank.values().copied());
        let mut entries = entries;
        let total = entries.len();

        // Each `for_each_entry` call below renders a `[bar] N/total` line
        // for one finding-attach pass. Doing it 6 times is the cost of
        // honest progress visibility — without these the user sees
        // "assembling report…" hang for minutes on a large repo.
        for_each_entry(
            &mut entries,
            "attaching recursive findings",
            progress,
            |e| insights::attach_recursive_findings(std::slice::from_mut(e)),
        );
        for_each_entry(
            &mut entries,
            "attaching missing-caching findings",
            progress,
            |e| insights::attach_missing_caching_findings(std::slice::from_mut(e)),
        );
        for_each_entry(
            &mut entries,
            "attaching log-amplification findings",
            progress,
            |e| {
                insights::attach_log_amplification_findings(
                    std::slice::from_mut(e),
                    pagerank_p90,
                )
            },
        );
        for_each_entry(
            &mut entries,
            "attaching hot-log findings",
            progress,
            |e| insights::attach_hot_log_findings(std::slice::from_mut(e), pagerank_p90),
        );
        for_each_entry(
            &mut entries,
            "attaching hot-zone findings",
            progress,
            |e| insights::attach_hot_zones(std::slice::from_mut(e), pagerank_p90),
        );
        // IMPORTANT: severity bumping must run LAST so it sees every
        // finding the prior passes produced. Without it, every finding
        // stays at its base severity regardless of where it sits in the
        // call graph — see pprof's red+thick = high-cum convention.
        for_each_entry(
            &mut entries,
            "bumping severities by impact",
            progress,
            |e| {
                insights::bump_severities_by_impact(
                    std::slice::from_mut(e),
                    pagerank_p90,
                )
            },
        );

        // ── Findings dedup pass ─────────────────────────────────────────
        //
        // Why this exists: each `attach_*` above walks every node of
        // every entry tree and pushes a fresh `Finding` clone. A symbol
        // like `Category.as_str` that's reached by 30 entry trees ends
        // up with 30 identical `MissingCaching` findings — one per
        // tree-node copy. On a real scan that produces ratios like
        // "13,765 findings on 397 symbols" (34× duplication), with the
        // top-N rows in the viewer showing the same fact dozens of
        // times.
        //
        // The fix is structural: a finding is a fact about a SYMBOL,
        // not a tree node. We do one final walk that keeps each
        // (SymbolId, FindingKind) on its FIRST occurrence and drops
        // it on subsequent appearances. `HashSet::insert` returns
        // true iff newly inserted — combined with `Vec::retain` this
        // gives O(N) dedup with no extra allocation per finding.
        //
        // Impact (measured on the self-scan of this very crate):
        //   - findings count: 13,765 → ~hundreds (≥25× reduction)
        //   - JSON size: shrinks by the same factor
        //   - `serializing JSON…` phase: 10–25× faster
        //   - memory peak in `to_string_pretty`: same factor lower
        //
        // The viewer's rollups (findings_by_kind, findings_top,
        // immediate_fixes, refactor_candidates) all benefit
        // automatically because they read from `node.findings`.
        progress.phase("deduplicating findings across trees…");
        dedupe_findings_across_trees(&mut entries);

        // Summary's own sub-passes get their own progress phases. See
        // `Summary::build_with_progress` for the breakdown.
        let _ = total; // silence unused on cfg(test) paths
        let summary = Summary::build_with_progress(
            all_tags,
            graph,
            &entries,
            language_stats,
            entry_declarations,
            progress,
        );
        Self {
            schema_version: "1.0".into(),
            mode: "static".into(),
            generator: Generator {
                tool: "drift-static-profiler".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                source_root: source_root.map(|p| p.display().to_string()),
                // RFC 3339 UTC, second precision — same shape the schema's
                // `format: date-time` advertises and what the viewer
                // parses via `new Date(...)`.
                captured_at: Some(
                    chrono::Utc::now()
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string(),
                ),
            },
            summary,
            entries,
        }
    }
}

/// Common loop body for the 6 attach passes inside `Report::build_with_progress`.
///
/// Centralizing the iteration here means each pass gets identical
/// progress semantics (step_start → per-entry update → step_end) and
/// the same call-site `set_current(entry.name)` UX. Extracting it
/// also keeps the body of `build_with_progress` readable — without
/// this helper that function would be 60+ lines of repetitive
/// `for (i, e) in entries.iter_mut().enumerate() { ... step_progress
/// ... }` blocks.
/// Walk every node in every entry tree and drop findings whose
/// `(symbol_id, kind)` pair has already been seen on an earlier
/// node. Keeps the FIRST occurrence of each unique fact and removes
/// every subsequent duplicate.
///
/// Why per-`SymbolId` and not per-tree-node: a finding describes a
/// fact about a symbol (e.g. "this method is in a recursion cycle").
/// The same symbol can appear as a `CallTreeNode` inside many entry
/// trees because the analyzer expands transitive callees. Each
/// `attach_*` pass pushes the fact on every appearance, producing
/// the runaway duplication the user observed (13,765 findings on
/// 397 symbols).
///
/// Why the FIRST occurrence wins: we want to keep at least one
/// node carrying the finding (so the viewer can navigate to it),
/// but it doesn't matter which one — every appearance points at
/// the same underlying symbol via `node.id`. The traversal order
/// is stable (entries in their input order, depth-first), so the
/// chosen representative is deterministic across runs.
///
/// Complexity: O(total_tree_nodes × avg_findings_per_node) with
/// constant-time `HashSet::insert`. Memory: one tuple per unique
/// (symbol, kind) — bounded by `unique_symbols × FindingKind::N`.
fn dedupe_findings_across_trees(entries: &mut [CallTreeNode]) {
    use crate::graph::SymbolId;
    use crate::insights::FindingKind;
    use std::collections::HashSet;

    fn walk(
        node: &mut CallTreeNode,
        seen: &mut HashSet<(SymbolId, FindingKind)>,
    ) {
        // `HashSet::insert` returns true iff the value was newly
        // inserted, false if already present. `retain` keeps the
        // first occurrence (insert returns true) and drops every
        // subsequent duplicate (insert returns false). No extra
        // allocation per finding — we just clone the SymbolId for
        // the hash key, which is cheap.
        node.findings
            .retain(|f| seen.insert((node.id.clone(), f.kind)));
        for c in node.children.iter_mut() {
            walk(c, seen);
        }
    }

    let mut seen: HashSet<(SymbolId, FindingKind)> = HashSet::new();
    for e in entries.iter_mut() {
        walk(e, &mut seen);
    }
}

fn for_each_entry(
    entries: &mut [CallTreeNode],
    label: &str,
    progress: &dyn Progress,
    mut body: impl FnMut(&mut CallTreeNode),
) {
    let total = entries.len();
    progress.step_start(label, total);
    for (i, e) in entries.iter_mut().enumerate() {
        // Same `set_current` UX as the tree-build phase: the user
        // sees which entry's tree is currently being processed.
        if i & 0x0F == 0 {
            progress.set_current(&e.name);
            progress.step_progress(i, total);
        }
        body(e);
    }
    progress.step_progress(total, total);
    progress.step_end();
}

impl Summary {
    /// Backward-compatible silent path. Library/test callers stay
    /// here; the CLI's `Report::build_with_progress` always routes
    /// through `build_with_progress` instead.
    pub fn build(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: &[CallTreeNode],
        language_stats: &LanguageStats,
        entry_declarations: Vec<EntryDecl>,
    ) -> Self {
        Self::build_with_progress(
            all_tags,
            graph,
            entries,
            language_stats,
            entry_declarations,
            &NullProgress,
        )
    }

    /// Like `build` but emits `phase()` / `step_*` events for each
    /// sub-pass. The slowest pass on large repos (`collect_hot_paths`
    /// — recursive walk over every node of every tree) gets a real
    /// per-entry bar; the rest get spinner labels so the user can
    /// at least see what's running. Behavior is bit-identical to
    /// `build`; only the timing of stderr writes differs.
    pub fn build_with_progress(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: &[CallTreeNode],
        language_stats: &LanguageStats,
        entry_declarations: Vec<EntryDecl>,
        progress: &dyn Progress,
    ) -> Self {
        progress.phase("collecting languages…");
        let languages: Vec<String> = {
            let mut s: HashSet<&'static str> = HashSet::new();
            for ft in all_tags {
                s.insert(match ft.language {
                    Language::Python => "python",
                    Language::Java => "java",
                    Language::TypeScript => "typescript",
                    Language::JavaScript => "javascript",
                    Language::Go => "go",
                    Language::Rust => "rust",
                    Language::Scala => "scala",
                    Language::Kotlin => "kotlin",
                });
            }
            let mut v: Vec<String> = s.into_iter().map(|x| x.to_string()).collect();
            v.sort();
            v
        };

        // Categories aggregate across all entries
        progress.phase("aggregating categories…");
        let mut categories: BTreeMap<String, usize> = BTreeMap::new();
        for e in entries {
            for (k, v) in &e.categories_reached {
                *categories.entry(k.clone()).or_default() += *v;
            }
        }
        // Ensure every category is represented (zero-valued) for stable UI
        for c in Category::ALL {
            categories.entry(c.as_str().to_string()).or_insert(0);
        }

        // Top callers (most-called symbols across the project)
        progress.phase("ranking top callers / callees…");
        let mut callers_rank: Vec<(String, &crate::graph::SymbolId, usize)> = graph
            .callers
            .iter()
            .filter_map(|(id, list)| {
                let sym = graph.symbols.get(id)?;
                Some((sym.name.clone(), id, list.len()))
            })
            .collect();
        callers_rank.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.cmp(&b.0))
                .then_with(|| a.1.0.cmp(&b.1.0))
        });
        let top_callers: Vec<TopSymbol> = callers_rank
            .into_iter()
            .filter(|(_, _, c)| *c > 0)
            .take(10)
            .filter_map(|(name, id, count)| {
                let sym = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name,
                    file: sym.file.display().to_string(),
                    line: sym.line,
                    parent_class: sym.parent.clone(),
                    count,
                })
            })
            .collect();

        // Top callees (symbols with the most fan-out)
        let mut callees_rank: Vec<(String, &crate::graph::SymbolId, usize)> = graph
            .edges
            .iter()
            .filter_map(|(id, list)| {
                let sym = graph.symbols.get(id)?;
                Some((sym.name.clone(), id, list.len()))
            })
            .collect();
        callees_rank.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.cmp(&b.0))
                .then_with(|| a.1.0.cmp(&b.1.0))
        });
        let top_callees: Vec<TopSymbol> = callees_rank
            .into_iter()
            .filter(|(_, _, c)| *c > 0)
            .take(10)
            .filter_map(|(name, id, count)| {
                let sym = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name,
                    file: sym.file.display().to_string(),
                    line: sym.line,
                    parent_class: sym.parent.clone(),
                    count,
                })
            })
            .collect();

        // Hot paths: walk each entry, collect chains ending at nodes with a
        // category_self or external_calls, keep the longest few.
        //
        // This is the slowest pass in `Summary::build` on large repos —
        // a recursive walk over every node of every tree, with chain
        // construction at every category-bearing leaf. We surface a
        // real per-entry step bar so the user sees the progress
        // moving instead of an apparent "assembling report" hang.
        let hp_total = entries.len();
        progress.step_start("collecting hot paths", hp_total);
        let mut hot_paths: Vec<HotPath> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if i & 0x0F == 0 {
                progress.set_current(&e.name);
                progress.step_progress(i, hp_total);
            }
            collect_hot_paths(e, &mut Vec::new(), &mut hot_paths);
        }
        progress.step_progress(hp_total, hp_total);
        progress.step_end();
        progress.phase("ranking hot paths…");
        hot_paths.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then_with(|| a.terminal_category.cmp(&b.terminal_category))
                .then_with(|| a.frames.cmp(&b.frames))
        });
        hot_paths.truncate(10);

        let edges_count: usize = graph.edges.values().map(|v| v.len()).sum();

        // ── Phase B rollups ──

        // Entry-point IDs (the user-pinned roots) — these are NOT dead even
        // if no caller exists in source (HTTP handlers, main, etc.)
        let entry_ids: std::collections::HashSet<&crate::graph::SymbolId> =
            entries.iter().map(|e| &e.id).collect();

        progress.phase("computing dead code…");
        let mut dead_code: Vec<TopSymbol> = graph
            .callers
            .iter()
            .filter(|(id, callers)| callers.is_empty() && !entry_ids.contains(id))
            .filter_map(|(id, _)| {
                let s = graph.symbols.get(id)?;
                // Classes are often "instantiated" rather than called by name; skip
                // them in dead-code reporting to reduce noise.
                if matches!(s.kind, crate::SymbolKind::Class) {
                    return None;
                }
                Some(TopSymbol {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    count: 0,
                })
            })
            .collect();
        dead_code.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        progress.phase("ranking pagerank top…");
        let mut pagerank_pairs: Vec<(&crate::graph::SymbolId, f64)> =
            graph.pagerank.iter().map(|(id, r)| (id, *r)).collect();
        pagerank_pairs.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.0.cmp(&b.0.0))
        });
        let pagerank_top: Vec<RankedByScore> = pagerank_pairs
            .into_iter()
            .take(10)
            .filter_map(|(id, score)| {
                let s = graph.symbols.get(id)?;
                Some(RankedByScore {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    score,
                })
            })
            .collect();

        progress.phase("listing recursive symbols…");
        let mut recursive_symbols: Vec<TopSymbol> = graph
            .is_recursive
            .iter()
            .filter(|(_, rec)| **rec)
            .filter_map(|(id, _)| {
                let s = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    count: 1,
                })
            })
            .collect();
        recursive_symbols.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        // Phase E rollups — derived from `findings` already attached to
        // each node by the per-node detectors and the post-build pass.
        //
        // Each of these collects walks every node of every entry tree.
        // On a 700-entry repo they're individually fast (under a
        // second) but stacked they add up — and previously appeared
        // as part of the silent "assembling report" black box. We
        // surface them as a counted step bar (5 sub-passes / 5)
        // updating after each, so the user sees real progress.
        progress.step_start("collecting findings rollups", 5);
        progress.set_current("findings_by_kind");
        let findings_by_kind = insights::collect_findings_by_kind(entries);
        progress.step_progress(1, 5);
        progress.set_current("findings_top");
        let findings_top = insights::collect_findings_top(entries, 50);
        progress.step_progress(2, 5);
        progress.set_current("roots_overview");
        let roots_overview = insights::collect_roots_overview(entries);
        progress.step_progress(3, 5);
        progress.set_current("immediate_fixes");
        let immediate_fixes = insights::collect_immediate_fixes(entries, 50);
        progress.step_progress(4, 5);
        progress.set_current("refactor_candidates");
        let refactor_candidates = insights::collect_refactor_candidates(entries, 30);
        progress.step_progress(5, 5);
        progress.step_end();

        Self {
            languages,
            files: all_tags.len(),
            symbols: graph.symbols.len(),
            edges: edges_count,
            categories,
            top_callers,
            top_callees,
            hot_paths,
            dead_code,
            pagerank_top,
            recursive_symbols,
            language_breakdown: language_stats.breakdown.clone(),
            profiled_language: language_stats.dominant_supported_name.clone(),
            profiled_language_percent: language_stats.dominant_supported_percent,
            findings_by_kind,
            findings_top,
            roots_overview,
            immediate_fixes,
            refactor_candidates,
            entry_declarations,
        }
    }
}

fn collect_hot_paths(
    node: &CallTreeNode,
    stack: &mut Vec<String>,
    out: &mut Vec<HotPath>,
) {
    let label = format!(
        "{}{}",
        node.parent_class
            .as_ref()
            .map(|p| format!("{p}."))
            .unwrap_or_default(),
        node.name
    );
    stack.push(label);

    let terminal_cat = node.category_self.map(|c| c.as_str().to_string()).or_else(|| {
        node.external_calls
            .first()
            .map(|e| e.category.as_str().to_string())
    });

    if let Some(cat) = terminal_cat {
        out.push(HotPath {
            frames: stack.clone(),
            depth: node.depth,
            terminal_category: cat,
        });
    }

    for c in &node.children {
        collect_hot_paths(c, stack, out);
    }
    stack.pop();
}

