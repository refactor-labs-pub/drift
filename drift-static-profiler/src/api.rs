//! Public library entry point: analyze a project root and build a Report.
//!
//! Both the CLI (`main.rs`) and external library consumers should use
//! [`analyze`] (for explicit entries) or [`analyze_roots`] (for
//! auto-discovered entry points) to produce a [`Report`]. Keeping the
//! orchestration here ensures the `drift-static-profiler` binary and any
//! embedded usage stay behaviorally identical.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::{
    docker::{self, EntryDecl},
    graph::CallGraph,
    linguist::{compute_language_stats_from_entries, LanguageStats},
    progress::{NullProgress, Progress},
    report::Report,
    roots::{DiscoverOpts, DiscoveredRoot},
    tags::extract_tags_for_files,
    tree::{CallTreeNode, TreeBuilder},
    walker::{walk_files_classified_with, WalkOpts},
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
    entry_declarations: Vec<EntryDecl>,
}

fn build_graph_context(
    root: &Path,
    opts: &AnalyzeOptions,
    progress: &dyn Progress,
) -> GraphContext {
    // ── 1. ONE filesystem walk, classified up-front ──────────────────────
    //
    // The legacy implementation walked the repo twice: once for the
    // linguist byte-counting and once for source-file discovery. On a
    // large monorepo that doubled inode reads and gitignore evaluation.
    //
    // We now walk once, classifying each file as it's discovered, then
    // feed the same vector to (a) the linguist breakdown and (b) the
    // source-file filter below. `ClassifiedFile` carries both the
    // language NAME (for the bar) and the supported `Language` enum
    // (for the parse filter).
    //
    // The breakdown intentionally sees the WHOLE walk (not test-
    // filtered), so a repo that's 78% TypeScript still reads as 78%
    // TypeScript when the user passes `--no-tests`. The parse-filter
    // step below applies `exclude_tests`.
    let walk_opts = WalkOpts {
        exclude_tests: opts.exclude_tests,
        ..WalkOpts::default()
    };
    let walked = walk_files_classified_with(root, &walk_opts, progress);
    let language_stats = compute_language_stats_from_entries(&walked);
    let profiled_language = language_stats.dominant_supported;

    // ── 2. Filter to source files in the profiled language ──────────────
    //
    // Single pass through `walked` keeping only entries whose
    // `Language` matches the dominant pick. The `walked` vector is
    // consumed here — we don't need it again after this step, so we
    // drop it to release the path/size memory before the parse loop's
    // peak (which holds source strings for parallelism × largest_file
    // bytes simultaneously).
    let files: Vec<(PathBuf, Language)> = match profiled_language {
        Some(target) => walked
            .into_iter()
            .filter_map(|f| match f.language {
                Some(lang) if lang == target => Some((f.path, lang)),
                _ => None,
            })
            .collect(),
        None => {
            drop(walked);
            Vec::new()
        }
    };

    // ── 3. Parse all source files in parallel ────────────────────────────
    //
    // `extract_tags_for_files` uses rayon's worker pool with a
    // thread-local Parser+Query cache (see `tags.rs` for the cache
    // design). Net effect on a large repo:
    //   - Query compilation is paid once per (thread, language)
    //     instead of once per file.
    //   - Tree-sitter parsing scales with available cores.
    //   - Errors are logged and the file is skipped — one corrupt
    //     source file does NOT fail the whole scan.
    let all_tags = extract_tags_for_files(&files, progress);

    // ── 4. Build the symbol graph and discover entry declarations ────────
    //
    // `build_with_progress` emits one `step_*` bar per internal pass
    // (indexing → wiring edges → counting call sites → PageRank). On
    // a large monorepo the wiring pass alone can take tens of
    // seconds; without per-pass progress the user sees the single
    // "building call graph…" phase apparently hang.
    let graph = CallGraph::build_with_progress(&all_tags, progress);

    // Walk container-deployment files (Dockerfile + docker-compose)
    // AND per-language manifests (package.json, pyproject.toml,
    // Cargo.toml, deno.json). Both families produce `EntryDecl` values
    // so the matcher can wire them to in-graph symbols uniformly.
    //
    // Independent of profiled_language — a Java service can still
    // have its Dockerfile read, and a polyglot monorepo may have
    // manifests for several languages.
    progress.phase("collecting entry declarations…");
    let mut entry_declarations = docker::collect(root, &all_tags, &graph);
    let mut manifest_entries = crate::manifest::collect(root);
    crate::docker::match_entries(&mut manifest_entries, &all_tags, &graph);
    entry_declarations.extend(manifest_entries);
    GraphContext {
        all_tags,
        graph,
        language_stats,
        profiled_language,
        entry_declarations,
    }
}

fn build_trees_from_ids(
    ctx: &GraphContext,
    root: &Path,
    ids: &[crate::graph::SymbolId],
    opts: &AnalyzeOptions,
    progress: &dyn Progress,
) -> Vec<CallTreeNode> {
    let mut builder = TreeBuilder::new(&ctx.graph, root);
    builder.max_depth = opts.max_depth;
    builder.skip_accessors = opts.skip_accessors;
    let total = ids.len();
    progress.step_start("building call trees", total);
    let mut out = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        // "Current item" indicator — same role as the file-path
        // display during parse. Surfacing the entry symbol name
        // turns "building call trees: 12/179" into something the
        // user can debug: which entry is slow? (Often the answer
        // is a god-function with thousands of transitive callees.)
        if let Some(sym) = ctx.graph.symbols.get(id) {
            progress.set_current(&sym.name);
        }
        if let Some(node) = builder.build(id) {
            out.push(node);
        }
        // Trees can be expensive individually on highly-connected
        // entry points, so we update every 16 trees (not 64 like the
        // graph passes) to keep the bar lively even with few entries.
        if i & 0x0F == 0 {
            progress.step_progress(i, total);
        }
    }
    progress.step_progress(total, total);
    progress.step_end();
    out
}

pub fn analyze(root: &Path, entries: &[String], opts: &AnalyzeOptions) -> Result<AnalyzeOutcome> {
    analyze_with_progress(root, entries, opts, &NullProgress)
}

/// Same as [`analyze`] but lets the caller observe pipeline progress.
///
/// The CLI's `scan` / `analyze-root` commands pass a `CliProgress`
/// sink so terminal users see a live progress bar; library consumers
/// can plug in their own implementation, or stay on the silent
/// [`analyze`] entry point.
pub fn analyze_with_progress(
    root: &Path,
    entries: &[String],
    opts: &AnalyzeOptions,
    progress: &dyn Progress,
) -> Result<AnalyzeOutcome> {
    let ctx = build_graph_context(root, opts, progress);

    progress.phase("resolving entry points…");
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
    let mut roots = build_trees_from_ids(&ctx, root, &entry_ids, opts, progress);
    docker::label_call_tree_entries(&ctx.entry_declarations, &mut roots);
    // `build_with_progress` emits a per-pass step bar for each
    // finding-attach + a real bar for collect_hot_paths + phase
    // labels for the remaining sub-rollups, so the user never sees
    // "assembling report…" hang silently for minutes on a 700-entry
    // repo.
    let report = Report::build_with_progress(
        &ctx.all_tags,
        &ctx.graph,
        roots,
        &ctx.language_stats,
        Some(root),
        ctx.entry_declarations,
        progress,
    );
    // NOTE: we deliberately do NOT call `progress.finish()` here.
    // Callers may have additional progress phases to emit *after*
    // analyze returns (e.g. the CLI's JSON serialize + file write),
    // and finish() commits the overall bar to scrollback — locking
    // out any post-analyze phase from contributing to the visible
    // progress. Ownership of the bar lifecycle therefore lives with
    // the caller (main.rs's run_scan / run_analyze_root).
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
    analyze_roots_with_progress(root, discover, opts, &NullProgress)
}

/// Same as [`analyze_roots`] but lets the caller observe pipeline
/// progress. See [`analyze_with_progress`] for the design rationale.
pub fn analyze_roots_with_progress(
    root: &Path,
    discover: &DiscoverOpts,
    opts: &AnalyzeOptions,
    progress: &dyn Progress,
) -> Result<AnalyzeOutcome> {
    let ctx = build_graph_context(root, opts, progress);
    // discover_roots_with_progress emits its own "scanning roots"
    // step bar — no extra `phase()` label needed here.
    let discovered = crate::roots::discover_roots_with_progress(
        &ctx.graph,
        root,
        discover,
        progress,
    );
    let ids: Vec<_> = discovered.iter().map(|r| r.id.clone()).collect();
    let mut roots = build_trees_from_ids(&ctx, root, &ids, opts, progress);
    docker::label_call_tree_entries(&ctx.entry_declarations, &mut roots);
    // See the matching call in `analyze_with_progress` above for the
    // motivation — `analyze-root` with `--min-reach 1` produced the
    // 700+ entry case the user hit, which is exactly where the
    // per-pass progress matters most.
    let report = Report::build_with_progress(
        &ctx.all_tags,
        &ctx.graph,
        roots,
        &ctx.language_stats,
        Some(root),
        ctx.entry_declarations,
        progress,
    );
    // NOTE: we deliberately do NOT call `progress.finish()` here.
    // Callers may have additional progress phases to emit *after*
    // analyze returns (e.g. the CLI's JSON serialize + file write),
    // and finish() commits the overall bar to scrollback — locking
    // out any post-analyze phase from contributing to the visible
    // progress. Ownership of the bar lifecycle therefore lives with
    // the caller (main.rs's run_scan / run_analyze_root).
    Ok(AnalyzeOutcome {
        report,
        unresolved_entries: Vec::new(),
        language_stats: ctx.language_stats,
        profiled_language: ctx.profiled_language,
        discovered_roots: discovered,
    })
}
