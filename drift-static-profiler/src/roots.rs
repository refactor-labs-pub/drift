//! Root-entry-point discovery for the static profiler.
//!
//! This is the static-analysis equivalent of Chrome DevTools' "Top-Down" view
//! (which the docs explicitly call "root activities"), pprof's `top -cum`,
//! Speedscope's Sandwich view, or IntelliJ's Method List: instead of asking
//! the user "which symbol is the entry point?", we surface every plausible
//! entry point ranked by reach so the user can pick one — or scan them all.
//!
//! "Plausible entry point" is heuristic by nature. We use:
//!   - in-degree == 0 in the project call graph (no in-project caller).
//!     Captures route handlers registered via decorator/metadata (e.g.
//!     FastAPI `@router.post`, NestJS `@Post()`, Spring `@RequestMapping`)
//!     since those registrations are data, not calls, and so don't add a
//!     caller edge.
//!   - not a class definition (classes are containers, not entries).
//!   - not a trivial accessor (`getX`, `setX`, `isX`) — same filter as the
//!     tree builder so the roots list matches what's actually navigable.
//!   - reach >= `min_reach` (subtree size in the static graph). This is
//!     the static analog of pprof's "cumulative samples" — it excludes
//!     leaf utility functions that happen to be unused inside the project
//!     but provide no interesting tree to explore.
//!   - optionally skip test files and language-conventional private symbols.
//!
//! Trade-off acknowledged: in-degree == 0 also matches *dead code* (truly
//! unreferenced helpers). We don't try to distinguish — every CallGraph
//! root is worth surfacing, and reach-based ranking pushes dead leaves to
//! the bottom. Listing dead-but-callable entry points is itself useful for
//! exploring an unfamiliar codebase.

use crate::{
    graph::{CallGraph, SymbolId},
    SymbolKind,
};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DiscoverOpts {
    /// Minimum reach (transitive callee count, deduped) required to be
    /// considered a "root worth profiling". 1 keeps single-symbol roots;
    /// the default of 2 drops leaves with no in-project callees.
    pub min_reach: usize,
    /// Skip symbols whose file path contains a `tests`, `test`, `__tests__`,
    /// or `spec` segment. Defaults to true — test entry points clutter the
    /// list for production-code exploration.
    pub skip_tests: bool,
    /// Skip language-conventional private symbols: Python `_foo` /
    /// `__foo`, TS/JS leading `_`. Defaults to true.
    pub skip_private: bool,
    /// Drop accessors (`getX`, `setX`, `isX`). Defaults to true — same
    /// filter as the tree builder's `--no-accessors` so the roots list
    /// matches what users see when they drill in.
    pub skip_accessors: bool,
    /// Hard cap on number of discovered roots. Prevents the viewer from
    /// being asked to render a tree for thousands of symbols in a large
    /// monorepo. Defaults to 200 — generous but bounded.
    pub max_roots: usize,
}

impl Default for DiscoverOpts {
    fn default() -> Self {
        Self {
            min_reach: 2,
            skip_tests: true,
            skip_private: true,
            skip_accessors: true,
            max_roots: 200,
        }
    }
}

/// A discovered root, with the in-graph reach used to rank it. The reach is
/// computed as the size of the deduped set of transitively reachable symbols
/// (cycles are visited once). This mirrors how flame-graph tools compute
/// "cumulative samples" — what fraction of the program is "underneath" this
/// frame — but with edge-count rather than sample-count weight.
#[derive(Debug, Clone)]
pub struct DiscoveredRoot {
    pub id: SymbolId,
    pub name: String,
    pub reach: usize,
}

pub fn discover_roots(
    graph: &CallGraph,
    root_dir: &Path,
    opts: &DiscoverOpts,
) -> Vec<DiscoveredRoot> {
    let mut out: Vec<DiscoveredRoot> = graph
        .symbols
        .iter()
        .filter(|(id, sym)| {
            // Class is a container, not an executable entry.
            if matches!(sym.kind, SymbolKind::Class) {
                return false;
            }
            // In-degree == 0 in the project graph.
            let callers = graph.callers.get(*id).map(|v| v.len()).unwrap_or(0);
            if callers != 0 {
                return false;
            }
            if opts.skip_accessors && is_accessor(&sym.name) {
                return false;
            }
            if opts.skip_private && is_private(&sym.name) {
                return false;
            }
            if opts.skip_tests && in_test_path(&sym.file, root_dir) {
                return false;
            }
            true
        })
        .map(|(id, sym)| {
            let reach = reachable_count(graph, id);
            DiscoveredRoot {
                id: id.clone(),
                name: sym.name.clone(),
                reach,
            }
        })
        .filter(|r| r.reach >= opts.min_reach)
        .collect();

    // Rank: biggest reach first; tie-break by name for stable output.
    out.sort_by(|a, b| b.reach.cmp(&a.reach).then_with(|| a.name.cmp(&b.name)));
    out.truncate(opts.max_roots);
    out
}

fn reachable_count(graph: &CallGraph, start: &SymbolId) -> usize {
    let mut seen: HashSet<&SymbolId> = HashSet::new();
    let mut stack: Vec<&SymbolId> = vec![start];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(out) = graph.edges.get(id) {
            for c in out {
                stack.push(c);
            }
        }
    }
    seen.len()
}

fn is_accessor(name: &str) -> bool {
    if name.len() < 4 {
        return false;
    }
    let suffix = if let Some(s) = name.strip_prefix("get").or_else(|| name.strip_prefix("set")) {
        s
    } else if let Some(s) = name.strip_prefix("is") {
        s
    } else {
        return false;
    };
    suffix
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
}

fn is_private(name: &str) -> bool {
    // Python conventional private, TS/JS `_foo`. We don't try to honor TS
    // `private` keyword because that needs AST context — convention is
    // good enough for filtering noise out of a roots list.
    name.starts_with('_')
}

/// True when `path` lives in a test/spec subdirectory *relative to the
/// project root*. We strip the project-root prefix first so that a project
/// rooted at, say, `tests/fixtures/foo/` is not itself misidentified as
/// "test code" — only test directories *inside* the analyzed root count.
fn in_test_path(path: &Path, root_dir: &Path) -> bool {
    let rel = path.strip_prefix(root_dir).unwrap_or(path);
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(s.as_str(), "tests" | "test" | "__tests__" | "spec" | "specs")
    })
}
