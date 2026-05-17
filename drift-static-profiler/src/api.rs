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
    sql_lint::{SqlDialect, SqlFileOpts},
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
    /// Walker-level filter: skip directories matching `static`/`assets`
    /// (see [`crate::walker::STATIC_ASSET_DIRS`]). Default **true** because
    /// these dirs almost always hold vendored minified bundles that
    /// dominate the entry-point picker with synthetic top callers
    /// (e.g. swagger-ui's `Gk`, `Ek` with reach 4000+). Forwarded to
    /// `WalkOpts.exclude_static_assets`. UI surfaces this as a settings
    /// toggle so users analyzing a project where these dirs really hold
    /// hand-written source can disable the filter.
    pub exclude_static_assets: bool,
    /// Run the `.sql`-file scan pass (plan §3.2). When `true`, drift
    /// walks the project root for `*.sql` files, parses each with the
    /// inferred dialect, and emits a synthetic entry per file
    /// carrying the SQL rule findings. Default `true`. Opt-out via CLI
    /// `--no-sql-files`.
    pub scan_sql_files: bool,
    /// Explicit SQL dialect override for the `.sql`-file scan pass.
    /// When `None` (default), drift infers the dialect per-file from
    /// path and sibling configs ([`crate::sql_lint::infer_dialect_for_path`]).
    /// When `Some(d)`, every `.sql` file uses `d`, ignoring inference.
    /// Driven by CLI `--sql-dialect <name>`.
    pub sql_dialect_override: Option<SqlDialect>,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            max_depth: 12,
            skip_accessors: false,
            exclude_tests: false,
            exclude_static_assets: true,
            scan_sql_files: true,
            sql_dialect_override: None,
        }
    }
}

/// Materialize the `SqlFileOpts` Report::build expects, OR `None` to
/// disable the pass entirely. Single source of truth for translating
/// public `AnalyzeOptions` into private pipeline opts — keeps the
/// `Report::build_with_progress` call sites identical.
fn sql_file_opts_from(opts: &AnalyzeOptions) -> Option<SqlFileOpts> {
    if !opts.scan_sql_files {
        return None;
    }
    Some(SqlFileOpts {
        dialect_override: opts.sql_dialect_override,
    })
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
        exclude_static_assets: opts.exclude_static_assets,
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
    let sql_opts = sql_file_opts_from(opts);
    let report = Report::build_with_progress(
        &ctx.all_tags,
        &ctx.graph,
        roots,
        &ctx.language_stats,
        Some(root),
        ctx.entry_declarations,
        sql_opts.as_ref(),
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
    let sql_opts = sql_file_opts_from(opts);
    let report = Report::build_with_progress(
        &ctx.all_tags,
        &ctx.graph,
        roots,
        &ctx.language_stats,
        Some(root),
        ctx.entry_declarations,
        sql_opts.as_ref(),
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

/// One row of display data for the interactive root picker — enough
/// for the caller (e.g. a CLI prompt) to render a meaningful menu
/// without needing to know about `CallGraph` or `GraphContext`.
///
/// `callers` is the resolved (name, file, line) for each in-graph
/// caller of this root. For most genuine entry points the list is
/// empty; for symbols called only by the synthetic `<module>` symbol
/// (Python `if __name__`, module-top-level invocations) it contains
/// that single entry.
#[derive(Debug, Clone)]
pub struct PickerRoot {
    pub id: crate::graph::SymbolId,
    pub name: String,
    pub reach: usize,
    /// File path relative to the scanned `root` directory.
    pub file: String,
    pub line: usize,
    pub callers: Vec<PickerCaller>,
}

#[derive(Debug, Clone)]
pub struct PickerCaller {
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// Build the graph + discover roots + hand the top-N to a caller-supplied
/// `pick` closure. If the closure returns `Some(index)`, build a focused
/// report containing only the call tree of that root; if it returns
/// `None`, abort cleanly without doing any tree work.
///
/// Design notes (Clean Code split):
///   - All terminal I/O (prompt rendering, stdin parsing) lives in the
///     `pick` closure — the library crate has no awareness of TTYs.
///   - The closure receives pre-decorated `PickerRoot` rows, not the
///     raw graph, so it can't accidentally mutate analyzer state.
///   - We only build trees AFTER a selection is made. The expensive
///     per-root expansion (max_depth recursion) is paid exactly once,
///     for the one root the user actually wants.
///
/// `discover.max_roots` is honored: pass `10` (or whatever cap the UI
/// wants) so the picker doesn't get handed thousands of rows.
pub fn analyze_picked_with_progress<F>(
    root: &Path,
    discover: &DiscoverOpts,
    opts: &AnalyzeOptions,
    progress: &dyn Progress,
    pick: F,
) -> Result<Option<AnalyzeOutcome>>
where
    F: FnOnce(&[PickerRoot]) -> Option<usize>,
{
    let ctx = build_graph_context(root, opts, progress);
    let discovered = crate::roots::discover_roots_with_progress(
        &ctx.graph,
        root,
        discover,
        progress,
    );

    // Empty-graph short-circuit: nothing to pick. The caller decides
    // how to surface this; we just return None.
    if discovered.is_empty() {
        return Ok(None);
    }

    let rows = decorate_roots_for_picker(&ctx, root, &discovered);

    let picked_idx = match pick(&rows) {
        Some(i) if i < discovered.len() => i,
        _ => return Ok(None),
    };
    let picked = &discovered[picked_idx];

    // Single-entry build path: identical to `analyze_with_progress`
    // beyond this point but with a 1-element `ids` slice.
    let ids = vec![picked.id.clone()];
    let mut roots = build_trees_from_ids(&ctx, root, &ids, opts, progress);
    docker::label_call_tree_entries(&ctx.entry_declarations, &mut roots);
    let sql_opts = sql_file_opts_from(opts);
    let report = Report::build_with_progress(
        &ctx.all_tags,
        &ctx.graph,
        roots,
        &ctx.language_stats,
        Some(root),
        ctx.entry_declarations,
        sql_opts.as_ref(),
        progress,
    );
    Ok(Some(AnalyzeOutcome {
        report,
        unresolved_entries: Vec::new(),
        language_stats: ctx.language_stats,
        profiled_language: ctx.profiled_language,
        // Carry the discovered roots through so the caller can echo
        // metadata about what was on the menu.
        discovered_roots: discovered,
    }))
}

/// Resolve each `DiscoveredRoot` to a `PickerRoot` carrying display
/// fields. Path-stripping mirrors `tree::build_inner`'s convention:
/// paths under the scanned root render relative; anything outside
/// renders absolute.
fn decorate_roots_for_picker(
    ctx: &GraphContext,
    root_dir: &Path,
    discovered: &[crate::roots::DiscoveredRoot],
) -> Vec<PickerRoot> {
    discovered
        .iter()
        .filter_map(|r| {
            let sym = ctx.graph.symbols.get(&r.id)?;
            let file = sym
                .file
                .strip_prefix(root_dir)
                .unwrap_or(&sym.file)
                .display()
                .to_string();
            let callers = ctx
                .graph
                .callers
                .get(&r.id)
                .map(|cids| {
                    cids.iter()
                        .filter_map(|cid| {
                            let s = ctx.graph.symbols.get(cid)?;
                            Some(PickerCaller {
                                name: s.name.clone(),
                                file: s
                                    .file
                                    .strip_prefix(root_dir)
                                    .unwrap_or(&s.file)
                                    .display()
                                    .to_string(),
                                line: s.line,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(PickerRoot {
                id: r.id.clone(),
                name: r.name.clone(),
                reach: r.reach,
                file,
                line: sym.line,
                callers,
            })
        })
        .collect()
}
