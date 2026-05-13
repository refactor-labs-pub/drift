use crate::categories::{classify, Category, ClassifyTier};
use crate::progress::{NullProgress, Progress};
use crate::{FileTags, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SymbolId(pub String);

impl SymbolId {
    pub fn for_symbol(s: &Symbol) -> Self {
        Self(format!(
            "{}::{}::{}",
            s.file.display(),
            s.parent.clone().unwrap_or_default(),
            s.name
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCall {
    pub name: String,
    pub receiver: Option<String>,
    pub category: Category,
    pub tier: ClassifyTier,
    pub evidence: String,
    pub line: usize,
    pub in_loop: bool,
    pub in_await: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraph {
    pub symbols: HashMap<SymbolId, Symbol>,
    pub by_name: HashMap<String, Vec<SymbolId>>,
    pub edges: HashMap<SymbolId, Vec<SymbolId>>,
    pub callers: HashMap<SymbolId, Vec<SymbolId>>,
    pub external_calls: HashMap<SymbolId, Vec<ExternalCall>>,
    // ── Phase B graph-derived metrics ──
    pub call_site_count: HashMap<SymbolId, usize>,
    pub is_recursive: HashMap<SymbolId, bool>,
    pub pagerank: HashMap<SymbolId, f64>,
}

impl CallGraph {
    /// Convenience: build with no progress reporting. Library
    /// consumers and existing tests stay on this path.
    pub fn build(all: &[FileTags]) -> Self {
        Self::build_with_progress(all, &NullProgress)
    }

    /// Build the call graph and surface per-file progress through
    /// `progress`. The body is identical to the silent `build` but
    /// the three large loops over `all` each emit `step_*` events so
    /// the CLI can render a percentage bar — without this, the user
    /// sees a single static "building call graph…" phase for the
    /// entire (potentially multi-minute) graph construction on a
    /// large monorepo.
    pub fn build_with_progress(all: &[FileTags], progress: &dyn Progress) -> Self {
        let total = all.len();
        let mut symbols: HashMap<SymbolId, Symbol> = HashMap::new();
        let mut by_name: HashMap<String, Vec<SymbolId>> = HashMap::new();
        let mut edges: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        let mut callers: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        let mut external_calls: HashMap<SymbolId, Vec<ExternalCall>> = HashMap::new();

        // ── Pass 1: index every defined symbol ──────────────────────────
        progress.step_start("indexing symbols", total);
        for (i, ft) in all.iter().enumerate() {
            // Surface the file path so the bar shows which file's
            // symbols are being indexed right now. Cheap stringify;
            // the sink only redraws every ~30Hz so this won't
            // bottleneck the loop.
            if i & 0x3F == 0 {
                progress.set_current(&ft.file.display().to_string());
            }
            for s in &ft.symbols {
                let id = SymbolId::for_symbol(s);
                by_name.entry(s.name.clone()).or_default().push(id.clone());
                symbols.insert(id.clone(), s.clone());
                edges.entry(id.clone()).or_default();
                callers.entry(id).or_default();
            }
            // Throttle: indicatif's own draw thread already debounces
            // redraws (~30fps), but skipping at the call-site keeps
            // the callback path itself cheap when per-file work is
            // tiny. 64 files per checkpoint matches the granularity
            // used in pass 2 and 3 below.
            if i & 0x3F == 0 {
                progress.step_progress(i, total);
            }
        }
        progress.step_progress(total, total);
        progress.step_end();

        // ── Pass 2: wire edges + external classifications ───────────────
        // For each reference R inside symbol X:
        //   - if R's name resolves to one or more defined symbols → add edges
        //   - if R's name doesn't resolve AND matches a category pattern →
        //     record as an external call on X.
        //
        // This is typically the slowest pass on large graphs because the
        // by_name lookup happens per-reference and the de-dup checks
        // (`bucket.contains(...)`) walk small vectors. We report progress
        // at file granularity (matches the natural outer-loop boundary)
        // rather than per-reference (would flood the callback).
        progress.step_start("wiring call edges", total);
        for (i, ft) in all.iter().enumerate() {
            if i & 0x3F == 0 {
                progress.set_current(&ft.file.display().to_string());
            }
            for r in &ft.references {
                let Some(in_name) = &r.in_symbol else { continue };
                let Some(src) = ft.symbols.iter().find(|s| {
                    &s.name == in_name
                        && s.byte_start <= r.byte_offset
                        && s.byte_end >= r.byte_offset
                }) else {
                    continue;
                };
                let src_id = SymbolId::for_symbol(src);

                let resolved: Vec<SymbolId> = by_name
                    .get(&r.name)
                    .map(|v| v.iter().filter(|t| *t != &src_id).cloned().collect())
                    .unwrap_or_default();

                if !resolved.is_empty() {
                    let bucket = edges.entry(src_id.clone()).or_default();
                    for t in &resolved {
                        if !bucket.contains(t) {
                            bucket.push(t.clone());
                            let entry = callers.entry(t.clone()).or_default();
                            if !entry.contains(&src_id) {
                                entry.push(src_id.clone());
                            }
                        }
                    }
                } else if let Some(c) = classify(&r.name, r.receiver.as_deref(), &ft.imports) {
                    // Either truly unresolved, or only resolved to self (filtered).
                    // Either way, an external classification still applies — this
                    // catches e.g. TypeORM `this.repo.save()` inside our own save().
                    // Phase D: tag the call site as in-loop / in-await using the
                    // source symbol's byte ranges collected during metrics walk.
                    let in_loop = src
                        .loop_ranges
                        .iter()
                        .any(|(s, e)| r.byte_offset >= *s && r.byte_offset <= *e);
                    let in_await = src
                        .await_ranges
                        .iter()
                        .any(|(s, e)| r.byte_offset >= *s && r.byte_offset <= *e);
                    let bucket = external_calls.entry(src_id.clone()).or_default();
                    if !bucket.iter().any(|e| e.name == r.name && e.line == r.line) {
                        bucket.push(ExternalCall {
                            name: r.name.clone(),
                            receiver: r.receiver.clone(),
                            category: c.category,
                            tier: c.tier,
                            evidence: c.evidence,
                            line: r.line,
                            in_loop,
                            in_await,
                        });
                    }
                }
            }
            if i & 0x3F == 0 {
                progress.step_progress(i, total);
            }
        }
        progress.step_progress(total, total);
        progress.step_end();

        // ── Phase B: graph-derived metrics ──

        // 1. call_site_count: total references resolving TO each symbol (not unique callers).
        //    Different from callers.len() (unique source symbols) — counts every callsite.
        progress.step_start("counting call sites", total);
        let mut call_site_count: HashMap<SymbolId, usize> =
            symbols.keys().map(|k| (k.clone(), 0usize)).collect();
        for (i, ft) in all.iter().enumerate() {
            for r in &ft.references {
                let Some(_) = r.in_symbol.as_ref() else { continue };
                if let Some(targets) = by_name.get(&r.name) {
                    for t in targets {
                        if let Some(c) = call_site_count.get_mut(t) {
                            *c += 1;
                        }
                    }
                }
            }
            if i & 0x3F == 0 {
                progress.step_progress(i, total);
            }
        }
        progress.step_progress(total, total);
        progress.step_end();

        // 2. Build a petgraph DiGraph for PageRank + SCC.
        //
        // These two passes (Tarjan SCC + 100-iteration PageRank) are
        // single atomic operations from our perspective — there's no
        // natural per-symbol checkpoint inside petgraph. We surface
        // them as `phase(...)` labels rather than step_* bars so the
        // user at least sees that "ranking…" is what's running, even
        // though we can't show a percentage.
        progress.phase("computing SCC + PageRank…");
        use petgraph::graph::DiGraph;
        let mut g: DiGraph<SymbolId, ()> = DiGraph::new();
        let mut idx_of: HashMap<SymbolId, petgraph::graph::NodeIndex> = HashMap::new();
        for id in symbols.keys() {
            let n = g.add_node(id.clone());
            idx_of.insert(id.clone(), n);
        }
        for (src, dsts) in &edges {
            let Some(&si) = idx_of.get(src) else { continue };
            for d in dsts {
                if let Some(&di) = idx_of.get(d) {
                    g.add_edge(si, di, ());
                }
            }
        }

        // 3. is_recursive: nodes in an SCC of size > 1.
        let mut is_recursive: HashMap<SymbolId, bool> =
            symbols.keys().map(|k| (k.clone(), false)).collect();
        for scc in petgraph::algo::tarjan_scc(&g) {
            if scc.len() > 1 {
                for ni in scc {
                    if let Some(id) = g.node_weight(ni) {
                        if let Some(b) = is_recursive.get_mut(id) {
                            *b = true;
                        }
                    }
                }
            }
        }

        // 4. PageRank, α=0.85, custom power-iteration with early-exit
        //    on convergence at tol=1e-6 (NetworkX-standard tolerance).
        //
        // Rationale for the swap from `petgraph::algo::page_rank`:
        //   - The hot-zone / log-amplification / severity-bump
        //     detectors all gate on `rank >= pagerank_p90`. Fixed-iter
        //     PageRank loses accuracy near the p90 boundary as iters
        //     drop — a 30-iter fixed run leaves ~7.6e-3 worst-case
        //     residual, big enough to flip borderline classifications.
        //   - Tolerance-based convergence pays only for the precision
        //     we need: typically 25–50 iters on real call graphs,
        //     capped at 100 to bound the worst case. The residual
        //     bound from tol=1e-6 is ~5.7e-6 (≈ tol·α/(1−α)) — three
        //     orders of magnitude tighter than fixed-30, and within
        //     striking distance of fixed-100's ≈1.7e-7.
        //   - Per-iter work is also lower: out-degrees are computed
        //     once, the receive-buffer is reused across iters, and
        //     the dangling-redistribution loop is skipped entirely
        //     when no dangling nodes exist.
        //
        // See `src/pagerank.rs` for the implementation + tests.
        let (ranks, pr_iters) = crate::pagerank::page_rank(
            &g,
            0.85_f64,
            crate::pagerank::DEFAULT_TOL,
            crate::pagerank::MAX_ITER,
        );
        // Surface the actual iteration count so users (and CI logs)
        // can spot graphs that hit the max-iter cap, which would
        // indicate either a pathological structure or a tolerance
        // that's too tight for the graph's spectral gap.
        progress.phase(&format!(
            "PageRank converged in {pr_iters}/{} iters",
            crate::pagerank::MAX_ITER,
        ));
        let mut pagerank: HashMap<SymbolId, f64> = HashMap::new();
        for (ni, rank) in ranks.iter().enumerate() {
            if let Some(id) = g.node_weight(petgraph::graph::NodeIndex::new(ni)) {
                pagerank.insert(id.clone(), *rank);
            }
        }

        Self {
            symbols,
            by_name,
            edges,
            callers,
            external_calls,
            call_site_count,
            is_recursive,
            pagerank,
        }
    }

    pub fn callers_of(&self, id: &SymbolId) -> &[SymbolId] {
        self.callers
            .get(id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn externals_of(&self, id: &SymbolId) -> &[ExternalCall] {
        self.external_calls
            .get(id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn find_entry_points(&self, query: &str) -> Vec<SymbolId> {
        // Heuristic: match by exact name first, then substring.
        let mut exact: Vec<SymbolId> = self
            .by_name
            .get(query)
            .cloned()
            .unwrap_or_default();
        if !exact.is_empty() {
            return exact;
        }
        for (name, ids) in &self.by_name {
            if name.contains(query) {
                exact.extend(ids.iter().cloned());
            }
        }
        exact
    }

    pub fn callees(&self, id: &SymbolId) -> &[SymbolId] {
        self.edges
            .get(id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

pub fn relative<'a>(root: &std::path::Path, path: &'a std::path::Path) -> &'a std::path::Path {
    path.strip_prefix(root).unwrap_or(path)
}

pub fn relative_buf(root: &std::path::Path, path: &std::path::Path) -> PathBuf {
    relative(root, path).to_path_buf()
}

#[allow(dead_code)]
pub fn all_symbol_ids(graph: &CallGraph) -> HashSet<SymbolId> {
    graph.symbols.keys().cloned().collect()
}
