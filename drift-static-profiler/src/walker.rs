use crate::Language;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Always-skip directories. Applied regardless of `.gitignore` contents because
/// they're universal noise across languages and dramatically reduce wall time
/// even when a project's `.gitignore` is missing or sloppy.
///
/// Source: cross-referenced GitHub's default `gitignore` templates
/// (github.com/github/gitignore) for Node, Python, Java, Maven, Gradle, Rust,
/// Go, plus common framework conventions.
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
    /// Skip test/spec/mock files and the test-segment directories that
    /// hold them. Off by default — the scan walks tests. When on, both
    /// path segments (e.g. `tests/`, `__tests__/`, `spec/`) AND filename
    /// conventions (e.g. `*.test.ts`, `*_test.go`, `test_*.py`,
    /// `*Test.java`) are filtered. See `is_test_path` for the full rule.
    pub exclude_tests: bool,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            respect_driftignore: true,
            apply_defaults: true,
            skip_hidden: true,
            exclude_tests: false,
        }
    }
}

/// Test-file recognition shared by walker filtering AND roots discovery
/// so the definition of "test code" stays consistent across the two
/// stages. Returns true for paths that are either:
///   - inside a test/spec subdirectory (tests, test, __tests__, spec,
///     specs, __mocks__, testdata, fixtures), OR
///   - have a test-suffix filename per the language's convention:
///       JS/TS  → `*.test.{ts,tsx,js,jsx}`, `*.spec.*`, `*.mock.*`
///       Python → `test_*.py`, `*_test.py`
///       Go     → `*_test.go`
///       Java   → `*Test.java`, `*Tests.java`
///       Scala  → `*Spec.scala`, `*Specs.scala`
///
/// `root` is used to strip the project-root prefix BEFORE checking path
/// segments, so a project rooted at e.g. `tests/fixtures/foo/` is not
/// itself misidentified as test code — only test directories *inside*
/// the analyzed root count.
pub fn is_test_path(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    // Path segments (test/spec/mock/data buckets).
    if rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            s.as_str(),
            "tests" | "test" | "__tests__" | "spec" | "specs" | "__mocks__" | "testdata"
        )
    }) {
        return true;
    }
    // Filename conventions. We check name-as-given (case-sensitive) for
    // Java/Scala suffixes (which depend on PascalCase), and a lowercased
    // copy for the substring-style JS/TS/Python/Go patterns.
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.ends_with("Test.java")
        || name.ends_with("Tests.java")
        || name.ends_with("Spec.scala")
        || name.ends_with("Specs.scala")
    {
        return true;
    }
    let lname = name.to_ascii_lowercase();
    if lname.contains(".test.")
        || lname.contains(".spec.")
        || lname.contains(".mock.")
        || lname.contains("_test.")
        || lname.contains("_spec.")
        || lname.contains("_mock.")
    {
        return true;
    }
    if lname.starts_with("test_") && lname.ends_with(".py") {
        return true;
    }
    if lname.ends_with("_test.go") {
        return true;
    }
    false
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

/// Walk every file under `root` honoring the same ignore semantics as
/// [`discover_source_files_with`], but WITHOUT filtering by language. Returns
/// `(path, byte_len)` per file.
///
/// This is the entry point the linguist-style byte counter uses: it needs to
/// see *all* source-shaped files (including Rust, Go, etc. we don't profile)
/// so the language percentages it computes reflect the whole repo, not just
/// the languages whose tree-sitter parsers we ship.
pub fn walk_files_with(root: &Path, opts: &WalkOpts) -> Vec<(PathBuf, u64)> {
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

    let mut out = Vec::new();
    for entry in wb.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if opts.apply_defaults && hits_default_ignore(path) {
            continue;
        }
        if opts.exclude_tests && is_test_path(path, root) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push((path.to_path_buf(), size));
    }
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
}
