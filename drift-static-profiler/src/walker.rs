use crate::progress::{NullProgress, Progress};
use crate::Language;
use ignore::WalkBuilder;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Always-skip directories. Applied regardless of `.gitignore` contents because
/// they're universal noise across languages and dramatically reduce wall time
/// even when a project's `.gitignore` is missing or sloppy.
///
/// Source: cross-referenced GitHub's default `gitignore` templates
/// (github.com/github/gitignore) for Node, Python, Java, Maven, Gradle, Rust,
/// Go, plus common framework conventions.
///
/// Contract: every name in this list represents code that is **never** the
/// user's source — package caches, build output, editor metadata. If a name
/// belongs here it must NEVER hold project source code by any convention.
/// Names that are "usually" asset/non-source live in
/// [`STATIC_ASSET_DIRS`] instead and are toggleable.
pub const DEFAULT_IGNORE_DIRS: &[&str] = &[
    // VCS metadata
    ".git", ".hg", ".svn",
    // JS / TS dependencies & build output
    "node_modules", "bower_components", "vendor",
    "dist", ".next", ".nuxt", "out", ".cache", ".turbo",
    "coverage", ".nyc_output",
    // Python venvs & caches
    "__pycache__", ".venv", "venv", "env",
    ".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox", ".nox",
    "site-packages",
    // JVM build dirs
    "target", "build", ".gradle",
    // Rust / Go (target also covered)
    // Editor / OS
    ".idea", ".vscode", ".DS_Store",
];

/// "Probably-not-source" directories. On by default but **togglable** via
/// [`WalkOpts::exclude_static_assets`].
///
/// Why this is separate from [`DEFAULT_IGNORE_DIRS`]:
///   - These names *can* legitimately hold source. A Django app's
///     `static/` may also contain hand-written admin JavaScript; a Vue
///     project's `public/` is usually plain HTML but a team may put
///     small scripts there. Marking these "always skip" would silently
///     hide real code on some projects.
///   - The motivating signal is huge minified bundles (`swagger-ui-
///     bundle.js`, vendored `app.min.js`, copied SDK builds). When the
///     analyzer parses these, they dominate the top entries with reach
///     counts in the thousands and bury the user's actual code.
///   - Default ON because the noise case is the common one; advanced
///     users can disable from the desktop Settings UI when they really
///     are trying to inspect a static-asset payload.
pub const STATIC_ASSET_DIRS: &[&str] = &["static", "assets"];

/// Options that control which paths the walker emits.
#[derive(Debug, Clone)]
pub struct WalkOpts {
    /// Read `.gitignore`, `.git/info/exclude`, and the global git excludes
    /// file. Applied EVEN WHEN there is no `.git` directory at the root —
    /// the `ignore` crate's default behavior of "no gitignore outside a git
    /// repo" is unhelpful for our static analysis use case.
    pub respect_gitignore: bool,
    /// Read `.driftignore` files (same syntax as `.gitignore`).
    pub respect_driftignore: bool,
    /// Apply [`DEFAULT_IGNORE_DIRS`] as a hard fallback regardless of any
    /// user-provided ignore files.
    pub apply_defaults: bool,
    /// Skip hidden files / dirs (anything starting with `.`).
    pub skip_hidden: bool,
    /// Skip test/spec/mock files and any directory whose name follows a
    /// test-naming convention. Off by default — the scan walks tests.
    /// When on, files AND directory segments share the same grammar
    /// (see [`is_test_token`]):
    ///
    /// - **Test-marker prefix** — PascalCase `Test<UPPER>...` AND
    ///   camelCase `test<UPPER>...` / `test<DIGIT>...` (`TestRunner/`,
    ///   `TestUserService.java`, `testFixtures/`, `testHelpers/`,
    ///   `testEmptyMeansEmpty`).
    /// - **Test-marker suffix** — `<alnum_lower>Test[s]`,
    ///   `<alnum_lower>Spec[s]`, `<alnum_lower>Mock[s]` — with digit
    ///   boundary support (`MyTests/`, `IntegrationTest.java`,
    ///   `UserMocks/`, `ServiceMock`, `jsr166Test/`, `JUnit5Tests/`,
    ///   bare `Test.java`).
    /// - **Separator-bounded substring** (case-insensitive) — any of
    ///   `test`/`tests`/`spec`/`specs`/`mock`/`mocks`/`testdata`
    ///   bounded by non-alphanumeric chars (or string boundary).
    ///   Covers dot- (`*.test.ts`), underscore- (`test_*.py`,
    ///   `*_test.go`), and dash-separated (`test-*`, `integration-tests/`)
    ///   names, as well as classic buckets like `__tests__/` and
    ///   `__mocks__/`.
    ///
    /// Boundary handling guarantees no false positives on words that
    /// merely contain "test" / "spec" / "mock" mid-word —
    /// `testimony/`, `contest.py`, `Tester.java`, `inspector.ts`,
    /// `mockery.ts` all correctly pass through.
    pub exclude_tests: bool,
    /// Skip directories named in [`STATIC_ASSET_DIRS`] (currently `static`,
    /// `assets`). **Defaults to true** — vendored asset directories on real
    /// projects (e.g. Django `static/`, FastAPI `static/`, generic
    /// `assets/swagger-ui-bundle.js`) routinely contain minified JS that
    /// otherwise dominates the entry-point picker with synthetic
    /// "functions" named `Gk`, `Ek`, etc. Settable to `false` for projects
    /// where these directories legitimately contain hand-written source.
    pub exclude_static_assets: bool,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            respect_driftignore: true,
            apply_defaults: true,
            skip_hidden: true,
            exclude_tests: false,
            exclude_static_assets: true,
        }
    }
}

/// Test-file recognition shared by walker filtering AND roots discovery
/// so the definition of "test code" stays consistent across the two
/// stages. Both checks delegate to the same [`is_test_token`] grammar
/// so directories and filenames cannot drift apart:
///
/// - **directory segments** between `root` (exclusive) and the
///   filename (exclusive) are tested as-is. Catches both the classic
///   buckets (`tests/`, `__tests__/`, `__mocks__/`, `testdata/`,
///   `spec/`) and the longer variants real projects actually use
///   (`integration-tests/`, `test-utils/`, `e2e_tests/`, `MyTests/`,
///   `TestRunner/`).
/// - **filenames** are tested against the part before the final
///   extension (so `foo.test.ts` → `foo.test` still matches via the
///   bounded `test` substring rule). Covers every shape we used to
///   match before — `foo.test.ts`, `*.spec.js`, `*.mock.ts`,
///   `test_foo.py`, `foo_test.go`, `Test<UPPER>...`, `<lower>Test`,
///   `<lower>Tests`, `<lower>Spec`, `<lower>Specs`, etc.
///
/// Boundary rule (the heart of the matcher): we never flag `test` /
/// `spec` / `mock` embedded mid-word. `testimony.ts`, `contesting.py`,
/// `Tester.java`, `inspector.ts`, `mockery.ts` all pass through.
///
/// `root` is stripped BEFORE the segment scan so a project rooted at
/// e.g. `tests/fixtures/foo/` is not itself misidentified — only test
/// directories *inside* the analyzed root count.
pub fn is_test_path(path: &Path, root: &Path) -> bool {
    if has_test_directory_segment(path, root) {
        return true;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    is_test_filename(name)
}

/// True iff any path segment between `root` (exclusive) and the
/// filename (also exclusive) is a test-named directory. Delegates to
/// [`is_test_token`] so directory and filename matching share one
/// grammar.
///
/// The filename itself is intentionally skipped here; it's handled by
/// [`is_test_filename`] so we only have one place that strips the
/// final extension.
fn has_test_directory_segment(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let total = rel.components().count();
    rel.components().take(total.saturating_sub(1)).any(|c| {
        c.as_os_str()
            .to_str()
            .map(is_test_token)
            .unwrap_or(false)
    })
}

/// Decide whether a bare filename (no directory) names a test file.
/// Thin wrapper over [`is_test_token`] — strips the final extension
/// (so `foo.test.ts` → `foo.test`, `MyTest.java` → `MyTest`) and
/// applies the shared grammar. See [`is_test_token`] for the rules.
pub fn is_test_filename(name: &str) -> bool {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    is_test_token(stem)
}

/// Test-marker PREFIX — covers BOTH PascalCase `Test<UPPER>...`
/// (`TestUserService`, `TestRunner`, `TestRepositoryImpl`) AND
/// camelCase `test<UPPER>...` (`testFixtures`, `testHelpers`,
/// `testEmptyMeansEmpty`). The character class `[Tt]` accepts either
/// leading case; the trailing `[A-Z]` is the camelCase / PascalCase
/// boundary that distinguishes a real test marker from words like
/// `Tester` / `Testing` / `testimony` (next char is a lowercase
/// letter, NOT uppercase) which correctly fail.
///
/// Uppercase-only boundary on purpose: a digit boundary here would
/// pull in ambiguous names like `test123` (a generic identifier, not
/// a test marker). The suffix pattern accepts digits BEFORE the
/// marker because `jsr166Test` is unambiguous, but a digit AFTER
/// `test` is too weak a signal.
fn test_prefix_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"^[Tt]est[A-Z]").expect("test-prefix regex compiles"))
}

/// Test-marker SUFFIX — `<alnum_lower>Test[s]` /
/// `<alnum_lower>Spec[s]` / `<alnum_lower>Mock[s]`. Catches
/// `UserTest`, `IntegrationTests`, `MyTests`, `UserMocks`,
/// `ServiceMock`, AND digit-boundary variants like `jsr166Test`,
/// `JUnit5Tests` (the JSR / version-numbered directories real Java
/// repos use). The `[a-z0-9]` boundary is what separates
/// `<word>Test` (match) from `attest` (no match — `t` before `Test`
/// is uppercase via `Test`'s own `T`, not a separator); bare `Test` /
/// `Tests` / `Spec` / `Specs` / `Mock` / `Mocks` also qualify via the
/// `^` alternative.
///
/// Case-sensitive on purpose: `Mocks?` requires the capital `M`, so
/// `Hammock` / `Hammocks` (lowercase `m`) don't false-positive even
/// though they textually end with "mock"/"mocks".
fn test_suffix_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"(?:^|[a-z0-9])(?:Tests?|Specs?|Mocks?)$")
            .expect("test-suffix regex compiles")
    })
}

/// Case-insensitive `test` / `tests` / `spec` / `specs` / `mock` /
/// `mocks` / `testdata` bounded by non-alphanumeric chars (or string
/// boundary). This is the rule that lets longer real-world names
/// match (`integration-tests`, `e2e_tests`, `__tests__`,
/// `mock-server`) while rejecting `testimony`, `contest`, `tester`,
/// `mockery`, `inspector`, `respect`.
fn bounded_test_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[^a-z0-9])(?:tests?|specs?|mocks?|testdata)(?:[^a-z0-9]|$)")
            .expect("bounded-test regex compiles")
    })
}

/// Single source of truth for "does this name look like a test?" —
/// applied to both directory segments and filename stems. Three
/// orthogonal rules; any one firing is enough:
///
///   1. **Test-marker prefix** — see [`test_prefix_pattern`].
///      Camel- or PascalCase `test<UPPER>` / `Test<UPPER>` start.
///   2. **Test-marker suffix** — see [`test_suffix_pattern`].
///      `<alnum_lower>Test[s]` / `Spec[s]` / `Mock[s]` end.
///   3. **Separator-bounded substring** — see [`bounded_test_pattern`].
///      Non-alphanumeric-bounded `test`/`spec`/`mock`/`testdata` token.
///
/// Keeping the three rules as separately-named patterns (rather than
/// a single mega-disjunction) follows Uncle Bob's
/// "intention-revealing names" — each predicate tells you what shape
/// of name it catches without forcing a reader to decode a long
/// alternation.
pub fn is_test_token(name: &str) -> bool {
    test_prefix_pattern().is_match(name)
        || test_suffix_pattern().is_match(name)
        || bounded_test_pattern().is_match(name)
}

/// Convenience wrapper using sensible defaults. Used by the CLI.
pub fn discover_source_files(root: &Path) -> Vec<(PathBuf, Language)> {
    discover_source_files_with(root, &WalkOpts::default())
}

pub fn discover_source_files_with(root: &Path, opts: &WalkOpts) -> Vec<(PathBuf, Language)> {
    walk_files_with(root, opts)
        .into_iter()
        .filter_map(|(p, _)| Language::from_path(&p).map(|l| (p, l)))
        .collect()
}

/// Discover every `.sql` file under `root` respecting the same walker
/// hygiene rules as source discovery (gitignore, driftignore, test-dir
/// filter when `opts.exclude_tests`, default ignore dirs).
///
/// **Why a separate function and not an extra `Language` variant?**
/// `.sql` files don't have callable symbols, callers, callees, or any
/// of the call-graph machinery the per-language tree-sitter pipeline
/// assumes. They're pure-content inputs to the SQL Query Analyzer —
/// orthogonal to the language-dominance filter that `discover_source_files`
/// applies. A `.sql` file inside a Python repo would otherwise be
/// filtered out as "not Python" and never reach the SQL rule engine.
///
/// Returns absolute paths. Case-insensitive extension match (so `*.SQL`
/// on case-preserving filesystems doesn't get missed).
pub fn discover_sql_files(root: &Path) -> Vec<PathBuf> {
    discover_sql_files_with(root, &WalkOpts::default())
}

/// Walker-options-aware variant of [`discover_sql_files`].
pub fn discover_sql_files_with(root: &Path, opts: &WalkOpts) -> Vec<PathBuf> {
    walk_files_with(root, opts)
        .into_iter()
        .filter_map(|(p, _)| {
            let ext = p.extension()?.to_str()?;
            // ASCII-case-insensitive match: `.sql` and `.SQL`.
            if ext.eq_ignore_ascii_case("sql") {
                Some(p)
            } else {
                None
            }
        })
        .collect()
}

/// Walk every file under `root` honoring the same ignore semantics as
/// [`discover_source_files_with`], but WITHOUT filtering by language. Returns
/// `(path, byte_len)` per file.
///
/// Thin wrapper over [`walk_files_classified_with`] that drops the
/// classification columns. Kept for backward compatibility with library
/// consumers and the linguist's standalone `compute_language_stats`
/// path. New orchestration code (api.rs) should call the classified
/// variant directly so the linguist breakdown and source-discovery
/// share a single filesystem walk.
pub fn walk_files_with(root: &Path, opts: &WalkOpts) -> Vec<(PathBuf, u64)> {
    walk_files_classified_with(root, opts, &NullProgress)
        .into_iter()
        .map(|f| (f.path, f.size))
        .collect()
}

/// One entry per file the walker emits, pre-classified with linguist
/// metadata so downstream consumers don't need a second walk to
/// recover the language tag.
///
/// Fields are pub(crate): the type is an internal coordination shape
/// between `walker`, `linguist`, and `api`; library consumers stay on
/// the simpler `Vec<(PathBuf, Language)>` / `Vec<(PathBuf, u64)>` APIs.
#[derive(Debug, Clone)]
pub struct ClassifiedFile {
    pub path: PathBuf,
    pub size: u64,
    /// Linguist-style display name (e.g. "Python", "TypeScript",
    /// "Kotlin"). `None` for files that don't match any known
    /// extension/filename — those still appear in the walk output so
    /// callers can decide how to treat them, but they don't contribute
    /// to the language bar.
    pub(crate) lang_name: Option<&'static str>,
    /// Linguist bucket — only `Programming` files contribute to the
    /// language percentage denominator. `None` mirrors `lang_name`.
    pub(crate) lang_kind: Option<crate::linguist::LangKind>,
    /// Drift's tree-sitter parser variant for this file. Populated only
    /// for files we can actually profile; `None` means the file is
    /// counted in the linguist bar (if it's a known programming language)
    /// but skipped by source discovery.
    pub language: Option<Language>,
}

/// One-pass walk that classifies each file as it's discovered.
///
/// Replaces the legacy two-walk pattern in the orchestrator: the old
/// `compute_language_stats` and `discover_source_files_with` each
/// traversed the filesystem independently — for a large monorepo,
/// that's twice the inode reads and twice the gitignore evaluation.
/// This variant yields all the data both phases need in a single
/// pass, and emits `Progress::walk_progress` checkpoints so the CLI
/// can show "scanning… N files" while it runs.
///
/// Progress checkpoint cadence: every 256 files. Fine-grained enough
/// to feel responsive on slow filesystems, coarse enough that the
/// callback overhead is irrelevant.
pub fn walk_files_classified_with(
    root: &Path,
    opts: &WalkOpts,
    progress: &dyn Progress,
) -> Vec<ClassifiedFile> {
    let mut wb = WalkBuilder::new(root);

    // `standard_filters(true)` is a shortcut that enables:
    //   hidden(true), parents(true), ignore(true),
    //   git_ignore(true), git_global(true), git_exclude(true).
    // We override below where needed.
    wb.standard_filters(true)
        .hidden(opts.skip_hidden)
        .parents(true)
        // CRITICAL: by default, the ignore crate only consults .gitignore when
        // the walked directory sits inside a real git repo. For our purposes
        // (analyzing arbitrary checkouts) we want gitignore semantics to apply
        // always.
        .require_git(false);

    if !opts.respect_gitignore {
        wb.git_ignore(false).git_global(false).git_exclude(false);
    }
    if opts.respect_driftignore {
        wb.add_custom_ignore_filename(".driftignore");
    }

    progress.walk_start();
    let mut out: Vec<ClassifiedFile> = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in wb.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if opts.apply_defaults && hits_default_ignore(path) {
            continue;
        }
        if opts.exclude_static_assets && hits_static_asset_ignore(path) {
            continue;
        }
        if opts.exclude_tests && is_test_path(path, root) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        // Minified-bundle filter — path-independent guard against the
        // "one huge JS bundle hijacks the dominant-language picker"
        // failure mode. STATIC_ASSET_DIRS catches `static/`/`assets/`
        // but real-world projects also ship bundles under
        // `web/public/`, `dist/`, custom subdirs we can't enumerate.
        // The content-shape detector below catches them by signature
        // (avg line length, whitespace ratio) — the same approach
        // JetBrains/SonarQube use. Off when `exclude_static_assets`
        // is off so users analyzing a literal bundle can opt in.
        if opts.exclude_static_assets && is_likely_minified_bundle(path, size) {
            continue;
        }
        // Single dispatch: `classify` already maps the extension to
        // both the linguist name+kind AND the supported `Language` (when
        // we ship a parser for it). Reading `info.supported` here
        // instead of calling `Language::from_path` avoids duplicating
        // the extension→language match in two places.
        let info = crate::linguist::classify(path);
        out.push(ClassifiedFile {
            path: path.to_path_buf(),
            size,
            lang_name: info.map(|i| i.name),
            lang_kind: info.map(|i| i.kind),
            language: info.and_then(|i| i.supported),
        });
        total_bytes += size;
        // Throttled progress: 256-file granularity. Walker is single-
        // threaded today (`ignore::Walk` rather than `WalkParallel`),
        // so this is the only hot loop pushing walk_progress events.
        if out.len() & 0xFF == 0 {
            progress.walk_progress(out.len());
        }
    }
    progress.walk_end(out.len(), total_bytes);
    out
}

fn hits_default_ignore(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| DEFAULT_IGNORE_DIRS.contains(&s))
            .unwrap_or(false)
    })
}

/// True iff any component of `path` matches one of [`STATIC_ASSET_DIRS`]
/// (case-sensitive — every convention this targets uses lowercase). We match
/// at any depth, not just the project root, because vendored static
/// directories appear under sub-apps too (`apps/admin/static/...`,
/// `dashboards/static/admin.js`). Users with edge-case projects where these
/// names legitimately hold source can disable the whole filter via
/// [`WalkOpts::exclude_static_assets`].
fn hits_static_asset_ignore(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| STATIC_ASSET_DIRS.contains(&s))
            .unwrap_or(false)
    })
}

/// Cheap minified-bundle detector. Returns `true` for files whose
/// content shape matches a minified JS/CSS/JSON bundle (very long
/// avg line length AND very low whitespace ratio).
///
/// # Why content-shape, not path
///
/// `STATIC_ASSET_DIRS` (`static/`, `assets/`) is a heuristic guess at
/// *where* bundles live. Real-world projects ship bundles under
/// `web/public/`, `dist/`, custom subdirs, etc. — paths we can't
/// enumerate. The actual failure mode is: one ≥1 MB minified bundle
/// dominates the byte-share linguist computation, drift picks that
/// language as "profiled", and the user's real source is silently
/// excluded.
///
/// JetBrains and SonarQube use the same content-shape recognizer:
/// minified files have ≥500 chars per line on average AND ≤10%
/// whitespace bytes. Real source averages ~80 chars/line and 20-30%
/// whitespace. The signal is unambiguous in the corner cases that
/// matter — it never false-positives on hand-written source unless
/// somebody one-lined their entire file, which would be its own
/// pathology.
///
/// # Why size-gated
///
/// Reading bytes from disk is non-free. We only run this check on
/// files ≥ `MIN_SIZE_FOR_MINIFIED_CHECK` (256 KB). Below that, a
/// "bundle" can't displace the language-share calculation enough to
/// matter (the cumulative threshold for hijacking depends on total
/// repo bytes, but 256 KB is well below practical concern).
fn is_likely_minified_bundle(path: &Path, size: u64) -> bool {
    /// Only consider files large enough that, if minified, they would
    /// realistically pull the dominant-language picker. Below this size,
    /// even a perfect bundle can't displace a normal repo's source.
    const MIN_SIZE_FOR_MINIFIED_CHECK: u64 = 256 * 1024;
    /// Average chars per line. Real source: ~80. Minified: >>500.
    /// 500 is conservative — Webpack/Rollup bundles routinely exceed
    /// 10,000 chars/line.
    const MIN_AVG_LINE_LEN: u64 = 500;
    /// Whitespace byte ratio (0.0..=1.0). Real source: 0.20-0.35.
    /// Minified: <0.10 (whitespace stripped except newlines).
    const MAX_WHITESPACE_RATIO: f64 = 0.10;
    /// How many bytes to sample from the file head. 64 KB is plenty
    /// for the avg-line-length / whitespace-ratio computation to
    /// stabilize. Tiny vs file size doesn't matter — bundle shape
    /// is consistent.
    const SAMPLE_BYTES: usize = 64 * 1024;
    /// Only run on extensions that are commonly bundled. Avoids
    /// inadvertently flagging large-but-legitimate data files we
    /// happen to have parsed (rare; defensive).
    const BUNDLED_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "ts", "css", "json", "html"];

    if size < MIN_SIZE_FOR_MINIFIED_CHECK {
        return false;
    }
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| BUNDLED_EXTENSIONS.iter().any(|x| s.eq_ignore_ascii_case(x)))
        .unwrap_or(false);
    if !ext_ok {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else { return false };
    use std::io::Read;
    let mut buf = vec![0u8; SAMPLE_BYTES.min(size as usize)];
    let n = match (&file).take(buf.len() as u64).read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if n == 0 {
        return false;
    }
    let sample = &buf[..n];
    let newlines = sample.iter().filter(|b| **b == b'\n').count().max(1);
    let avg_line_len = (n as u64) / (newlines as u64);
    if avg_line_len < MIN_AVG_LINE_LEN {
        return false;
    }
    let ws = sample
        .iter()
        .filter(|b| matches!(**b, b' ' | b'\t' | b'\n' | b'\r'))
        .count();
    let ws_ratio = (ws as f64) / (n as f64);
    ws_ratio < MAX_WHITESPACE_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Make a unique temp dir per test under /tmp. Tests run in parallel so
    /// names must be unique. Caller is responsible for cleanup.
    fn tmp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("drift-walker-{label}-{pid}-{n}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("mkdir tmp");
        p
    }

    fn rel(root: &Path, files: &[(PathBuf, Language)]) -> Vec<String> {
        let mut v: Vec<String> = files
            .iter()
            .map(|(p, _)| {
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .display()
                    .to_string()
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn defaults_skip_node_modules_even_without_gitignore() {
        let root = tmp_dir("defaults-node-modules");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("node_modules/lodash")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;").unwrap();
        fs::write(root.join("node_modules/lodash/index.js"), "module.exports = {};").unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert!(names.contains(&"src/app.ts".to_string()));
        assert!(
            !names.iter().any(|n| n.contains("node_modules")),
            "node_modules must be skipped by default; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn defaults_skip_pycache_and_venv() {
        let root = tmp_dir("defaults-python");
        fs::create_dir_all(root.join("app")).unwrap();
        fs::create_dir_all(root.join("app/__pycache__")).unwrap();
        fs::create_dir_all(root.join(".venv/lib/python3.12/site-packages")).unwrap();
        fs::write(root.join("app/main.py"), "x = 1").unwrap();
        fs::write(root.join("app/__pycache__/main.cpython-312.pyc"), "garbage").unwrap();
        fs::write(
            root.join(".venv/lib/python3.12/site-packages/requests.py"),
            "x = 1",
        )
        .unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert_eq!(names, vec!["app/main.py".to_string()]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gitignore_is_respected_even_without_git_dir() {
        let root = tmp_dir("gitignore");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("private")).unwrap();
        fs::write(root.join(".gitignore"), "private/\nsecret.py\n").unwrap();
        fs::write(root.join("src/app.py"), "x = 1").unwrap();
        fs::write(root.join("private/internal.py"), "y = 2").unwrap();
        fs::write(root.join("secret.py"), "z = 3").unwrap();
        // NOTE: no `.git/` directory — the `ignore` crate's default would
        // ignore .gitignore here. require_git(false) fixes that.

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert_eq!(names, vec!["src/app.py".to_string()],
            "private/ and secret.py must be skipped via .gitignore even without a .git dir; got {names:?}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn driftignore_filters_additional_paths() {
        let root = tmp_dir("driftignore");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".driftignore"), "src/legacy.py\n").unwrap();
        fs::write(root.join("src/main.py"), "x = 1").unwrap();
        fs::write(root.join("src/legacy.py"), "x = 1").unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert!(names.contains(&"src/main.py".to_string()));
        assert!(
            !names.contains(&"src/legacy.py".to_string()),
            ".driftignore should drop src/legacy.py; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn opts_can_disable_gitignore() {
        let root = tmp_dir("opts-no-git");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "src/skipped.py\n").unwrap();
        fs::write(root.join("src/main.py"), "x = 1").unwrap();
        fs::write(root.join("src/skipped.py"), "x = 1").unwrap();

        let opts = WalkOpts {
            respect_gitignore: false,
            ..WalkOpts::default()
        };
        let files = discover_source_files_with(&root, &opts);
        let names = rel(&root, &files);
        assert!(
            names.contains(&"src/skipped.py".to_string()),
            "with gitignore disabled, src/skipped.py should reappear; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_gitignore_in_subdir_works() {
        let root = tmp_dir("nested-gitignore");
        fs::create_dir_all(root.join("app/internal")).unwrap();
        fs::write(root.join("app/internal/.gitignore"), "*.py\n").unwrap();
        fs::write(root.join("app/handler.py"), "x = 1").unwrap();
        fs::write(root.join("app/internal/private.py"), "x = 1").unwrap();
        fs::write(root.join("app/internal/keep.ts"), "x = 1").unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert!(names.contains(&"app/handler.py".to_string()));
        assert!(
            !names.contains(&"app/internal/private.py".to_string()),
            "nested .gitignore (*.py) should drop app/internal/private.py; got {names:?}"
        );
        assert!(
            names.contains(&"app/internal/keep.ts".to_string()),
            "nested .gitignore only ignored *.py, not *.ts; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn negation_patterns_in_gitignore_are_honored() {
        let root = tmp_dir("gitignore-negation");
        fs::create_dir_all(root.join("logs")).unwrap();
        // Ignore everything in logs/, but un-ignore important.py
        fs::write(root.join(".gitignore"), "logs/*\n!logs/important.py\n").unwrap();
        fs::write(root.join("logs/scratch.py"), "x = 1").unwrap();
        fs::write(root.join("logs/important.py"), "x = 1").unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert!(
            names.contains(&"logs/important.py".to_string()),
            "negation should un-ignore logs/important.py; got {names:?}"
        );
        assert!(!names.contains(&"logs/scratch.py".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn typescript_repo_with_realistic_gitignore() {
        // Mimic a NestJS-shaped project. We exercise the same machinery the
        // CLI uses on real checkouts: a Node-style .gitignore, build output,
        // installed dependencies, env files, and coverage reports.
        //
        // What MUST be discovered:
        //   src/main.ts, src/app.module.ts
        //   src/users/users.controller.ts, src/users/users.service.ts
        //   test/app.e2e-spec.ts
        //
        // What MUST be filtered:
        //   dist/*           (gitignore)
        //   coverage/*       (gitignore)
        //   node_modules/*   (gitignore AND L1 default)
        //   .env             (gitignore)
        //   *.log            (gitignore)
        //
        // The .gitignore content is verbatim from GitHub's `Node.gitignore`
        // template (truncated to the relevant lines).
        let root = tmp_dir("ts-repo");

        // ── project metadata ──────────────────────────────────────────────
        fs::write(
            root.join(".gitignore"),
            "node_modules/\n\
             dist/\n\
             coverage/\n\
             .env\n\
             *.log\n\
             .DS_Store\n\
             .npm\n",
        )
        .unwrap();
        fs::write(root.join("package.json"), r#"{"name":"orders-svc"}"#).unwrap();
        fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{}}"#).unwrap();
        fs::write(root.join("README.md"), "# orders-svc").unwrap();
        fs::write(root.join(".env"), "DB_URL=postgres://...").unwrap();
        fs::write(root.join("app.log"), "[INFO] started").unwrap();

        // ── real source ───────────────────────────────────────────────────
        fs::create_dir_all(root.join("src/users")).unwrap();
        fs::write(
            root.join("src/main.ts"),
            "import { NestFactory } from '@nestjs/core';\nasync function bootstrap() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/app.module.ts"),
            "import { Module } from '@nestjs/common';\n@Module({})\nexport class AppModule {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.controller.ts"),
            "export class UsersController { create() { return {}; } }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/users/users.service.ts"),
            "export class UsersService { findAll() { return []; } }\n",
        )
        .unwrap();

        // ── tests dir (kept by default — Node gitignore does NOT exclude it) ─
        fs::create_dir_all(root.join("test")).unwrap();
        fs::write(
            root.join("test/app.e2e-spec.ts"),
            "import { Test } from '@nestjs/testing';\ndescribe('App', () => {});\n",
        )
        .unwrap();

        // ── build output (gitignored) ─────────────────────────────────────
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/main.js"), "console.log('hi');\n").unwrap();
        fs::write(root.join("dist/app.module.js"), "module.exports = {};\n").unwrap();

        // ── coverage report (gitignored) ──────────────────────────────────
        fs::create_dir_all(root.join("coverage/lcov-report")).unwrap();
        fs::write(root.join("coverage/lcov-report/index.html"), "<html/>").unwrap();
        fs::write(root.join("coverage/extra.ts"), "// fake source").unwrap();

        // ── installed dependencies (gitignored + L1 default) ──────────────
        fs::create_dir_all(root.join("node_modules/@nestjs/common")).unwrap();
        fs::create_dir_all(root.join("node_modules/typeorm/dist")).unwrap();
        fs::write(
            root.join("node_modules/@nestjs/common/index.d.ts"),
            "export declare const X: number;",
        )
        .unwrap();
        fs::write(
            root.join("node_modules/typeorm/index.js"),
            "module.exports = {};",
        )
        .unwrap();
        fs::write(
            root.join("node_modules/typeorm/dist/repository.ts"),
            "export class Repository {}",
        )
        .unwrap();

        // ── act ───────────────────────────────────────────────────────────
        let files = discover_source_files(&root);
        let names = rel(&root, &files);

        // ── must include ──────────────────────────────────────────────────
        let must_include = [
            "src/main.ts",
            "src/app.module.ts",
            "src/users/users.controller.ts",
            "src/users/users.service.ts",
            "test/app.e2e-spec.ts",
        ];
        for f in must_include {
            assert!(
                names.contains(&f.to_string()),
                "expected {f:?} to be discovered; got {names:?}"
            );
        }

        // ── must exclude ──────────────────────────────────────────────────
        for forbidden in [
            "dist/",
            "coverage/",
            "node_modules/",
        ] {
            assert!(
                !names.iter().any(|n| n.contains(forbidden)),
                "expected nothing matching {forbidden:?}; got {names:?}"
            );
        }
        // .env, app.log, README.md, package.json — none are source languages we
        // recognize anyway, so they're filtered out by Language::from_path
        // regardless of .gitignore. Just sanity-check.
        assert!(!names.iter().any(|n| n.ends_with(".env")));
        assert!(!names.iter().any(|n| n.ends_with(".log")));

        // ── now layer a .driftignore on top to also exclude tests/ ────────
        fs::write(root.join(".driftignore"), "test/\n").unwrap();
        let files2 = discover_source_files(&root);
        let names2 = rel(&root, &files2);
        assert!(
            !names2.iter().any(|n| n.starts_with("test/")),
            ".driftignore should now exclude test/; got {names2:?}"
        );
        // src/ must still be present
        assert!(names2.contains(&"src/main.ts".to_string()));

        // ── final sanity: exactly the 5 src+test files originally ─────────
        // (test/ count: 1 before .driftignore)
        // Using a set for clarity:
        let set: std::collections::HashSet<String> = names.into_iter().collect();
        assert_eq!(set.len(), 5, "expected exactly 5 source files; got {set:?}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn static_assets_dirs_are_excluded_by_default() {
        // Real-world shape: a server project with vendored Swagger UI sitting
        // in `static/` and `assets/`. Real source under `src/` and `app/`.
        // The motivating bug was these vendored bundles appearing as the
        // top entry-point candidates with reach 4000+, drowning out the
        // user's actual code.
        let root = tmp_dir("static-assets-default");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("static")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(root.join("apps/admin/static")).unwrap();
        fs::write(root.join("src/server.py"), "x = 1").unwrap();
        fs::write(root.join("static/swagger-ui-bundle.js"), "var Gk={};").unwrap();
        fs::write(root.join("static/redoc.standalone.js"), "var Ek={};").unwrap();
        fs::write(root.join("assets/app.min.js"), "var ai={};").unwrap();
        fs::write(root.join("apps/admin/static/admin.js"), "var ci={};").unwrap();

        let files = discover_source_files(&root);
        let names = rel(&root, &files);
        assert_eq!(
            names,
            vec!["src/server.py".to_string()],
            "default WalkOpts should exclude static/ and assets/ at any depth; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn static_assets_filter_can_be_disabled() {
        // The escape hatch — users analyzing a project that genuinely keeps
        // source under static/ (rare but real, e.g. Django apps with
        // `static/admin/js/admin.js` as the codebase under analysis) can
        // opt out via the new flag. Settings UI exposes this toggle.
        let root = tmp_dir("static-assets-disabled");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("static")).unwrap();
        fs::write(root.join("src/main.py"), "x = 1").unwrap();
        fs::write(root.join("static/admin.py"), "x = 1").unwrap();

        let opts = WalkOpts {
            exclude_static_assets: false,
            ..WalkOpts::default()
        };
        let files = discover_source_files_with(&root, &opts);
        let names = rel(&root, &files);
        let set: std::collections::HashSet<String> = names.into_iter().collect();
        assert!(
            set.contains("src/main.py") && set.contains("static/admin.py"),
            "with the static-assets filter off, static/admin.py must reappear; got {set:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fixtures_still_discover_their_source_files() {
        // Regression check: the existing four fixtures must still resolve.
        for (fix, expected_lang) in &[
            ("python-fastapi", Language::Python),
            ("java-spring", Language::Java),
            ("typescript-nestjs", Language::TypeScript),
            ("javascript-express", Language::JavaScript),
        ] {
            let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            p.push("tests/fixtures");
            p.push(fix);
            let files = discover_source_files(&p);
            assert!(
                files.iter().any(|(_, l)| l == expected_lang),
                "expected at least one {:?} file in {fix}, got {:?}",
                expected_lang,
                rel(&p, &files)
            );
        }
    }

    // ── is_test_filename — exhaustive grammar tests ────────────────────
    //
    // These are the unit tests for the file-name half of `is_test_path`.
    // We keep them in walker.rs (not integration.rs) so they exercise the
    // helper directly without touching the filesystem — fast, focused,
    // and they pin down the exact grammar above.

    #[test]
    fn is_test_filename_dot_separated_conventions() {
        // The classic JS/TS shape, plus its sibling .spec / .mock.
        for name in [
            "app.test.ts",
            "app.test.tsx",
            "app.test.js",
            "app.spec.js",
            "api.mock.ts",
            "e2e.test.ts",
            "deeply.nested.feature.test.tsx",
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_underscore_separated_conventions() {
        // Python + Go classics. We accept both `test_X` and `X_test` for
        // Python (pytest collects both); Go only blesses `_test.go`.
        for name in [
            "test_utils.py",
            "utils_test.py",
            "test_data_loader.py",
            "util_test.go",
            "user_test.go",
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_dash_separated_conventions() {
        // New in this pass — dash-separated test naming is common in
        // TS/JS/Deno repos but wasn't previously matched.
        for name in [
            "test-helper.ts",
            "helper-test.ts",
            "foo-test-bar.ts",
            "spec-runner.ts",
            "runner-spec.ts",
            "mock-server.ts",
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_pascal_case_suffix_works_across_extensions() {
        // PascalCase `*Test` / `*Tests` / `*Spec` / `*Specs` is no longer
        // tied to `.java` / `.scala` — Kotlin, C#, Rust, etc. all qualify.
        for name in [
            "UserTest.java",
            "UserTests.java",
            "UserTest.kt",
            "UserTests.kt",
            "UserTest.cs",
            "UserTest.rs",
            "UserSpec.scala",
            "UserSpecs.scala",
            "IntegrationTest.java",   // multi-camel still works
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_pascal_case_prefix_test_x() {
        // `Test<UPPER>...` — typical for the "Test*" Java/Kotlin style.
        for name in [
            "TestUserService.java",
            "TestHelper.kt",
            "TestUtils.java",
            "TestRepository.scala",
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_bare_names_count() {
        // Empty prefix / empty suffix is fine — a file literally named
        // `Test.java`, `Spec.ts`, or `test.py` IS a test artifact.
        for name in [
            "Test.java",
            "Tests.kt",
            "Spec.scala",
            "Specs.scala",
            "Spec.ts",
            "Test.ts",
            "test.py",
            "spec.rb",
        ] {
            assert!(is_test_filename(name), "should match: {name:?}");
        }
    }

    #[test]
    fn is_test_filename_rejects_words_that_merely_contain_test() {
        // THE boundary rule is the heart of this matcher. None of these
        // should fire, even though they textually contain "test" /
        // "spec" / "mock".
        for name in [
            "app.py",                  // unrelated
            "users.ts",
            "handler.go",
            "User.java",
            "UserService.scala",
            "testimony.py",            // alnum AFTER `test`
            "testimonial.ts",
            "contest.py",              // alnum BEFORE `test`
            "contesting.ts",
            "protester.go",
            "Tester.java",             // PascalCase: `Test` + LOWERCASE `e` is the boundary fail
            "Testing.java",
            "MyTestUtil.java",         // PascalCase: `MyTest` + UPPERCASE `U` → util, not a test class
            "inspector.ts",            // contains `spec` mid-word
            "respect.ts",
            "mockery.ts",              // `mock` + alnum `e` after
            "smoke.ts",                // contains `mok` not `mock` — safety check
        ] {
            assert!(!is_test_filename(name), "should NOT match: {name:?}");
        }
    }

    // ── is_test_token — directory-segment grammar tests ───────────────
    //
    // is_test_token is the shared predicate behind both filename and
    // directory matching. The filename behavior is already covered by
    // the `is_test_filename_*` suites above (they delegate to
    // is_test_token under the hood). These tests focus on the
    // *directory* shapes that were silently missed before the regex
    // refactor — and pin the boundary behavior that prevents new
    // false positives.

    #[test]
    fn is_test_token_matches_classic_buckets() {
        // The original exact-name allowlist must still match — these
        // were the only directories the pre-refactor code recognized.
        for name in [
            "test",
            "tests",
            "Test",
            "Tests",
            "TEST",
            "TESTS",
            "spec",
            "specs",
            "__tests__",
            "__mocks__",
            "testdata",
        ] {
            assert!(is_test_token(name), "classic bucket should match: {name:?}");
        }
    }

    #[test]
    fn is_test_token_matches_longer_dir_names() {
        // New behavior: longer real-world directory names that contain
        // a bounded `test` / `spec` / `mock` token now match too.
        for name in [
            "integration-tests",
            "integration_tests",
            "unit-tests",
            "e2e_tests",
            "smoke-tests",
            "acceptance.tests",
            "test-utils",
            "test_helpers",
            "mock-server",
            "MyTests",          // PascalCase suffix
            "IntegrationTest",  // PascalCase suffix
            "TestRunner",       // PascalCase prefix
            "TestUtils",        // PascalCase prefix
        ] {
            assert!(is_test_token(name), "long test dir should match: {name:?}");
        }
    }

    #[test]
    fn is_test_token_matches_camel_case_test_prefix() {
        // Real-world bug: `caffeine/src/testFixtures/java/...` and
        // `caffeine/src/jsr166Test/java/...` slipped past the filter
        // because:
        //   - `testFixtures` starts with lowercase `t` (camelCase),
        //     and the old `^Test[A-Z]` only matched capital `Test`.
        //   - `jsr166Test` has digit `6` before `Test`, and the old
        //     `(?:^|[a-z])(?:Tests?|...)$` only matched a lowercase
        //     letter before the marker.
        // Both shapes are extremely common in Java/JUnit codebases
        // (camelCase test source-sets, JSR-versioned test packages).
        for name in [
            "testFixtures",        // Gradle test-source-set convention
            "testHelpers",
            "testIntegration",
            "testE2E",             // capital E after `test` — PascalCase boundary
            "testV2Runner",        // V is the boundary; 2 sits inside the tail
        ] {
            assert!(
                is_test_token(name),
                "camelCase test prefix must match: {name:?}"
            );
        }
    }

    #[test]
    fn is_test_token_matches_digit_boundary_test_suffix() {
        // The `jsr166Test/` directory family — version-numbered test
        // packages from upstream Java projects (Caffeine vendors JSR
        // 166 tests this way).
        for name in [
            "jsr166Test",
            "jsr166Tests",
            "JUnit5Test",
            "JUnit5Tests",
            "Spring5Test",
            "Java11Tests",
            "PostgreSQL9Spec",     // digit before Spec
            "Redis7Mocks",         // digit before Mocks
        ] {
            assert!(
                is_test_token(name),
                "digit-boundary test suffix must match: {name:?}"
            );
        }
    }

    #[test]
    fn is_test_token_matches_pascal_case_mock_suffix() {
        // PascalCase `Mock` / `Mocks` suffix is part of the suffix
        // rule alongside `Test[s]` / `Spec[s]`. Catches the common
        // shape `<Subject>Mock[s]` for hand-written test doubles —
        // `UserMocks/`, `ServiceMock.java`, `HttpClientMocks/`.
        for name in [
            "UserMocks",
            "ServiceMock",
            "HttpClientMocks",
            "RepositoryMock",
            "Mock",                // bare — empty prefix is fine
            "Mocks",
        ] {
            assert!(is_test_token(name), "Mock suffix should match: {name:?}");
        }

        // Case-sensitivity guard: lowercase `mock`/`mocks` does NOT
        // satisfy the PascalCase suffix rule, so common-English words
        // ending in those letters don't false-positive. The bounded
        // rule rejects them too (their lowercase letter before `mock`
        // is alnum), so `is_test_token` returns false overall.
        for name in [
            "Hammock",             // ends with lowercase "mock"
            "Hammocks",            // ends with lowercase "mocks"
            "Schoolmock",          // hypothetical — same shape
        ] {
            assert!(
                !is_test_token(name),
                "lowercase mock/mocks tail must NOT match: {name:?}"
            );
        }
    }

    #[test]
    fn is_test_token_rejects_words_that_merely_contain_test() {
        // The boundary invariant: names that have `test` / `spec` /
        // `mock` embedded inside another word must NOT match. This is
        // what protects unrelated code from being filtered.
        for name in [
            "src",
            "lib",
            "app",
            "vendor",
            "static",
            "assets",
            "testimony",       // alnum after `test`
            "testimonial",
            "contest",         // alnum before `test`
            "contesting",
            "protester",
            "tester",          // bare alnum `e` after
            "Tester",          // PascalCase prefix fails: `e` not uppercase
            "Testing",         // same
            "MyTestUtil",      // bounded fails: `y` before; suffix fails: ends in `Util`
            "inspector",       // contains `spec` mid-word
            "respect",
            "mockery",         // `mock` + alnum `e` after
            "smoke",
            "test123",         // bounded fails: `1` after — must be non-alnum
        ] {
            assert!(
                !is_test_token(name),
                "non-test name must NOT match: {name:?}"
            );
        }
    }

    #[test]
    fn exclude_tests_filters_caffeine_style_test_source_sets() {
        // Reproduces the exact bug seen in the desktop UI: a Caffeine-
        // shaped repo with `testFixtures/` (Gradle test source set,
        // lowercase camelCase) and `jsr166Test/` (digit-bounded test
        // package). Before the prefix/suffix boundary extension, files
        // under both directories survived the filter and their methods
        // (e.g. `provideArguments`, `testEmptyMeansEmpty`) polluted
        // the entry-roots picker.
        let root = tmp_dir("exclude-tests-caffeine");
        fs::create_dir_all(root.join("caffeine/src/main/java")).unwrap();
        fs::create_dir_all(root.join("caffeine/src/testFixtures/java")).unwrap();
        fs::create_dir_all(root.join("caffeine/src/jsr166Test/java")).unwrap();
        fs::write(
            root.join("caffeine/src/main/java/Cache.java"),
            "class Cache {}",
        )
        .unwrap();
        fs::write(
            root.join("caffeine/src/testFixtures/java/Fixtures.java"),
            "class Fixtures { Object provideArguments() { return null; } }",
        )
        .unwrap();
        fs::write(
            root.join("caffeine/src/jsr166Test/java/MapTest.java"),
            "class MapTest { void testEmptyMeansEmpty() {} }",
        )
        .unwrap();

        let opts = WalkOpts {
            exclude_tests: true,
            ..WalkOpts::default()
        };
        let files = discover_source_files_with(&root, &opts);
        let names = rel(&root, &files);
        assert_eq!(
            names,
            vec!["caffeine/src/main/java/Cache.java".to_string()],
            "testFixtures/ (camelCase) and jsr166Test/ (digit-boundary) \
             must both be filtered when exclude_tests is on; got {names:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exclude_tests_filters_substring_named_directories() {
        // End-to-end: with exclude_tests on, the walker drops files
        // under directories whose NAMES contain a bounded test token,
        // even when the directory isn't in the original exact-name
        // allowlist. `testimony/` is the negative control — its name
        // textually contains "test" but fails the boundary rule, so
        // its contents stay.
        let root = tmp_dir("exclude-tests-substring-dirs");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("integration-tests")).unwrap();
        fs::create_dir_all(root.join("MyTests")).unwrap();
        fs::create_dir_all(root.join("TestRunner")).unwrap();
        fs::create_dir_all(root.join("apps/admin/e2e_tests")).unwrap();
        fs::create_dir_all(root.join("testimony")).unwrap();
        fs::write(root.join("src/main.py"), "x = 1").unwrap();
        fs::write(root.join("integration-tests/e2e.py"), "x = 1").unwrap();
        fs::write(root.join("MyTests/runner.py"), "x = 1").unwrap();
        fs::write(root.join("TestRunner/run.py"), "x = 1").unwrap();
        fs::write(root.join("apps/admin/e2e_tests/login.py"), "x = 1").unwrap();
        fs::write(root.join("testimony/affidavit.py"), "x = 1").unwrap();

        let opts = WalkOpts {
            exclude_tests: true,
            ..WalkOpts::default()
        };
        let files = discover_source_files_with(&root, &opts);
        let names = rel(&root, &files);

        assert!(
            names.contains(&"src/main.py".to_string()),
            "src/main.py must survive; got {names:?}"
        );
        assert!(
            names.contains(&"testimony/affidavit.py".to_string()),
            "testimony/ is not a test directory (alnum after 'test'); got {names:?}"
        );
        for forbidden in ["integration-tests", "MyTests", "TestRunner", "e2e_tests"] {
            assert!(
                !names.iter().any(|n| n.contains(forbidden)),
                "test-shaped directory {forbidden:?} should be excluded; got {names:?}"
            );
        }

        let _ = fs::remove_dir_all(&root);
    }

    // ── Minified-bundle detector ──────────────────────────────────

    #[test]
    fn minified_detector_skips_files_under_size_threshold() {
        // Even if content shape matches, files <256 KB don't trigger
        // (they can't displace the dominant-language picker enough to
        // matter, and reading bytes from disk on every file would slow
        // down monorepo walks).
        let root = tmp_dir("minified-small");
        let p = root.join("tiny.js");
        // 64 KB of pure-minified shape — but below MIN_SIZE.
        let content = "x".repeat(64 * 1024);
        fs::write(&p, &content).unwrap();
        assert!(
            !is_likely_minified_bundle(&p, content.len() as u64),
            "small files should never be flagged minified"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn minified_detector_skips_extensions_outside_bundled_set() {
        // A 1 MB .md or .csv file with long lines is not a bundle.
        let root = tmp_dir("minified-ext");
        let p = root.join("data.csv");
        let line = "a".repeat(2000) + "\n";
        let content = line.repeat(200); // ~400 KB, every line 2000 chars
        fs::write(&p, &content).unwrap();
        assert!(
            !is_likely_minified_bundle(&p, content.len() as u64),
            ".csv must not be flagged regardless of line shape"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn minified_detector_flags_typical_bundle_shape() {
        // The signature: large file, >500 chars/line avg, <10% whitespace,
        // bundled extension. This is the exact failure mode that hijacked
        // the user's db-mcp scan (one 9.5 MB main.test.js in web/public/).
        let root = tmp_dir("minified-pos");
        let p = root.join("main.bundle.js");
        // 1024 chars, just 1 space → near-zero whitespace ratio.
        let chunk = format!(
            "var{}={};",
            "x".repeat(508),
            "1".repeat(509),
        );
        // Repeat to ~512 KB, all on near-zero lines (one newline per chunk).
        let mut content = String::with_capacity(512 * 1024);
        while content.len() < 512 * 1024 {
            content.push_str(&chunk);
            content.push('\n');
        }
        fs::write(&p, &content).unwrap();
        assert!(
            is_likely_minified_bundle(&p, content.len() as u64),
            "1 MB JS file with 1000-char lines and ~0% whitespace must be flagged",
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn minified_detector_does_not_flag_real_source() {
        // Hand-written source: short lines, plenty of whitespace.
        let root = tmp_dir("minified-neg");
        let p = root.join("real.js");
        let line = "  const x = computeValue(args, options);\n";
        // Repeat to ~512 KB. Each line ~40 chars, ~25% whitespace.
        let mut content = String::with_capacity(512 * 1024);
        while content.len() < 512 * 1024 {
            content.push_str(line);
        }
        fs::write(&p, &content).unwrap();
        assert!(
            !is_likely_minified_bundle(&p, content.len() as u64),
            "real source with 40-char lines and indentation must NOT be flagged",
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn walker_skips_minified_bundle_so_language_picker_holds() {
        // End-to-end: a repo with 10 small TS files + 1 huge minified JS
        // bundle. Default walker (exclude_static_assets=true) must skip
        // the bundle so the TS files dominate. Without this filter, the
        // bundle's bytes hijack the language picker — exactly the
        // db-mcp `web/public/main.test.js` failure mode.
        let root = tmp_dir("walker-minified-bundle");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("web/public")).unwrap();
        // 10 small TS files
        for i in 0..10 {
            fs::write(
                root.join(format!("src/m{i}.ts")),
                "export const x = 1;\nexport function f() { return x; }\n",
            )
            .unwrap();
        }
        // 1 minified bundle, not in static/assets — sneaks past path filter.
        let chunk = format!("var{}={};", "x".repeat(508), "1".repeat(509));
        let mut content = String::with_capacity(512 * 1024);
        while content.len() < 512 * 1024 {
            content.push_str(&chunk);
            content.push('\n');
        }
        fs::write(root.join("web/public/main.bundle.js"), &content).unwrap();

        let opts = WalkOpts::default();
        let files = discover_source_files_with(&root, &opts);
        let names = rel(&root, &files);
        assert!(
            !names.iter().any(|n| n.contains("main.bundle.js")),
            "minified bundle must be filtered; got {names:?}",
        );
        assert!(
            names.iter().filter(|n| n.ends_with(".ts")).count() == 10,
            "all 10 TS files must survive; got {names:?}",
        );
        let _ = fs::remove_dir_all(&root);
    }
}
