use crate::categories::{classify, Category, ClassifyTier};
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
    pub fn build(all: &[FileTags]) -> Self {
        let mut symbols: HashMap<SymbolId, Symbol> = HashMap::new();
        let mut by_name: HashMap<String, Vec<SymbolId>> = HashMap::new();
        let mut edges: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        let mut callers: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        let mut external_calls: HashMap<SymbolId, Vec<ExternalCall>> = HashMap::new();

        for ft in all {
            for s in &ft.symbols {
                let id = SymbolId::for_symbol(s);
                by_name.entry(s.name.clone()).or_default().push(id.clone());
                symbols.insert(id.clone(), s.clone());
                edges.entry(id.clone()).or_default();
                callers.entry(id).or_default();
            }
        }

        // Wire edges + external classifications.
        // For each reference R inside symbol X:
        //   - if R's name resolves to one or more defined symbols → add edges
        //   - if R's name doesn't resolve AND matches a category pattern →
        //     record as an external call on X.
        for ft in all {
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
        }

        // ── Phase B: graph-derived metrics ──

        // 1. call_site_count: total references resolving TO each symbol (not unique callers).
        //    Different from callers.len() (unique source symbols) — counts every callsite.
        let mut call_site_count: HashMap<SymbolId, usize> =
            symbols.keys().map(|k| (k.clone(), 0usize)).collect();
        for ft in all {
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
        }

        // 2. Build a petgraph DiGraph for PageRank + SCC.
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

        // 4. PageRank, α=0.85, 100 iters (Brin & Page canonical).
        let ranks: Vec<f64> = petgraph::algo::page_rank(&g, 0.85_f64, 100);
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
