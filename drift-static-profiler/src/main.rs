use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use drift_static_profiler::{
    analyze, analyze_roots_with_progress, analyze_with_progress, compute_language_stats,
    tags::extract_tags, tree::render_ascii, walker::discover_source_files, AnalyzeOptions,
    CliProgress, DiscoverOpts, LanguageStats, NullProgress, Progress,
};
use std::path::PathBuf;

/// Pick the progress sink for the CLI context.
///
/// `CliProgress` is backed by `indicatif::MultiProgress`, which:
///   - draws live bars only when stderr is a TTY,
///   - silently skips bar redraws on non-TTY (CI / pipe / log
///     capture), but still routes per-phase `✓ <label> in Xs`
///     completion lines through `eprintln!` via the `commit_line`
///     helper, so log-shaped output stays informative.
///
/// We therefore use `CliProgress` unconditionally — no `IsTerminal`
/// gate — unless the user explicitly opts out via `DRIFT_PROGRESS=off`.
/// That env var is the escape hatch for "I really want no output at
/// all even though I'm on a TTY" (rare but useful for clean script
/// composition).
fn pick_progress() -> Box<dyn Progress> {
    if std::env::var("DRIFT_PROGRESS").as_deref() == Ok("off") {
        Box::new(NullProgress)
    } else {
        Box::new(CliProgress::new())
    }
}

/// Parse the user-facing CLI `--sql-dialect <NAME>` value into the
/// internal enum. Returns `Ok(None)` when the flag wasn't passed;
/// returns `Err` with a friendly message when the name is unknown so
/// the user sees the accepted set up-front rather than discovering it
/// only when a `.sql` file fails to parse.
fn resolve_sql_dialect(
    name: Option<&str>,
) -> Result<Option<drift_static_profiler::sql_lint::SqlDialect>> {
    match name {
        None => Ok(None),
        Some(s) => drift_static_profiler::sql_lint::SqlDialect::parse(s)
            .map(Some)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown --sql-dialect {s:?}; accepted: postgres, mysql, \
                     sqlite, mssql, snowflake, bigquery, generic"
                )
            }),
    }
}

#[derive(Parser)]
#[command(name = "drift-static-profiler", version, about = "Static call-tree analyzer")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Analyze a project root and emit a call tree rooted at one or more symbols.
    Analyze {
        /// Project root to walk
        path: PathBuf,
        /// Entry-point symbol name (e.g. createOrder, create_order). Repeatable.
        #[arg(short, long)]
        entry: Vec<String>,
        /// Emit JSON instead of ASCII tree
        #[arg(long)]
        json: bool,
        /// Max tree depth (default 12)
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
        /// Hide trivial getX/setX/isX accessors in the tree
        #[arg(long)]
        no_accessors: bool,
        /// Exclude test/spec/mock files entirely (path segments + filename
        /// conventions). See `walker::is_test_path` for the full rule.
        #[arg(long)]
        no_tests: bool,
    },
    /// Dump all extracted tags (definitions + references) for a project.
    Tags {
        path: PathBuf,
    },
    /// Analyze any full path and write a JSON report directly into the viewer's
    /// fixtures directory so it shows up at http://localhost:5180/.
    ///
    /// Example:
    ///   drift-static-profiler scan /Users/me/code/myproj --entry handleRequest --name myproj
    Scan {
        /// Absolute or relative path to the project root to analyze
        path: PathBuf,
        /// Entry-point symbol name (repeatable). If omitted, the report will
        /// still contain summary/graph data but no rooted call tree.
        #[arg(short, long)]
        entry: Vec<String>,
        /// Fixture name (no extension). Defaults to "custom". The JSON is
        /// written to `<out_dir>/<name>.json`.
        #[arg(long, default_value = "custom")]
        name: String,
        /// Output directory. Defaults to the viewer's public/fixtures folder
        /// relative to the current working directory.
        #[arg(long, default_value = "viewer/public/fixtures")]
        out_dir: PathBuf,
        /// Max tree depth (default 12)
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
        /// Hide trivial getX/setX/isX accessors in the tree
        #[arg(long)]
        no_accessors: bool,
        /// Exclude test/spec/mock files entirely (path segments like
        /// `tests/`, `__tests__/`, `spec/` AND filename conventions like
        /// `*.test.ts`, `*_test.go`, `test_*.py`, `*Test.java`).
        #[arg(long)]
        no_tests: bool,
        /// Skip the `.sql` file scan pass (plan §3.2). By default
        /// drift walks the project for `*.sql` files, parses each with
        /// the inferred dialect, and reports SQL-rule findings as
        /// synthetic entries.
        #[arg(long)]
        no_sql_files: bool,
        /// Force a SQL dialect for the `.sql` file scan. Overrides
        /// drift's per-file inference. Accepted values: postgres,
        /// mysql, sqlite, mssql, snowflake, bigquery, generic.
        #[arg(long, value_name = "DIALECT")]
        sql_dialect: Option<String>,
        /// Also print the ASCII call tree to stdout
        #[arg(long)]
        print: bool,
    },
    /// Auto-discover every plausible root entry point in a project (symbols
    /// with no in-graph caller, ranked by transitive reach) and emit a single
    /// JSON report containing the call tree of each one. The viewer's "Roots"
    /// tab renders this as a sortable table; clicking a row drills into that
    /// entry's flame graph and call tree (same drill-in pattern as Chrome
    /// DevTools' Top-Down view, pprof's `top -cum`, or Speedscope's Sandwich).
    ///
    /// Example:
    ///   drift-static-profiler analyze-root /Users/me/code/myproj --name myproj-roots
    AnalyzeRoot {
        /// Absolute or relative path to the project root to analyze
        path: PathBuf,
        /// Fixture name (no extension). Defaults to "roots".
        #[arg(long, default_value = "roots")]
        name: String,
        /// Output directory. Defaults to the viewer's public/fixtures folder
        /// relative to the current working directory.
        #[arg(long, default_value = "viewer/public/fixtures")]
        out_dir: PathBuf,
        /// Minimum transitive reach (deduped subtree size) for a symbol to
        /// qualify as a root worth profiling. Default 2 drops leaves with no
        /// in-project callees; raise it to focus on top-level handlers.
        #[arg(long, default_value_t = 2)]
        min_reach: usize,
        /// Hard cap on number of discovered roots. Default 200 — generous but
        /// bounded so the viewer doesn't choke on a monorepo.
        #[arg(long, default_value_t = 200)]
        max_roots: usize,
        /// Include symbols under test/spec paths (off by default).
        #[arg(long)]
        include_tests: bool,
        /// Include language-conventional private symbols (`_foo`, off by default).
        #[arg(long)]
        include_private: bool,
        /// Include trivial accessors (`getX`/`setX`/`isX`, off by default).
        #[arg(long)]
        include_accessors: bool,
        /// Max tree depth per root (default 12)
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
        /// Hide accessor frames inside the per-root tree (mirrors `analyze`
        /// flag). Independent from `--include-accessors`, which controls the
        /// roots-list filter.
        #[arg(long)]
        no_accessors: bool,
        /// Exclude test/spec/mock files from the WALK entirely — different
        /// from `--include-tests`, which only controls the discovery filter
        /// (root candidates). With `--no-tests`, test files don't reach the
        /// graph at all, so they don't show up as dead_code, callees, or in
        /// `findings_top`. Implies `--no-tests` semantics in `roots.rs` too.
        #[arg(long)]
        no_tests: bool,
        /// Skip the `.sql` file scan pass (plan §3.2). By default
        /// drift walks the project for `*.sql` files, parses each with
        /// the inferred dialect, and reports SQL-rule findings as
        /// synthetic entries.
        #[arg(long)]
        no_sql_files: bool,
        /// Force a SQL dialect for the `.sql` file scan. Overrides
        /// drift's per-file inference. Accepted values: postgres,
        /// mysql, sqlite, mssql, snowflake, bigquery, generic.
        #[arg(long, value_name = "DIALECT")]
        sql_dialect: Option<String>,
        /// Also print the discovered roots table to stderr
        #[arg(long)]
        print: bool,
    },
    /// Interactive scan: discover root entry points, show the top 10
    /// by reach, prompt for a selection, then build a focused report
    /// on just that one entry.
    ///
    /// Defaults invert the `scan` command's "everything" stance:
    /// test/spec/mock files are excluded at the walker stage so the
    /// menu shows production roots. Pass `--include-tests` to bring
    /// test entry points back into the candidate list.
    ///
    /// The graph is built ONCE and reused for both discovery and the
    /// focused analysis, so the prompt costs no extra parsing.
    ///
    /// Example:
    ///   drift-static-profiler scan-prompt /Users/me/code/myproj
    ScanPrompt {
        /// Absolute or relative path to the project root to analyze
        path: PathBuf,
        /// Bring test/spec/mock files back into the walk. Off by default
        /// (opposite of `scan`) — the prompt is for picking real entry
        /// points, not test fixtures.
        #[arg(long)]
        include_tests: bool,
        /// Fixture name (no extension). Defaults to the basename of `path`.
        #[arg(long)]
        name: Option<String>,
        /// Output directory. Defaults to the viewer's scans fixture dir.
        #[arg(long, default_value = "viewer/public/fixtures/scans")]
        out_dir: PathBuf,
        /// Max tree depth (default 12)
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
        /// Hide trivial getX/setX/isX accessors in the call tree.
        #[arg(long)]
        no_accessors: bool,
        /// Minimum transitive reach for a symbol to appear on the menu.
        #[arg(long, default_value_t = 2)]
        min_reach: usize,
    },
    /// Rebuild the scans index used by the viewer's landing page.
    ///
    /// Walks `<dir>` for `*.json` files (excluding `index.json` itself),
    /// extracts `generator.source_root` from each scan's PREFIX (no full
    /// parse — see `scans_index::extract_source_root`), and writes
    /// `<dir>/index.json` atomically.
    ///
    /// Example:
    ///   drift-static-profiler regen-scans-index viewer/public/fixtures/scans
    RegenScansIndex {
        /// Directory holding the scan JSONs. Defaults to the viewer's
        /// scans fixture dir relative to the current working directory.
        #[arg(default_value = "viewer/public/fixtures/scans")]
        dir: PathBuf,
    },
    /// Compare two report JSONs (baseline vs current). Exit non-zero if regressions found.
    Diff {
        baseline: PathBuf,
        current: PathBuf,
        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,
        /// Exit 0 even when regressions are found (default: exit 1)
        #[arg(long)]
        no_fail: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Analyze {
            path,
            entry,
            json,
            max_depth,
            no_accessors,
            no_tests,
        } => run_analyze(&path, &entry, json, max_depth, no_accessors, no_tests),
        Cmd::Tags { path } => run_tags(&path),
        Cmd::RegenScansIndex { dir } => run_regen_scans_index(&dir),
        Cmd::ScanPrompt {
            path,
            include_tests,
            name,
            out_dir,
            max_depth,
            no_accessors,
            min_reach,
        } => run_scan_prompt(
            &path,
            include_tests,
            name.as_deref(),
            &out_dir,
            max_depth,
            no_accessors,
            min_reach,
        ),
        Cmd::Diff {
            baseline,
            current,
            json,
            no_fail,
        } => run_diff(&baseline, &current, json, no_fail),
        Cmd::Scan {
            path,
            entry,
            name,
            out_dir,
            max_depth,
            no_accessors,
            no_tests,
            no_sql_files,
            sql_dialect,
            print,
        } => run_scan(
            &path,
            &entry,
            &name,
            &out_dir,
            max_depth,
            no_accessors,
            no_tests,
            no_sql_files,
            sql_dialect.as_deref(),
            print,
        ),
        Cmd::AnalyzeRoot {
            path,
            name,
            out_dir,
            min_reach,
            max_roots,
            include_tests,
            include_private,
            include_accessors,
            max_depth,
            no_accessors,
            no_tests,
            no_sql_files,
            sql_dialect,
            print,
        } => run_analyze_root(
            &path,
            &name,
            &out_dir,
            min_reach,
            max_roots,
            include_tests,
            include_private,
            include_accessors,
            max_depth,
            no_accessors,
            no_tests,
            no_sql_files,
            sql_dialect.as_deref(),
            print,
        ),
    }
}

fn run_diff(
    baseline: &std::path::Path,
    current: &std::path::Path,
    json: bool,
    no_fail: bool,
) -> Result<()> {
    use drift_static_profiler::{diff, report::Report};
    let base: Report = serde_json::from_slice(
        &std::fs::read(baseline)
            .with_context(|| format!("read baseline {}", baseline.display()))?,
    )
    .context("parse baseline JSON")?;
    let cur: Report = serde_json::from_slice(
        &std::fs::read(current)
            .with_context(|| format!("read current {}", current.display()))?,
    )
    .context("parse current JSON")?;

    let d = diff::diff(&base, &cur);

    if json {
        println!("{}", serde_json::to_string_pretty(&d).context("serialize")?);
    } else {
        print!("{}", diff::render(&d));
    }

    if !no_fail && !d.regressions.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_analyze(
    root: &std::path::Path,
    entries: &[String],
    json: bool,
    max_depth: usize,
    no_accessors: bool,
    no_tests: bool,
) -> Result<()> {
    if entries.is_empty() {
        eprintln!("note: no --entry given; pass one or more entry-point symbol names");
        return Ok(());
    }

    let outcome = analyze(
        root,
        entries,
        &AnalyzeOptions {
            max_depth,
            skip_accessors: no_accessors,
            exclude_tests: no_tests,
            ..AnalyzeOptions::default()
        },
    )?;
    print_language_summary(&outcome.language_stats);
    for q in &outcome.unresolved_entries {
        eprintln!("warn: no symbol matched entry {q:?}");
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&outcome.report).context("serialize")?
        );
    } else {
        for r in &outcome.report.entries {
            println!("{}", render_ascii(r));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_scan(
    root: &std::path::Path,
    entries: &[String],
    name: &str,
    out_dir: &std::path::Path,
    max_depth: usize,
    no_accessors: bool,
    no_tests: bool,
    no_sql_files: bool,
    sql_dialect: Option<&str>,
    print: bool,
) -> Result<()> {
    let progress = pick_progress();
    let sql_dialect_override = resolve_sql_dialect(sql_dialect)?;
    let outcome = analyze_with_progress(
        root,
        entries,
        &AnalyzeOptions {
            max_depth,
            skip_accessors: no_accessors,
            exclude_tests: no_tests,
            scan_sql_files: !no_sql_files,
            sql_dialect_override,
            ..AnalyzeOptions::default()
        },
        progress.as_ref(),
    )?;
    // Serialize + write are the last two phases of the scan from the
    // user's perspective: `serde_json::to_string_pretty` on a 700-
    // entry report can take a couple of seconds, and writing the
    // resulting (possibly 100MB+) JSON to disk is non-trivial too.
    // Both used to be silent — surface them so the user sees the
    // overall bar reach its final phases instead of hanging.
    write_report_with_progress(&outcome, name, out_dir, progress.as_ref())?;
    progress.finish();

    print_language_summary(&outcome.language_stats);
    for q in &outcome.unresolved_entries {
        eprintln!("warn: no symbol matched entry {q:?}");
    }
    eprintln!(
        "✓ wrote viewer/public/fixtures/{name}.json ({} entries, {} symbols)",
        outcome.report.entries.len(),
        outcome.report.summary.symbols,
    );
    eprintln!(
        "  open the viewer (make viewer) and pick the fixture named '{name}' to see it",
    );

    if print {
        for r in &outcome.report.entries {
            println!("{}", render_ascii(r));
        }
    }
    Ok(())
}

/// Serialize the report to pretty JSON and stream it to disk via a
/// `BufWriter`, with a single `phase()` label so the CLI's overall
/// bar surfaces the work. Shared between `run_scan` and
/// `run_analyze_root` because both have the identical write tail.
///
/// Why streaming (vs. the old `to_string_pretty` + `fs::write`):
///   - `to_string_pretty` serializes the WHOLE report into a `String`
///     before any bytes hit disk. On a large polyglot scan that's a
///     100MB+ allocation that lives alongside the report's own
///     in-memory structures — easy to push peak RSS past 1 GB.
///   - `to_writer_pretty` walks the serde tree and pushes bytes
///     directly through the writer. With a 256 KB-buffered
///     `BufWriter` in front of the file we get amortized 256 KB
///     syscalls instead of one monolithic `fs::write`. Net effect:
///     same wall time on small reports, and **no double-buffer
///     memory cost on big ones**.
///
/// One phase, not two: serialize and write are interleaved by the
/// streaming path (serde produces bytes → BufWriter accumulates →
/// flushes at 256 KB boundaries → disk). The user can't meaningfully
/// separate "CPU-bound serialize" from "IO-bound write" anymore, so
/// we surface a single combined `writing …` phase. If the merged
/// timing ever masks a slow regression we can split it apart again,
/// but with streaming the wall-clock IS one timer.
fn write_report_with_progress(
    outcome: &drift_static_profiler::AnalyzeOutcome,
    name: &str,
    out_dir: &std::path::Path,
    progress: &dyn Progress,
) -> Result<()> {
    use std::io::{BufWriter, Write};

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("create output dir {}", out_dir.display()))?;
    let out_path = out_dir.join(format!("{name}.json"));

    progress.phase(&format!("writing {}…", out_path.display()));
    // BufWriter capacity: 256 KB. Default is 8 KB which means lots of
    // syscalls on a 100MB+ JSON. 256 KB hits a sweet spot for the
    // standard fs read-ahead size on macOS / Linux without being
    // wasteful for small reports (the buffer is freed on drop).
    let file = std::fs::File::create(&out_path)
        .with_context(|| format!("create report file {}", out_path.display()))?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    serde_json::to_writer_pretty(&mut writer, &outcome.report).context("serialize")?;
    // Flush the BufWriter before closing so a partial write surfaces
    // as an io::Error here, not as a silently-truncated JSON file
    // discovered later by the viewer.
    writer
        .flush()
        .with_context(|| format!("flush report to {}", out_path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_analyze_root(
    root: &std::path::Path,
    name: &str,
    out_dir: &std::path::Path,
    min_reach: usize,
    max_roots: usize,
    include_tests: bool,
    include_private: bool,
    include_accessors: bool,
    max_depth: usize,
    no_accessors: bool,
    no_tests: bool,
    no_sql_files: bool,
    sql_dialect: Option<&str>,
    print: bool,
) -> Result<()> {
    // `--no-tests` (walker-level filter) implies `--no-include-tests`
    // (discover-roots filter): if test files don't reach the graph at
    // all, there's no test code left for roots to discover anyway. But
    // we honor `--include-tests` if the user passes BOTH (they get
    // whatever the walker emitted). Default behavior unchanged.
    let discover = DiscoverOpts {
        min_reach,
        skip_tests: !include_tests,
        skip_private: !include_private,
        skip_accessors: !include_accessors,
        max_roots,
    };
    let progress = pick_progress();
    let sql_dialect_override = resolve_sql_dialect(sql_dialect)?;
    let outcome = analyze_roots_with_progress(
        root,
        &discover,
        &AnalyzeOptions {
            max_depth,
            skip_accessors: no_accessors,
            exclude_tests: no_tests,
            scan_sql_files: !no_sql_files,
            sql_dialect_override,
            ..AnalyzeOptions::default()
        },
        progress.as_ref(),
    )?;
    // Same write tail as run_scan — the JSON serialize + disk write
    // are the last visible phases of the scan and used to be silent.
    write_report_with_progress(&outcome, name, out_dir, progress.as_ref())?;
    progress.finish();

    print_language_summary(&outcome.language_stats);
    eprintln!(
        "discovered {} root entry points (min_reach={min_reach}, max_roots={max_roots})",
        outcome.discovered_roots.len(),
    );
    eprintln!(
        "✓ wrote {}/{}.json ({} entries, {} symbols)",
        out_dir.display(),
        name,
        outcome.report.entries.len(),
        outcome.report.summary.symbols,
    );
    eprintln!(
        "  open the viewer (make viewer) and pick the fixture named '{name}' to see it",
    );

    if print {
        eprintln!("\ntop roots (ranked by reach):");
        for (i, r) in outcome.discovered_roots.iter().take(20).enumerate() {
            eprintln!("  {:>3}. {:<32} reach={}", i + 1, r.name, r.reach);
        }
    }
    Ok(())
}

/// Interactive root-pick scan. See `Cmd::ScanPrompt`.
///
/// Pipeline:
///   1. Build graph + discover top-N roots (excluding tests unless
///      `include_tests`).
///   2. Render the menu to stderr, read selection from stdin.
///   3. If a root is picked: build a focused report for that one
///      entry, write JSON, regenerate the scans index.
///   4. If user quits or stdin is closed: exit cleanly with code 0.
///
/// We intentionally pin `--max-roots = 10` here so the menu is always
/// bounded. If a user wants the full discovered list, `analyze-root`
/// remains the right tool.
#[allow(clippy::too_many_arguments)]
fn run_scan_prompt(
    path: &std::path::Path,
    include_tests: bool,
    name: Option<&str>,
    out_dir: &std::path::Path,
    max_depth: usize,
    no_accessors: bool,
    min_reach: usize,
) -> Result<()> {
    use drift_static_profiler::{analyze_picked_with_progress, AnalyzeOptions, DiscoverOpts};
    use std::io::{IsTerminal, Write};

    // Sanity: scan-prompt is interactive by design. Refuse early when
    // stdin isn't a TTY so users in CI/pipes don't get a hanging read.
    // The env-var escape hatch exists for testing only — exposes the
    // same prompt-loop on a piped stdin so end-to-end smoke tests can
    // verify the picker without needing a pty harness.
    if !std::io::stdin().is_terminal()
        && std::env::var("DRIFT_SCAN_PROMPT_ALLOW_PIPE").is_err()
    {
        anyhow::bail!(
            "scan-prompt requires an interactive terminal — pipe or CI invocation detected.\n\
             For non-interactive scanning use `analyze-root` (auto-discovers all roots).",
        );
    }

    let progress = pick_progress();
    let discover = DiscoverOpts {
        min_reach,
        skip_tests: !include_tests,
        skip_private: true,
        skip_accessors: true,
        max_roots: 10,
    };
    let opts = AnalyzeOptions {
        max_depth,
        skip_accessors: no_accessors,
        // include_tests=false → walker drops test files at the walk stage
        // so they don't show up as callees or in dead_code either.
        exclude_tests: !include_tests,
        ..AnalyzeOptions::default()
    };

    let outcome = analyze_picked_with_progress(
        path,
        &discover,
        &opts,
        progress.as_ref(),
        |rows| pick_root_via_stdin(rows),
    )?;
    progress.finish();

    let Some(outcome) = outcome else {
        eprintln!("no root selected — nothing written.");
        return Ok(());
    };

    // Resolve output filename: explicit --name wins, else basename of path.
    let derived_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("scan")
        .to_string();
    let fixture_name = name.unwrap_or(&derived_name);

    write_report_with_progress(&outcome, fixture_name, out_dir, progress.as_ref())?;

    print_language_summary(&outcome.language_stats);
    eprintln!(
        "✓ wrote {}/{}.json ({} entries, {} symbols)",
        out_dir.display(),
        fixture_name,
        outcome.report.entries.len(),
        outcome.report.summary.symbols,
    );

    // Refresh scans/index.json so the viewer picks up the new scan
    // on its next load. Matches the post-scan tail in `make scan`.
    let _ = drift_static_profiler::scans_index::regen(out_dir);

    // Flush stderr so the final summary lands before the shell prompt
    // returns — otherwise the indicatif draw thread can interleave.
    let _ = std::io::stderr().flush();
    Ok(())
}

/// Render the picker menu to stderr and read a 1-based selection from
/// stdin. Returns the zero-based index of the chosen row, or `None`
/// for "user quit" / "EOF" / "no rows".
///
/// The prompt loop accepts:
///   - `1`..`N` → pick that row (1-based for human ergonomics)
///   - `q` / `quit` / empty line / EOF → abort
///   - anything else → re-prompt with an error message
fn pick_root_via_stdin(rows: &[drift_static_profiler::PickerRoot]) -> Option<usize> {
    use std::io::{BufRead, Write};
    if rows.is_empty() {
        eprintln!("no root entry points discovered (try lowering --min-reach).");
        return None;
    }

    eprintln!();
    eprintln!("top {} roots by reach (descending):", rows.len());
    eprintln!();
    for (i, r) in rows.iter().enumerate() {
        eprintln!(
            "  {:>2}. {:<32} reach={:<5} {}:{}",
            i + 1,
            r.name,
            r.reach,
            r.file,
            r.line,
        );
        if r.callers.is_empty() {
            eprintln!("      callers: <none — entry point>");
        } else {
            let summary: Vec<String> = r
                .callers
                .iter()
                .take(2)
                .map(|c| format!("{} ({}:{})", c.name, c.file, c.line))
                .collect();
            let extra = if r.callers.len() > 2 {
                format!(" +{} more", r.callers.len() - 2)
            } else {
                String::new()
            };
            eprintln!("      callers: {}{}", summary.join(", "), extra);
        }
    }
    eprintln!();

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        eprint!("pick 1-{} (or 'q' to quit): ", rows.len());
        let _ = std::io::stderr().flush();
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => return None, // EOF
            Ok(_) => {}
            Err(_) => return None,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
            return None;
        }
        match trimmed.parse::<usize>() {
            Ok(n) if (1..=rows.len()).contains(&n) => return Some(n - 1),
            _ => eprintln!("  ! not a valid choice (expected 1-{} or 'q'); try again.", rows.len()),
        }
    }
}

fn run_regen_scans_index(dir: &std::path::Path) -> Result<()> {
    let count = drift_static_profiler::scans_index::regen(dir)
        .with_context(|| format!("regenerate scans index in {}", dir.display()))?;
    let plural = if count == 1 { "" } else { "s" };
    eprintln!(
        "  ↻ wrote {}/index.json ({count} scan{plural})",
        dir.display(),
    );
    Ok(())
}

fn run_tags(root: &std::path::Path) -> Result<()> {
    let stats = compute_language_stats(root);
    print_language_summary(&stats);
    let files: Vec<_> = match stats.dominant_supported {
        Some(target) => discover_source_files(root)
            .into_iter()
            .filter(|(_, l)| *l == target)
            .collect(),
        None => {
            eprintln!("note: no supported language detected; nothing to tag");
            return Ok(());
        }
    };
    for (file, lang) in files {
        match extract_tags(&file, lang) {
            Ok(tags) => {
                for s in &tags.symbols {
                    let parent = s.parent.clone().unwrap_or_default();
                    let kind = match s.kind {
                        drift_static_profiler::SymbolKind::Function => "fn",
                        drift_static_profiler::SymbolKind::Method => "method",
                        drift_static_profiler::SymbolKind::Class => "class",
                    };
                    println!(
                        "DEF  {} {parent}.{name}  ({file}:{line})",
                        kind,
                        name = s.name,
                        file = s.file.display(),
                        line = s.line,
                    );
                }
                for r in &tags.references {
                    let inside = r.in_symbol.clone().unwrap_or("<file>".into());
                    println!(
                        "REF  {name}  (called inside {inside} @ {file}:{line})",
                        name = r.name,
                        file = r.file.display(),
                        line = r.line,
                    );
                }
            }
            Err(e) => eprintln!("warn: failed to parse {}: {e:#}", file.display()),
        }
    }
    Ok(())
}

/// Render a GitHub-style language bar and announce which supported language
/// drift will profile. Goes to stderr so it doesn't contaminate `--json`
/// output on stdout.
fn print_language_summary(stats: &LanguageStats) {
    if stats.breakdown.is_empty() {
        eprintln!("languages: (no programming files detected)");
        return;
    }
    let top: Vec<String> = stats
        .breakdown
        .iter()
        .take(6)
        .map(|e| {
            let marker = if e.supported { "*" } else { "" };
            format!("{}{} {:.1}%", e.language, marker, e.percent)
        })
        .collect();
    eprintln!(
        "languages: {}  ({} files, {} bytes)",
        top.join(", "),
        stats.total_files,
        stats.total_bytes,
    );
    match (&stats.dominant_supported_name, stats.dominant_supported_percent) {
        (Some(name), Some(pct)) => {
            eprintln!("profiling: {name} ({pct:.1}% of code) — marked with *")
        }
        _ => eprintln!("profiling: (no supported language present)"),
    }
}
