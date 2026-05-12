use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use drift_static_profiler::{
    analyze, analyze_roots, compute_language_stats, tags::extract_tags, tree::render_ascii,
    walker::discover_source_files, AnalyzeOptions, DiscoverOpts, LanguageStats,
};
use std::path::PathBuf;

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
        /// Also print the discovered roots table to stderr
        #[arg(long)]
        print: bool,
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
        } => run_analyze(&path, &entry, json, max_depth, no_accessors),
        Cmd::Tags { path } => run_tags(&path),
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
            print,
        } => run_scan(&path, &entry, &name, &out_dir, max_depth, no_accessors, print),
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

fn run_analyze(
    root: &std::path::Path,
    entries: &[String],
    json: bool,
    max_depth: usize,
    no_accessors: bool,
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
    print: bool,
) -> Result<()> {
    let outcome = analyze(
        root,
        entries,
        &AnalyzeOptions {
            max_depth,
            skip_accessors: no_accessors,
        },
    )?;
    print_language_summary(&outcome.language_stats);
    for q in &outcome.unresolved_entries {
        eprintln!("warn: no symbol matched entry {q:?}");
    }

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("create output dir {}", out_dir.display()))?;
    let out_path = out_dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&outcome.report).context("serialize")?;
    std::fs::write(&out_path, &json)
        .with_context(|| format!("write report to {}", out_path.display()))?;

    eprintln!(
        "✓ wrote {} ({} entries, {} symbols)",
        out_path.display(),
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
    print: bool,
) -> Result<()> {
    let discover = DiscoverOpts {
        min_reach,
        skip_tests: !include_tests,
        skip_private: !include_private,
        skip_accessors: !include_accessors,
        max_roots,
    };
    let outcome = analyze_roots(
        root,
        &discover,
        &AnalyzeOptions {
            max_depth,
            skip_accessors: no_accessors,
        },
    )?;
    print_language_summary(&outcome.language_stats);

    eprintln!(
        "discovered {} root entry points (min_reach={min_reach}, max_roots={max_roots})",
        outcome.discovered_roots.len(),
    );

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("create output dir {}", out_dir.display()))?;
    let out_path = out_dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&outcome.report).context("serialize")?;
    std::fs::write(&out_path, &json)
        .with_context(|| format!("write report to {}", out_path.display()))?;

    eprintln!(
        "✓ wrote {} ({} entries, {} symbols)",
        out_path.display(),
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
