//! Public library entry point: analyze a project root and build a Report.
//!
//! Both the CLI (`main.rs`) and external library consumers should use
//! [`analyze`] (for explicit entries) or [`analyze_roots`] (for
//! auto-discovered entry points) to produce a [`Report`]. Keeping the
//! orchestration here ensures the `drift-static-profiler` binary and any
//! embedded usage stay behaviorally identical.

use anyhow::Result;
use std::path::Path;

use crate::{
    graph::CallGraph,
    linguist::{compute_language_stats, LanguageStats},
    report::Report,
    roots::{discover_roots, DiscoverOpts, DiscoveredRoot},
    tags::extract_tags,
    tree::{CallTreeNode, TreeBuilder},
    walker::{discover_source_files_with, WalkOpts},
    FileTags, Language,
};

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    pub max_depth: usize,
    pub skip_accessors: bool,
    /// Walker-level filter: skip test/spec/mock files entirely so they
    /// don't appear as roots, callees, dead_code, or anywhere in the
    /// graph. Off by default — the historical scan walks tests.
    /// Forwarded to `WalkOpts.exclude_tests` in `build_graph_context`.
    pub exclude_tests: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            max_depth: 12,
            skip_accessors: false,
            exclude_tests: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalyzeOutcome {
    pub report: Report,
    /// Entry-point names from `entries` that didn't match any symbol.
    pub unresolved_entries: Vec<String>,
    /// Linguist-style language breakdown of the project tree, plus which
    /// supported language was picked for profiling.
    pub language_stats: LanguageStats,
    /// The language that drift actually profiled (i.e. the dominant supported
    /// language). `None` when the project contained no source files for any
    /// of the four shipped tree-sitter parsers.
    pub profiled_language: Option<Language>,
    /// Auto-discovered roots — populated only by [`analyze_roots`]. Empty for
    /// the explicit-entry [`analyze`] path. Listed here on the outcome (not
    /// the Report) so it doesn't leak into the JSON schema unless explicitly
    /// surfaced by the caller.
    pub discovered_roots: Vec<DiscoveredRoot>,
}

/// Shared first phase: walk → linguist breakdown → filter → extract → graph.
/// Both [`analyze`] and [`analyze_roots`] start here so the graph build is
/// only paid for once per call, and the language picking logic stays in one
/// place.
struct GraphContext {
    all_tags: Vec<FileTags>,
    graph: CallGraph,
    language_stats: LanguageStats,
    profiled_language: Option<Language>,
}

fn build_graph_context(root: &Path, opts: &AnalyzeOptions) -> GraphContext {
    // 1. Compute a GitHub-Linguist-style language breakdown of the whole
    //    tree. This honors the same .gitignore / .driftignore / default-skip
    //    rules as source discovery, so build output and vendored deps don't
    //    skew the percentages.
    //
    // Note: the language breakdown intentionally walks WITHOUT the
    // exclude_tests filter so the language %s reflect the WHOLE repo
    // (e.g. "this project is 78% TypeScript") — independent of which
    // subset of files we then analyze. Excluding tests there would
    // produce surprising percentages.
    let language_stats = compute_language_stats(root);
    let profiled_language = language_stats.dominant_supported;

    // 2. Walk for source files (still all seven supported languages),
    //    then filter down to the dominant supported language. The
    //    exclude_tests flag IS applied here so the graph itself doesn't
    //    see test files when the caller asked for them dropped.
    let walk_opts = WalkOpts {
        exclude_tests: opts.exclude_tests,
        ..WalkOpts::default()
    };
    let all_files = discover_source_files_with(root, &walk_opts);
    let files: Vec<_> = match profiled_language {
        Some(lang) => all_files.into_iter().filter(|(_, l)| *l == lang).collect(),
        None => Vec::new(),
    };

    let mut all_tags = Vec::with_capacity(files.len());
    for (file, lang) in files {
        match extract_tags(&file, lang) {
            Ok(tags) => all_tags.push(tags),
            Err(e) => eprintln!("warn: failed to parse {}: {e:#}", file.display()),
        }
    }
    let graph = CallGraph::build(&all_tags);
    GraphContext {
        all_tags,
        graph,
        language_stats,
        profiled_language,
    }
}

fn build_trees_from_ids(
    ctx: &GraphContext,
    root: &Path,
    ids: &[crate::graph::SymbolId],
    opts: &AnalyzeOptions,
) -> Vec<CallTreeNode> {
    let mut builder = TreeBuilder::new(&ctx.graph, root);
    builder.max_depth = opts.max_depth;
    builder.skip_accessors = opts.skip_accessors;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(node) = builder.build(id) {
            out.push(node);
        }
    }
    out
}

pub fn analyze(root: &Path, entries: &[String], opts: &AnalyzeOptions) -> Result<AnalyzeOutcome> {
    let ctx = build_graph_context(root, opts);

    let mut entry_ids = Vec::new();
    let mut unresolved = Vec::new();
    for q in entries {
        let ids = ctx.graph.find_entry_points(q);
        if ids.is_empty() {
            unresolved.push(q.clone());
            continue;
        }
        entry_ids.extend(ids);
    }
    let roots = build_trees_from_ids(&ctx, root, &entry_ids, opts);
    let report = Report::build(&ctx.all_tags, &ctx.graph, roots, &ctx.language_stats, Some(root));
    Ok(AnalyzeOutcome {
        report,
        unresolved_entries: unresolved,
        language_stats: ctx.language_stats,
        profiled_language: ctx.profiled_language,
        discovered_roots: Vec::new(),
    })
}

/// Auto-discover every plausible root entry point in the project (symbols
/// with no in-graph caller, ranked by transitive reach) and analyze each
/// one. The returned [`AnalyzeOutcome.report.entries`] contains one
/// [`CallTreeNode`] per discovered root, ordered the same as
/// [`AnalyzeOutcome.discovered_roots`] (biggest reach first).
///
/// This is the static-analysis equivalent of Chrome DevTools' "Top-Down"
/// view, pprof's `top -cum`, or Speedscope's Sandwich view: a project-wide
/// roots overview from which the user drills into a specific call tree.
pub fn analyze_roots(
    root: &Path,
    discover: &DiscoverOpts,
    opts: &AnalyzeOptions,
) -> Result<AnalyzeOutcome> {
    let ctx = build_graph_context(root, opts);
    let discovered = discover_roots(&ctx.graph, root, discover);
    let ids: Vec<_> = discovered.iter().map(|r| r.id.clone()).collect();
    let roots = build_trees_from_ids(&ctx, root, &ids, opts);
    let report = Report::build(&ctx.all_tags, &ctx.graph, roots, &ctx.language_stats, Some(root));
    Ok(AnalyzeOutcome {
        report,
        unresolved_entries: Vec::new(),
        language_stats: ctx.language_stats,
        profiled_language: ctx.profiled_language,
        discovered_roots: discovered,
    })
}
