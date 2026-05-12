//! GitHub-Linguist-style language breakdown for a project tree.
//!
//! The goal is *picking which language to profile*, not byte-perfect parity
//! with GitHub. Concretely we:
//!   1. Walk every file under `root` honoring the same .gitignore /
//!      .driftignore / default-skip rules as the source-file walker
//!      (so build output and vendored deps don't skew percentages).
//!   2. Bucket each file by language using a filename-then-extension
//!      lookup. The table is intentionally broad (Rust, Go, Ruby, Kotlin,
//!      …) so that on a polyglot repo the user sees realistic ratios and
//!      understands why a specific supported language was picked.
//!   3. Only "programming" files contribute to the percentage denominator,
//!      mirroring GitHub's repo page where data/markup are dropped from
//!      the bar by default.
//!   4. Among files of *supported* languages (the four with tree-sitter
//!      parsers shipped in this crate), report the one with the most
//!      bytes as `dominant_supported`. That is the language the static
//!      profiler will actually analyze.
//!
//! Trade-off: byte-by-extension is GitHub's heuristic for the repo bar but
//! NOT what `linguist` itself uses internally (it also looks at shebangs,
//! modelines, and content classifiers). For our "which language dominates
//! this checkout" question the simpler approach is enough.

use crate::{
    walker::{walk_files_with, WalkOpts},
    Language,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LangKind {
    Programming,
    Markup,
    Data,
    Prose,
}

#[derive(Debug, Clone, Copy)]
struct LangInfo {
    name: &'static str,
    kind: LangKind,
    /// Set when this language has a tree-sitter parser shipped in this
    /// crate (i.e. drift can actually profile it). Used to pick the
    /// dominant *supported* language.
    supported: Option<Language>,
}

fn classify(path: &Path) -> Option<LangInfo> {
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if let Some(f) = fname.as_deref() {
        if let Some(info) = classify_filename(f) {
            return Some(info);
        }
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    classify_extension(&ext)
}

fn classify_filename(fname: &str) -> Option<LangInfo> {
    let (name, kind, supported) = match fname {
        "dockerfile" => ("Dockerfile", LangKind::Programming, None),
        "makefile" | "gnumakefile" => ("Makefile", LangKind::Programming, None),
        "cmakelists.txt" => ("CMake", LangKind::Programming, None),
        "rakefile" | "gemfile" => ("Ruby", LangKind::Programming, None),
        "build.gradle" | "settings.gradle" => ("Gradle", LangKind::Programming, None),
        _ => return None,
    };
    Some(LangInfo { name, kind, supported })
}

fn classify_extension(ext: &str) -> Option<LangInfo> {
    let (name, kind, supported) = match ext {
        // ── programming (supported by drift) ─────────────────────────────
        "py" | "pyi" | "pyw" => ("Python", LangKind::Programming, Some(Language::Python)),
        "java" => ("Java", LangKind::Programming, Some(Language::Java)),
        "ts" | "tsx" | "mts" | "cts" => {
            ("TypeScript", LangKind::Programming, Some(Language::TypeScript))
        }
        "js" | "jsx" | "mjs" | "cjs" => {
            ("JavaScript", LangKind::Programming, Some(Language::JavaScript))
        }

        // ── programming (also supported by drift) ────────────────────────
        "rs" => ("Rust", LangKind::Programming, Some(Language::Rust)),
        "go" => ("Go", LangKind::Programming, Some(Language::Go)),
        "scala" | "sc" => ("Scala", LangKind::Programming, Some(Language::Scala)),

        // ── programming (other) ──────────────────────────────────────────
        "rb" => ("Ruby", LangKind::Programming, None),
        "php" => ("PHP", LangKind::Programming, None),
        "c" | "h" => ("C", LangKind::Programming, None),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => ("C++", LangKind::Programming, None),
        "cs" => ("C#", LangKind::Programming, None),
        "kt" | "kts" => ("Kotlin", LangKind::Programming, None),
        "swift" => ("Swift", LangKind::Programming, None),
        "sh" | "bash" | "zsh" | "fish" => ("Shell", LangKind::Programming, None),
        "ps1" => ("PowerShell", LangKind::Programming, None),
        "lua" => ("Lua", LangKind::Programming, None),
        "pl" | "pm" => ("Perl", LangKind::Programming, None),
        "r" => ("R", LangKind::Programming, None),
        "dart" => ("Dart", LangKind::Programming, None),
        "ex" | "exs" => ("Elixir", LangKind::Programming, None),
        "erl" => ("Erlang", LangKind::Programming, None),
        "hs" => ("Haskell", LangKind::Programming, None),
        "clj" | "cljs" | "cljc" => ("Clojure", LangKind::Programming, None),
        "ml" | "mli" => ("OCaml", LangKind::Programming, None),
        "fs" | "fsx" | "fsi" => ("F#", LangKind::Programming, None),
        "groovy" | "gvy" => ("Groovy", LangKind::Programming, None),
        "vue" => ("Vue", LangKind::Programming, None),
        "svelte" => ("Svelte", LangKind::Programming, None),

        // ── markup ───────────────────────────────────────────────────────
        "html" | "htm" => ("HTML", LangKind::Markup, None),
        "css" | "scss" | "sass" | "less" => ("CSS", LangKind::Markup, None),
        "tex" => ("TeX", LangKind::Markup, None),

        // ── prose ────────────────────────────────────────────────────────
        "md" | "markdown" => ("Markdown", LangKind::Prose, None),
        "rst" => ("reStructuredText", LangKind::Prose, None),

        // ── data ─────────────────────────────────────────────────────────
        "json" => ("JSON", LangKind::Data, None),
        "yaml" | "yml" => ("YAML", LangKind::Data, None),
        "toml" => ("TOML", LangKind::Data, None),
        "xml" => ("XML", LangKind::Data, None),
        "csv" => ("CSV", LangKind::Data, None),
        "sql" => ("SQL", LangKind::Data, None),
        "graphql" | "gql" => ("GraphQL", LangKind::Data, None),
        "proto" => ("Protocol Buffer", LangKind::Data, None),

        _ => return None,
    };
    Some(LangInfo { name, kind, supported })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageBreakdownEntry {
    pub language: String,
    pub bytes: u64,
    pub files: usize,
    /// Share of `LanguageStats::total_bytes` (0.0–100.0).
    pub percent: f64,
    /// True when drift ships a tree-sitter parser for this language and can
    /// actually produce a profiling report for it.
    pub supported: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanguageStats {
    /// Total bytes counted across "programming" files. The denominator for
    /// every entry in `breakdown.percent`.
    pub total_bytes: u64,
    /// Total programming files counted (sum of `breakdown[*].files`).
    pub total_files: usize,
    /// Sorted descending by bytes, then by language name. Only programming
    /// languages — markup/data/prose are dropped from the bar.
    pub breakdown: Vec<LanguageBreakdownEntry>,
    /// The highest-byte language whose static profiler this crate ships.
    /// `None` when no supported source file was found in the tree.
    pub dominant_supported: Option<Language>,
    /// Display name corresponding to `dominant_supported`, for friendly
    /// printing without re-deriving from the enum.
    pub dominant_supported_name: Option<String>,
    /// Share of `total_bytes` accounted for by `dominant_supported`.
    pub dominant_supported_percent: Option<f64>,
}

pub fn compute_language_stats(root: &Path) -> LanguageStats {
    compute_language_stats_with(root, &WalkOpts::default())
}

pub fn compute_language_stats_with(root: &Path, opts: &WalkOpts) -> LanguageStats {
    #[derive(Default)]
    struct Bucket {
        bytes: u64,
        files: usize,
        supported: Option<Language>,
    }

    let mut buckets: HashMap<&'static str, Bucket> = HashMap::new();
    let mut total_bytes: u64 = 0;
    let mut total_files: usize = 0;

    for (path, size) in walk_files_with(root, opts) {
        let Some(info) = classify(&path) else { continue };
        // Only programming files contribute to the bar — JSON fixtures and
        // README.md should not dilute the percentage of the language we'll
        // actually profile.
        if !matches!(info.kind, LangKind::Programming) {
            continue;
        }
        let b = buckets.entry(info.name).or_default();
        b.bytes += size;
        b.files += 1;
        b.supported = info.supported;
        total_bytes += size;
        total_files += 1;
    }

    let mut breakdown: Vec<LanguageBreakdownEntry> = buckets
        .iter()
        .map(|(name, b)| LanguageBreakdownEntry {
            language: (*name).to_string(),
            bytes: b.bytes,
            files: b.files,
            percent: pct(b.bytes, total_bytes),
            supported: b.supported.is_some(),
        })
        .collect();
    breakdown.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.language.cmp(&b.language))
    });

    // Find the dominant *supported* language. Tie-break by language name for
    // determinism — picking by Language enum order would mean a 50/50 TS/JS
    // repo's choice depends on enum declaration order, which is too magic.
    let mut best: Option<(u64, &'static str, Language)> = None;
    for (name, b) in &buckets {
        let Some(lang) = b.supported else { continue };
        let take = match best {
            None => true,
            Some((cur_bytes, cur_name, _)) => {
                b.bytes > cur_bytes || (b.bytes == cur_bytes && *name < cur_name)
            }
        };
        if take {
            best = Some((b.bytes, name, lang));
        }
    }
    let (dominant_supported, dominant_supported_name, dominant_supported_percent) = match best {
        Some((bytes, name, lang)) => (
            Some(lang),
            Some(name.to_string()),
            Some(pct(bytes, total_bytes)),
        ),
        None => (None, None, None),
    };

    LanguageStats {
        total_bytes,
        total_files,
        breakdown,
        dominant_supported,
        dominant_supported_name,
        dominant_supported_percent,
    }
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64) / (total as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("drift-linguist-{label}-{pid}-{n}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("mkdir tmp");
        p
    }

    #[test]
    fn classify_handles_supported_extensions() {
        assert_eq!(
            classify(Path::new("src/main.ts")).map(|i| i.supported),
            Some(Some(Language::TypeScript))
        );
        assert_eq!(
            classify(Path::new("App.java")).map(|i| i.supported),
            Some(Some(Language::Java))
        );
        assert_eq!(
            classify(Path::new("foo.py")).map(|i| i.supported),
            Some(Some(Language::Python))
        );
        assert_eq!(
            classify(Path::new("foo.jsx")).map(|i| i.supported),
            Some(Some(Language::JavaScript))
        );
    }

    #[test]
    fn classify_handles_unsupported_programming() {
        // Use Lua — drift does not ship a tree-sitter parser for it.
        let li = classify(Path::new("src/init.lua")).unwrap();
        assert_eq!(li.name, "Lua");
        assert!(matches!(li.kind, LangKind::Programming));
        assert!(li.supported.is_none());
    }

    #[test]
    fn classify_handles_newly_supported_languages() {
        for (path, expected) in &[
            ("main.go", Language::Go),
            ("lib.rs", Language::Rust),
            ("App.scala", Language::Scala),
            ("worksheet.sc", Language::Scala),
        ] {
            let li = classify(Path::new(path))
                .unwrap_or_else(|| panic!("classify failed for {path}"));
            assert_eq!(
                li.supported,
                Some(*expected),
                "{path} should map to Language::{expected:?}"
            );
        }
    }

    #[test]
    fn classify_excludes_data_and_prose_from_programming() {
        assert!(matches!(
            classify(Path::new("README.md")).unwrap().kind,
            LangKind::Prose
        ));
        assert!(matches!(
            classify(Path::new("data.json")).unwrap().kind,
            LangKind::Data
        ));
        assert!(matches!(
            classify(Path::new("style.css")).unwrap().kind,
            LangKind::Markup
        ));
    }

    #[test]
    fn dominant_supported_prefers_largest_supported_even_when_unsupported_dominates() {
        // Kotlin dominates by bytes (unsupported by drift) but TypeScript is
        // the largest *supported* language, so TS must win.
        let root = tmp_dir("kotlin-vs-ts");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/big.kt"), vec![b'x'; 10_000]).unwrap();
        fs::write(root.join("src/small.ts"), vec![b'x'; 1_000]).unwrap();
        fs::write(root.join("src/tiny.py"), vec![b'x'; 100]).unwrap();

        let stats = compute_language_stats(&root);
        assert_eq!(stats.dominant_supported, Some(Language::TypeScript));
        // Breakdown ordering: Kotlin first by bytes, then TypeScript, then Python.
        let names: Vec<&str> = stats.breakdown.iter().map(|e| e.language.as_str()).collect();
        assert_eq!(names, vec!["Kotlin", "TypeScript", "Python"]);
        // Percentages sum to ~100.
        let sum: f64 = stats.breakdown.iter().map(|e| e.percent).sum();
        assert!((sum - 100.0).abs() < 1e-6, "percentages should sum to 100, got {sum}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dominant_supported_picks_rust_when_largest_supported() {
        // Rust now ships a parser — a Rust-heavy repo should be profiled
        // by drift directly. (Regression guard for the "Rust unsupported"
        // assumption baked into earlier tests.)
        let root = tmp_dir("rust-dominant");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), vec![b'x'; 5_000]).unwrap();
        fs::write(root.join("src/legacy.py"), vec![b'x'; 1_000]).unwrap();

        let stats = compute_language_stats(&root);
        assert_eq!(stats.dominant_supported, Some(Language::Rust));
        assert_eq!(stats.dominant_supported_name.as_deref(), Some("Rust"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dominant_supported_is_none_when_no_supported_lang_present() {
        // Use Lua + Kotlin — both unsupported.
        let root = tmp_dir("no-supported");
        fs::write(root.join("a.lua"), "print(1)").unwrap();
        fs::write(root.join("b.kt"), "fun main() {}").unwrap();

        let stats = compute_language_stats(&root);
        assert!(stats.dominant_supported.is_none());
        assert!(stats.dominant_supported_name.is_none());
        assert!(stats.dominant_supported_percent.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gitignore_excludes_files_from_byte_counts() {
        // If a 1MB JS bundle in dist/ counted toward the JS share, a
        // TypeScript repo could appear "JavaScript-dominant" because of
        // its own build output. .gitignore must filter that out.
        let root = tmp_dir("gitignore-bytes");
        fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("src/app.ts"), vec![b'x'; 500]).unwrap();
        fs::write(root.join("dist/bundle.js"), vec![b'x'; 1_000_000]).unwrap();

        let stats = compute_language_stats(&root);
        assert_eq!(stats.dominant_supported, Some(Language::TypeScript));
        // bundle.js must NOT appear
        assert!(
            !stats.breakdown.iter().any(|e| e.language == "JavaScript"),
            "dist/bundle.js should be filtered by .gitignore; got {:?}",
            stats.breakdown
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn node_modules_skipped_by_default_even_without_gitignore() {
        let root = tmp_dir("node-modules");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("node_modules/lodash")).unwrap();
        fs::write(root.join("src/app.ts"), vec![b'x'; 500]).unwrap();
        fs::write(
            root.join("node_modules/lodash/index.js"),
            vec![b'x'; 50_000],
        )
        .unwrap();

        let stats = compute_language_stats(&root);
        assert_eq!(stats.dominant_supported, Some(Language::TypeScript));
        assert!(!stats.breakdown.iter().any(|e| e.language == "JavaScript"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_project_yields_empty_stats() {
        let root = tmp_dir("empty");
        let stats = compute_language_stats(&root);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.total_files, 0);
        assert!(stats.breakdown.is_empty());
        assert!(stats.dominant_supported.is_none());

        let _ = fs::remove_dir_all(&root);
    }
}
