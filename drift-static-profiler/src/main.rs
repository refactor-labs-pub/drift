use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use drift_static_profiler::{
    graph::CallGraph,
    report::Report,
    tags::extract_tags,
    tree::{render_ascii, TreeBuilder},
    walker::discover_source_files,
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
    let files = discover_source_files(root);
    let mut all = Vec::with_capacity(files.len());
    for (file, lang) in files {
        match extract_tags(&file, lang) {
            Ok(tags) => all.push(tags),
            Err(e) => eprintln!("warn: failed to parse {}: {e:#}", file.display()),
        }
    }
    let graph = CallGraph::build(&all);
    let mut builder = TreeBuilder::new(&graph, root);
    builder.max_depth = max_depth;
    builder.skip_accessors = no_accessors;

    if entries.is_empty() {
        eprintln!("note: no --entry given; pass one or more entry-point symbol names");
        return Ok(());
    }

    let mut roots = Vec::new();
    for q in entries {
        let ids = graph.find_entry_points(q);
        if ids.is_empty() {
            eprintln!("warn: no symbol matched entry {q:?}");
        }
        for id in ids {
            if let Some(node) = builder.build(&id) {
                roots.push(node);
            }
        }
    }

    if json {
        let report = Report::build(&all, &graph, roots);
        println!("{}", serde_json::to_string_pretty(&report).context("serialize")?);
    } else {
        for r in &roots {
            println!("{}", render_ascii(r));
        }
    }
    Ok(())
}

fn run_tags(root: &std::path::Path) -> Result<()> {
    let files = discover_source_files(root);
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
