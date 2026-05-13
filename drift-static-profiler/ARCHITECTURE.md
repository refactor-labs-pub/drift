# ARCHITECTURE — drift-static-profiler

A complete, file-by-file walkthrough of the analyzer (Rust) and the viewer
(TypeScript / React). Every "findings", "immediate fix", and "refactor
candidate" you see in the UI comes from the algorithms documented in
§4 and §5. The TypeScript layer is purely presentational — it never
re-derives findings, only renders them.

> Scope: this document covers the algorithms inside `drift-static-profiler/`.
> The supporting Python research scripts under
> [src/research_classefiers+categories/](src/research_classefiers+categories/)
> are catalog generators feeding [categories.rs](src/categories.rs); they
> are not part of the runtime path.

---

## 1. What this is

`drift-static-profiler` is a **static** call-tree analyzer that reads a
source tree and emits a single JSON `Report` describing:

- Every defined symbol (functions, methods, classes) across the
  dominant supported language (Python, Java, TypeScript, JavaScript,
  Go, Rust, Scala).
- The call graph between those symbols.
- A per-root call tree (rooted either at user-supplied `--entry` names
  or at auto-discovered entry points).
- **Findings** — structured, severity-scored issues attached to
  individual symbols (N+1 risk, blocking I/O in async, recursion, etc.).
- **Immediate fixes** — the quick-win subset of findings
  (high-severity, low-effort).
- **Refactor candidates** — symbols with multiple findings, large-effort
  findings, or god-function bodies.
- **Roots overview** — per-entry-point rollups (subtree share,
  categories, findings by severity, callers, first callees).
- Entry-point declarations discovered from Dockerfile / docker-compose /
  package.json / pyproject.toml / Cargo.toml / deno.json, each wired
  back to an in-graph symbol.

The viewer (Vite + React SPA in [viewer/](viewer/)) consumes this JSON
and renders it. All "is this a smell?" decisions are made in Rust;
the viewer only filters, sorts, and links.

---

## 2. Pipeline (data flow, top to bottom)

```
                   ┌──────────────────────────────────────────────────┐
   CLI / library   │  api::analyze / api::analyze_roots               │
                   │  (the only public entry points)                  │
                   └────────────────────┬─────────────────────────────┘
                                        │
   ┌─────────────────┐                  │
   │  walker.rs      │  walk + filter source files
   │  linguist.rs    │  pick the dominant supported language
   └────────┬────────┘
            │
   ┌────────▼────────┐
   │  parser.rs      │  load tree-sitter grammars + queries
   │  tags.rs        │  per-file extract: Symbol + Reference + Import
   │  metrics.rs     │  per-symbol body metrics (loc, complexity,
   └────────┬────────┘  nesting, params, async, loop/await byte ranges)
            │
   ┌────────▼────────┐
   │  graph.rs       │  CallGraph: edges (callee/caller),
   │  categories.rs  │  external_calls (Db/Net/Io/...), call_site_count,
   │                 │  is_recursive (SCC), pagerank
   └────────┬────────┘
            │
   ┌────────▼────────┐
   │  roots.rs       │  pick entries: explicit (analyze) or
   │                 │  auto-discover (analyze_roots)
   └────────┬────────┘
            │
   ┌────────▼────────┐
   │  tree.rs        │  for each entry: build CallTreeNode tree
   │  insights.rs    │  + run per-node detectors INLINE in build_inner
   └────────┬────────┘
            │
   ┌────────▼────────┐
   │  manifest.rs    │  parse package.json / pyproject.toml /
   │  docker.rs      │  Cargo.toml / Dockerfile / compose →
   │                 │  EntryDecl, then match_entries wires each to a
   │                 │  symbol; label_call_tree_entries badges roots.
   └────────┬────────┘
            │
   ┌────────▼────────┐
   │  report.rs      │  Report::build:
   │  insights.rs    │   1. compute pagerank p90
   │                 │   2. attach_recursive_findings
   │                 │   3. attach_missing_caching_findings
   │                 │   4. attach_log_amplification_findings
   │                 │   5. attach_hot_log_findings
   │                 │   6. attach_hot_zones (no-op today)
   │                 │   7. bump_severities_by_impact  ←── ALWAYS LAST
   │                 │   8. Summary::build (rollups)
   └────────┬────────┘
            │
            ▼
   `{ schema_version, mode, generator, summary, entries[] }`  JSON
            │
            ▼
   ┌────────────────────┐
   │  viewer/ (React)   │  fetch JSON → render pages + tabs
   └────────────────────┘
```

The order of attach passes in step (1–7) is load-bearing —
`bump_severities_by_impact` runs **last** so it sees every finding
the earlier passes produced. See [report.rs:130-141](src/report.rs#L130-L141)
and §4.3 below.

---

## 3. Rust core, file by file

### lib.rs — module declarations and shared types

[lib.rs](src/lib.rs) declares 15 sibling modules and re-exports the
public entry points: `analyze`, `analyze_roots`, `AnalyzeOptions`,
`AnalyzeOutcome`, `compute_language_stats`, `LanguageStats`,
`discover_roots`, `DiscoverOpts`, `DiscoveredRoot`. It also defines
the cross-module value types every layer consumes:

- `Language` enum (7 variants), with `Language::from_path` mapping by
  extension ([lib.rs:35-49](src/lib.rs#L35-L49)).
- `SymbolKind` — `Function | Method | Class`.
- `Symbol` — the canonical per-definition struct with byte/line span,
  parent class, and the Phase A metrics (`loc`, `complexity`,
  `nesting_depth`, `parameter_count`, `is_async`) plus the Phase D
  byte-range inputs (`loop_ranges`, `await_ranges`) used by the
  N+1 / blocking-in-async detectors ([lib.rs:58-77](src/lib.rs#L58-L77)).
- `Reference` — a call site (`name`, optional `receiver`, file, line,
  `byte_offset`, and the `in_symbol` it was lexically inside).
- `ImportRecord` — `(local_name, module_path, imported_name, line)`,
  consumed by `categories::classify` Tier B.
- `Binding` — scope-local name → type-name + extends list. Currently
  collected but not used by the live classifier (forward-compat).
- `FileTags` — one per parsed file: `(file, language, symbols,
  references, imports, bindings)`.

### main.rs — the CLI

[main.rs](src/main.rs) is a `clap`-driven dispatcher with four
subcommands:

- `analyze <path> --entry <name>...` — explicit roots; ASCII tree or
  JSON ([main.rs:18-38](src/main.rs#L18-L38)).
- `tags <path>` — dump every extracted Symbol / Reference for
  debugging.
- `scan <path> [--entry ...] [--name ...]` — same as `analyze` but
  writes the JSON into `viewer/public/fixtures/<name>.json` so it
  appears in the live viewer ([main.rs:47-77](src/main.rs#L47-L77)).
- `analyze-root <path>` — auto-discover roots via `roots.rs`, build a
  tree per root, write JSON. Flags: `--min-reach`, `--max-roots`,
  `--include-tests`, `--include-private`, `--include-accessors`,
  `--no-tests`, `--no-accessors` ([main.rs:88-133](src/main.rs#L88-L133)).
  Note the subtlety at [main.rs:343-353](src/main.rs#L343-L353): if
  `--no-tests` is passed without `--include-tests`, the walker drops
  test files entirely so they cannot show up as roots, callees, or
  dead code.
- `diff <baseline> <current>` — load two reports and run
  [diff.rs](src/diff.rs); exits non-zero on regressions unless
  `--no-fail` is set.

`print_language_summary` ([main.rs:447](src/main.rs#L447)) emits the
GitHub-Linguist-style language bar to stderr (kept off stdout so
`--json` output stays clean for piping).

### api.rs — orchestration

[api.rs](src/api.rs) is the single point at which "analyze a project"
is implemented. Both `analyze` and `analyze_roots` start with the
shared `build_graph_context` ([api.rs:76](src/api.rs#L76)):

1. `compute_language_stats(root)` — language bar over the WHOLE repo
   (no test exclusion), so percentages reflect the repo, not the
   filtered subset.
2. `discover_source_files_with(root, &WalkOpts { exclude_tests })` —
   walk and filter to the dominant supported language only. If
   `opts.exclude_tests = true`, test files don't enter the graph.
3. For each file, run `tags::extract_tags(file, lang)` → `FileTags`.
4. `CallGraph::build(&all_tags)`.
5. `docker::collect` + `manifest::collect` produce `EntryDecl`s;
   `docker::match_entries` wires each to a symbol where possible.

`analyze(root, entries, opts)` ([api.rs:151](src/api.rs#L151)) then
resolves each `--entry name` against `graph.find_entry_points(name)`
(string match), collects matched `SymbolId`s, builds one
`CallTreeNode` per id via `TreeBuilder::build`, and runs
`docker::label_call_tree_entries` to badge entry points. The result is
handed to `Report::build`.

`analyze_roots(root, discover, opts)` ([api.rs:192](src/api.rs#L192))
is identical except the entry ids come from
`roots::discover_roots(&graph, root, discover)` instead of user input.

The output type is `AnalyzeOutcome` ([api.rs:46-62](src/api.rs#L46-L62))
— wraps `Report` plus `unresolved_entries`, `language_stats`,
`profiled_language`, and `discovered_roots`. The viewer only sees
`report`; the rest is CLI diagnostics.

### walker.rs — source-file discovery + test-path detection

[walker.rs](src/walker.rs) exposes `discover_source_files`,
`discover_source_files_with`, `walk_files_with`, and `is_test_path`.
The walk uses `ignore::WalkBuilder` ([walker.rs:254](src/walker.rs#L254))
with `standard_filters(true)` enabling `hidden`, `parents`, `ignore`,
`git_ignore`, `git_global`, `git_exclude` simultaneously. Then:

1. `parents(true)` — always honor `.gitignore` higher than `root`.
2. `require_git(false)` — load-bearing: drift analyzes arbitrary
   trees, so `.gitignore` is honored even without a `.git/` directory
   ([walker.rs:268](src/walker.rs#L268)).
3. If `respect_driftignore = true`, register a custom `.driftignore`
   (gitignore grammar, layered on top).
4. After the walker yields a file, two hard post-filters apply:
   `hits_default_ignore(path)` (rejects any path component matching
   `DEFAULT_IGNORE_DIRS`: `node_modules`, `__pycache__`, `target`,
   `dist`, `.venv`, `.idea`, etc.; full list at [walker.rs:12](src/walker.rs#L12))
   and, when `opts.exclude_tests` is set, `is_test_path`.

`is_test_path` ([walker.rs:103](src/walker.rs#L103)) is two disjoint
rules ORed:

- **Directory-segment rule** — case-insensitive match of any
  parent-directory name (not the filename itself) against
  `test | tests | __tests__ | spec | specs | __mocks__ | testdata`.
- **Filename grammar** ([walker.rs:156](src/walker.rs#L156)) — three rules,
  any one fires: (a) PascalCase prefix `Test<UPPER>...`, (b) PascalCase
  suffix `Test|Tests|Spec|Specs` (boundary-checked so `attest` is
  not a match), (c) separator-bounded substring `test|spec|mock` over
  the whole filename, so `foo.test.ts`, `bar_spec.py`, `*_test.go` all
  hit but `testimony`, `tester`, `contesting`, `mockery` do not.

`discover_source_files_with` filters the typed walk through
`Language::from_path` so only files matching one of the seven shipped
tree-sitter grammars survive.

### linguist.rs — GitHub-Linguist-style language share

[linguist.rs](src/linguist.rs) produces a `LanguageStats` with a
sorted `breakdown: Vec<LanguageBreakdownEntry>` (the percent bar) and
a `dominant_supported: Option<Language>` (the language drift will
profile).

`compute_language_stats_with` ([linguist.rs:182](src/linguist.rs#L182)):

1. Walk every file via `walker::walk_files_with` with the same
   ignore pipeline. Yields `(path, byte_len)`.
2. Classify each path via `classify` — filename match first
   (`Dockerfile`, `Makefile`, `Rakefile`, `Gemfile`, etc.), else
   lowercase-extension match against a static table at
   [linguist.rs:81](src/linguist.rs#L81). Each entry is
   `LangInfo { name, kind: Programming|Markup|Data|Prose, supported }`.
3. **Programming-only denominator** ([linguist.rs:199](src/linguist.rs#L199))
   — Markup/Data/Prose files are dropped from `total_bytes` and never
   appear in `breakdown`. Mirrors GitHub's repo-page bar.
4. Bucket by language name, compute `percent = (bytes / total) * 100`,
   sort breakdown desc by bytes (ties broken alphabetically).
5. `dominant_supported` = the largest bucket whose `supported` is
   `Some(...)`. Tiebreak by language name ([linguist.rs:235](src/linguist.rs#L235))
   — explicit so a 50/50 TS/JS repo is deterministic.

Empty trees → all-zero stats and `None`. Largest-language Kotlin still
gets profiled as Python if Kotlin has no shipped parser but Python
has the next-largest share. Shebang/content-classifier heuristics
(real Linguist behavior) are deliberately omitted.

### parser.rs — tree-sitter language registry

[parser.rs](src/parser.rs) exposes `language_for(Language) ->
tree_sitter::Language` and `tags_query(Language) -> &'static str`.
Grammars are statically linked (not loaded from disk) via the
`tree_sitter_*` crates. Queries are embedded as string constants, one
per supported language ([parser.rs:37](src/parser.rs#L37) onward).

Capture-name convention ([parser.rs:28](src/parser.rs#L28)), shared
across all 7 queries:

- `@def.name` — identifier of the definition
- `@def.function | @def.method | @def.class` — defining node; tag
  determines `SymbolKind`
- `@ref.name` — function/method being called
- `@ref.receiver` — receiver before `.` or `::`, optional
- `@ref.call` — the whole call site (its start byte/row become
  `Reference.byte_offset` and `Reference.line`)
- `@import.module` — module path string
- `@import.name` — imported identifier; `None` for whole-module imports
- `@import.alias` — local binding when aliased

This lets `tags::extract_tags_from_source` consume every match
generically — no per-language branches inside the loop.

Per-language quirks worth knowing:

- **Python**: `(call function: identifier)` AND `(call function:
  attribute object: ... attribute: identifier)` patterns capture
  receiver-style calls. `import x`, `import x as y`, `from m import
  x [as y]` all covered.
- **Java**: `(method_invocation object: ... name: ...)` AND
  `(method_invocation !object name: ...)`. `object_creation_expression`
  is captured so `new Foo()` becomes a call.
- **TS/JS**: ES module import shapes covered; JS additionally
  recognizes CommonJS `require(...)` via a `#eq?` predicate
  ([parser.rs:174](src/parser.rs#L174)).
- **Go**: no `class`; methods on receivers (`func (r *T) M()`) have
  parent set only later by byte-range containment in `tags.rs`. Import
  paths come quoted; the quotes are stripped at
  [tags.rs:127](src/tags.rs#L127).
- **Rust**: `impl_item` captured as `@def.class` with the type id as
  the name, so byte-range containment puts methods under that type.
  Four call shapes: bare `identifier`, `scoped_identifier` (`T::foo`),
  `field_expression` (`obj.method()`), and turbofish (`foo::<T>()`,
  `obj.collect::<Vec<_>>()`). Macros (`println!()`) are deliberately
  not captured. `use a::b::{c,d}` list-form is also not enumerated.
- **Scala**: `class_definition`, `object_definition`, `trait_definition`
  all `@def.class`. Infix and paren-less calls deliberately not
  captured.

### tags.rs — top-level per-file extractor

[tags.rs](src/tags.rs) ties parser + queries + metrics into one
`extract_tags(path, lang) -> FileTags`. Algorithm
([tags.rs:36](src/tags.rs#L36)):

1. Read source, parse, run the captured query.
2. For each `QueryMatch`, accumulate the captured nodes by capture
   name, then:
   - **Symbol**: if def captures resolved, build a `Symbol`. For
     functions/methods call `metrics::compute(node, source, lang)`;
     for classes use `SymbolMetrics::default()`.
   - **Reference**: if `@ref.call` resolved, push a `Reference`. The
     receiver string is normalized via `rightmost_id`
     ([tags.rs:226](src/tags.rs#L226)) — `prisma.user` → `user`.
   - **Import**: trim quotes around the module path
     ([tags.rs:127](src/tags.rs#L127)); local-name falls back through
     `alias → name → last segment of module_path`.
3. **Synthetic `<module>` symbol** ([tags.rs:179](src/tags.rs#L179)) —
   if any reference's byte_offset is outside every collected symbol's
   range, insert a single `<module>` symbol spanning the whole file
   (Function kind, complexity=1). This catches Python
   `if __name__ == "__main__":`, TS/JS top-level code, etc.,
   without which those references would be dropped from the graph.
   The angle-bracket name is unambiguous (no real identifier contains
   `<`); `insights::is_synthetic_symbol` ([insights.rs:518](src/insights.rs#L518))
   uses this property to skip detectors that would produce false
   positives on the synthetic node.
4. `resolve_containment` ([tags.rs:237](src/tags.rs#L237)) — for each
   symbol, find the smallest *other* symbol whose byte range strictly
   contains it; that becomes the parent (used to attach methods to
   classes). The synthetic `<module>` is explicitly excluded from
   being chosen as a parent. For each reference, find the smallest
   symbol whose byte range contains the reference's `byte_offset`
   and set `in_symbol`.

Edge cases: Python `__init__` is not specially skipped (treated as a
method). Go has no parent for methods until containment resolves
because there is no enclosing AST node for the receiver type. Rust
`impl T { fn m() {} }` puts `m.parent = Some("T")` via the
captured `impl_item`.

### metrics.rs — per-symbol body metrics

[metrics.rs](src/metrics.rs) produces a `SymbolMetrics { loc,
complexity, nesting_depth, parameter_count, is_async, loop_ranges,
await_ranges }` ([metrics.rs:4](src/metrics.rs#L4)) for every
function/method. These feed both the viewer's metric tiles and the
findings detectors (see §4).

`compute` ([metrics.rs:22](src/metrics.rs#L22)):

1. `body = node.child_by_field_name("body")`, fallback to the whole
   def node.
2. `loc = count_lines_in_range(source, body.start_byte, body.end_byte)`
   — newline count in the slice + 1.
3. `complexity = 1 + count_decision_points(body, lang)`.
4. `nesting_depth = max_nesting(body, lang)`.
5. `parameter_count` counted from the `parameters` field on the def
   node.
6. `is_async` — leading-text heuristic on the first 64 bytes of the
   def.
7. `loop_ranges` + `await_ranges` collected in a single pre-order walk.

**What counts as +1 cyclomatic complexity** ([metrics.rs:66](src/metrics.rs#L66)):
the common AST kinds across all 7 languages — `if_statement`,
`while_statement`, `for_statement`, `for_in_statement`,
`for_of_statement`, `do_statement`, `elif_clause`, `case_clause`,
`switch_case`, `switch_label`, `catch_clause`, `except_clause`,
`conditional_expression`, `ternary_expression`, `enhanced_for_statement`.
Go-specific: `expression_switch_statement`, `type_switch_statement`,
`type_case`, `default_case`, `communication_case` (select). Rust:
`if_expression`, `if_let_expression`, `while_expression`,
`while_let_expression`, `for_expression`, `loop_expression`,
`match_expression`, `match_arm`. Scala: `case_block`. Logical
short-circuit operators (`&&`, `||`, `??`) add +1 too — Python via
`boolean_operator`, others via inspecting the `operator` field on
`binary_expression`. `??` (nullish-coalescing) is included for TS/JS.

**Nesting depth** counts function/closure/lambda definitions as well
as control flow, so a function inside a function adds depth even
without `if` ([metrics.rs:146](src/metrics.rs#L146)).

**`is_async`** detection ([metrics.rs:244](src/metrics.rs#L244)) is
language-specific. Python: starts with `async def` or `async\n`.
TS/JS: any visibility modifier + `async`. Java: always `false`
(CompletableFuture analysis deferred). Go: always `false` (concurrency
is at call site, not on the definition). Rust: strips `pub` and
`pub(crate)|pub(super)`, checks for `async fn` / `async unsafe`.
Scala: `false` (async is library-level, not a keyword).

**`loop_ranges` and `await_ranges` are BYTE OFFSETS, not lines** —
this is the input to N+1 and blocking-in-async detection:
`external_call.in_loop = true` iff the call's `byte_offset` falls
inside any range in the symbol's `loop_ranges`. Same for
`in_await`. Loop kinds include comprehensions, generators, Rust
expression-loops (`for_expression`, `loop_expression`, etc.). Await
kinds: `await_expression`, `await`.

### categories.rs — external-call classifier (3 tiers)

[categories.rs](src/categories.rs) returns
`Option<Classification { category, tier, evidence }>` for a call site,
where `Category` is one of `Db | Network | Io | Cache | Queue | Log |
Compute` ([categories.rs:8](src/categories.rs#L8)). Inputs: call
name, optional receiver, and the file's `ImportRecord` list.

Rule data is compiled in via `include_str!` and cached in `OnceLock`:

- Per-language catalogs +
  [research_classefiers+categories/module_overrides.json](src/research_classefiers+categories/module_overrides.json),
  loaded and sorted longest-prefix-first ([categories.rs:211](src/categories.rs#L211))
  so `django.db` matches before `django` and
  `org.springframework.data` before `org.springframework`.
- [receiver_patterns.json](src/research_classefiers+categories/receiver_patterns.json)
  — Tier C data.
- [unambiguous_methods.json](src/research_classefiers+categories/unambiguous_methods.json)
  — Tier D data.

**Tier order** ([categories.rs:73](src/categories.rs#L73)) — first
hit wins:

1. **Tier B — ImportedModule** ([categories.rs:89](src/categories.rs#L89)).
   Requires a receiver. For each `ImportRecord`, two match shapes:
   *direct binding* (`imp.local_name == receiver`) or *crate/package
   root* (first segment of `imp.module_path` split on `.`/`/`/`::`
   equals the receiver). On match, classify the module path against
   the catalog — equality, `prefix.`, `prefix/`, or `prefix::` are
   all accepted.
2. **Tier C — ReceiverPattern** ([categories.rs:110](src/categories.rs#L110)).
   Requires a receiver. Lowercases it and looks up
   `receiver_patterns()` — `session`/`db`/`conn`/`tx` → Db,
   `axios`/`httpclient`/`fetch` → Network, `logger`/`log` → Log, etc.
3. **Tier D — MethodSignature** ([categories.rs:121](src/categories.rs#L121)).
   No receiver required. Exact, case-sensitive lookup against
   `unambiguous_methods()`. Deliberately tight: `save`, `add`, `find`,
   `get`, `delete`, `update` are NOT in the table (too ambiguous).
   Inclusions: `executeQuery` → Db, `hgetall` → Cache, `insertOne` →
   Db, `basicPublish` → Queue, etc.

Why this ordering: Tier B knows the actual library and is most
precise; Tier C is name-based heuristic but recall-friendly; Tier D
is a last resort and the catalog is intentionally conservative.

The receiver passed to `classify` has already been reduced to its
rightmost identifier by `tags::rightmost_id` — so `prisma.user.create()`
arrives as `receiver = "user"`, NOT `"prisma.user"`. This limits
Tier C to one identifier; the regression test at
[categories.rs:362](src/categories.rs#L362) documents this.

### graph.rs — building the call graph

[graph.rs](src/graph.rs) produces a `CallGraph` ([graph.rs:33](src/graph.rs#L33))
with `symbols`, `by_name`, `edges` (callees), `callers`,
`external_calls`, `call_site_count`, `is_recursive`, `pagerank`.

`SymbolId::for_symbol` ([graph.rs:11](src/graph.rs#L11)) is
`"<file>::<parent_or_empty>::<name>"`. Two same-named functions in
different files therefore have distinct ids. Two same-named methods
on different classes also distinct.

**Reference resolution** ([graph.rs:69-126](src/graph.rs#L69-L126)) is
purely name-based — there is NO receiver-class lookup or
binding-aware dispatch:

1. **Locate the source symbol**: scan the file's symbols for one
   whose name equals `r.in_symbol` AND whose byte range contains
   `r.byte_offset`.
2. **Resolve targets by name** via `by_name.get(&r.name)` — across
   the ENTIRE project. Multiple matches all become targets. Self
   edges (`target == source`) are filtered.
3. **External fallback**: if name lookup produced no targets,
   `categories::classify` runs against the call. The branch also
   fires when the only resolution was to the source symbol itself
   (so `this.repo.save()` inside our own `save` still classifies
   externally).
4. **Phase D in-loop/in-await tagging** ([graph.rs:103](src/graph.rs#L103)):
   the SOURCE symbol's `loop_ranges` / `await_ranges` are checked
   against `r.byte_offset`. The resulting `ExternalCall` gets
   `in_loop` and `in_await` booleans here — exactly the inputs the
   findings detectors need.
5. Duplicate external calls within a symbol are suppressed by
   `(name, line)` pair.

**call_site_count** is the count of textual callsites that resolve to
a symbol — incremented for EVERY target a reference resolves to.

**Recursion** via Tarjan SCC ([graph.rs:165](src/graph.rs#L165)): every
node in an SCC of size > 1 is marked recursive. Size-1 self-loops are
not recursive because self-edges were filtered at edge-add time.

**PageRank** ([graph.rs:180](src/graph.rs#L180)) is
`petgraph::algo::page_rank(g, α=0.85, 100 iterations)`. Edge weights
are unit (call frequency is not weighted).

Edge cases worth flagging: class instantiations don't add call edges
unless the class name happens to match a function/method name in
`by_name`. PageRank doesn't weight by callsite count. Module-level
references all bind to the synthetic `<module>` symbol.

### tree.rs — building per-root call trees + invoking detectors

[tree.rs](src/tree.rs) turns a chosen entry `SymbolId` into a
`CallTreeNode` tree. Build is depth-bounded (`max_depth: 12` default)
and cycle-aware ([tree.rs:84-92](src/tree.rs#L84-L92)).

**Accessor filter** ([tree.rs:94](src/tree.rs#L94)) — when
`skip_accessors` is on, names `getX | setX | isX` (length ≥ 4,
followed by an uppercase letter) are pruned from children.

`build_inner` ([tree.rs:122-261](src/tree.rs#L122-L261)) does a single
DFS with a global `seen: HashSet<SymbolId>` (NOT a path stack — once
visited, never re-expanded anywhere in the tree). For each node:

1. Look up the symbol; defensive `None` return if missing.
2. `is_cycle = seen.contains(id); seen.insert(id)`. First visit gets
   full expansion; second visit anywhere becomes a `truncated_reason
   = "cycle"` stub.
3. Build the file-relative path.
4. `externals = graph.externals_of(id).to_vec()`. `pick_self_category`
   ([tree.rs:284](src/tree.rs#L284)) picks the first external category
   in fixed priority order `[Db, Network, Io, Cache, Queue, Log]`.
5. **Per-node findings detection (Phase E)** — THIS is where the
   detector is invoked ([tree.rs:145-148](src/tree.rs#L145-L148)):

   ```rust
   let ctx = insights::Ctx::default();
   let findings = insights::collect_node_findings(sym, &externals, &ctx);
   let n_plus_one_risk     = insights::has_kind(&findings, FindingKind::NPlusOne);
   let blocking_in_async   = insights::has_kind(&findings, FindingKind::BlockingInAsync);
   ```

   The legacy Phase D booleans on `CallTreeNode` are now *derived* from
   `findings` ([tree.rs:60](src/tree.rs#L60)).
6. Callers list mapped from `graph.callers_of(id)`.
7. Per-node graph metrics pulled from `call_site_count`, `is_recursive`,
   `pagerank`.
8. Construct the node with `subtree_size = 1`, empty
   `categories_reached`, percentages 0.0 (filled by Phase C below).
9. Termination paths: `is_cycle → truncated_reason "cycle"`;
   `depth >= max_depth → truncated_reason "max-depth"`. In both cases
   `tally_self` fills `categories_reached` from this node alone (no
   children).
10. Recurse into callees via `graph.callees(id)`, with optional
    accessor skip. Each child built at `depth + 1`.
11. **Aggregation pass**: `subtree_size += child.subtree_size` per
    child; `categories_reached` adds +1 for `category_self`, +1 per
    `external_calls` entry, plus the merged child maps.

After all trees are built, `compute_percentages`
([tree.rs:264](src/tree.rs#L264)) walks each tree to set
`percent_total = (size / root_size) * 100` and `percent_parent =
(size / parent_size) * 100`.

### roots.rs — auto-discovering entry-point candidates

[roots.rs](src/roots.rs) implements `discover_roots(&CallGraph, &Path,
&DiscoverOpts) -> Vec<DiscoveredRoot>` ([roots.rs:78](src/roots.rs#L78)).
Default opts: `min_reach: 2`, `skip_tests: true`, `skip_private: true`,
`skip_accessors: true`, `max_roots: 200`.

Selection pipeline ([roots.rs:84-150](src/roots.rs#L84-L150)):

1. Drop `SymbolKind::Class`.
2. **Real in-degree == 0** — the candidate's `callers` filtered to
   exclude synthetic `<module>` callers (via
   `insights::is_synthetic_symbol`). Only real-caller count must be 0.
   Critical: this lets module-level-invoked symbols (TS bottom-level
   calls, Python `if __name__ == "__main__":`-invoked entry) still
   qualify as roots.
3. Apply `skip_accessors`, `skip_private` (`name.starts_with('_')`),
   `skip_tests` (delegates to `walker::is_test_path`).
4. Compute `reach = reachable_count(graph, id)` —
   ([roots.rs:152](src/roots.rs#L152)) DFS-via-stack with dedupe.
5. Keep only those with `reach >= min_reach`.

Ranking: `sort_by(|a,b| b.reach.cmp(&a.reach).then_with(name))` then
`truncate(max_roots)`.

Note: "no real caller" matches dead code as well as true roots; the
ranking by reach floats dead leaves to the bottom. Decorator-registered
handlers (FastAPI `@router.post`, Spring `@RequestMapping`) typically
have in-degree 0 since the registrations are data, not calls — they
get picked up correctly.

### manifest.rs — language-manifest entry discovery

[manifest.rs](src/manifest.rs) walks the project tree and emits one
`EntryDecl` per discovered entry in `package.json`, `deno.json[c]`,
`pyproject.toml`, and `Cargo.toml`. The matcher in `docker.rs` then
wires each `EntryDecl` to a `SymbolId` where possible.

- **`package.json`** ([manifest.rs:83](src/manifest.rs#L83)): emits
  `PackageJsonMain`, `PackageJsonModule`, `PackageJsonBin` (both
  string and `{cmd: path}` object forms), and one `PackageJsonScript`
  per `scripts.*` (argv = whitespace-split).
- **`deno.json[c]`** ([manifest.rs:140](src/manifest.rs#L140)): strips
  `//` and `/* */` comments (naïve, no string-state tracking — works
  in practice for task values), emits one `DenoTask` per `tasks.<name>`.
- **`pyproject.toml`** ([manifest.rs:205](src/manifest.rs#L205)):
  reads `[project.scripts]` (PEP 621) and `[tool.poetry.scripts]`
  (Poetry). Crucially, `argv = vec![target]` keeps the
  `"pkg.module:func"` token whole; the docker matcher's Pass 3 splits
  on `:` later.
- **`Cargo.toml`** ([manifest.rs:251](src/manifest.rs#L251)): iterates
  `[[bin]]` tables. `argv` is `path` if given, else
  `["src/bin/<name>.rs"]` by Cargo convention. The implicit
  `src/main.rs` binary is NOT emitted (auto-root discovery already
  finds its `main` symbol).

`line` is always `1` because serde_json/yaml/toml drop line spans.
Read or parse errors silently return `Vec::new()` — the file is
simply omitted.

### docker.rs — container entry discovery + entry matching

[docker.rs](src/docker.rs) does three things: discover container
files, parse them into `EntryDecl`s, and match every `EntryDecl`
(both container and manifest) to an in-graph `SymbolId`. The matcher
is generic — manifest entries pass through the same `match_entries`
pipeline.

**Dockerfile parsing** ([docker.rs:194](src/docker.rs#L194)) uses
tree-sitter-containerfile. Walks top-level children tracking `workdir`
(threaded into subsequent CMD/ENTRYPOINT emissions). `WORKDIR`
captures `instruction_arg_text`; `CMD` / `ENTRYPOINT` emit one
`EntryDecl` each with argv from `extract_instruction_argv`
([docker.rs:281](src/docker.rs#L281)):

- **JSON-array form** `["python","app.py"]` — depth-first scan for
  `json_string_array` / `string_array`; tokens are
  `json_string` / `double_quoted_string` children, quote-stripped.
- **Shell form** `python app.py` — body text minus the leading
  keyword token, then `split_whitespace` and `strip_quotes`.
  Deliberately NOT POSIX-parsed — only goal is to find file-shaped
  tokens.

**docker-compose parsing** ([docker.rs:375](src/docker.rs#L375)) uses
`serde_yaml`. For each `services.<name>`, emit `ComposeCommand`
and/or `ComposeEntrypoint` from `command` / `entrypoint`. `yaml_argv`
([docker.rs:430](src/docker.rs#L430)) accepts both string and
sequence shapes.

**Matcher — `match_one`** ([docker.rs:475-510](src/docker.rs#L475-L510)),
three passes:

- **Pass 1 — argv contains a parsed file**: for each `tok` in argv,
  run `pick_entry_symbol_for_filename(tok, all_tags, graph)`. Token
  must contain `.` or `/` (rejects bare words like `pip`). Strip
  leading `./`. Find every `FileTags` whose path `ends_with(tok)`;
  break ties by max symbol count. Then `pick_entry_symbol_in_file`
  picks a name from `[main, bootstrap, app, start, run, serve,
  listen, create_app]`, falling back to the first non-class symbol
  with no in-graph callers. Confidence: `Exact`.
- **Pass 2 — `python -m mod`**: detect `python|python3` + `-m mod`,
  call `resolve_python_module(mod, None)` which tries the candidates
  `pkg/mod.py`, `pkg/mod/__main__.py`, `pkg/mod/__init__.py`,
  `mod.py`. Confidence: `Likely`.
- **Pass 3 — pyproject-style `pkg.mod:func`**: single-token argv
  containing `:`. Splits and calls `resolve_python_module(mod,
  Some(func))`. Exact match if a symbol named `func` exists in the
  resolved file.

**`label_call_tree_entries`** ([docker.rs:671](src/docker.rs#L671))
appends human labels to `CallTreeNode.entry_labels` for any root
whose `id` matches a matched `EntryDecl`. Label format:
`"Dockerfile CMD"`, `"compose:<svc> command"`,
`"package.json:scripts.<name>"`, `"pyproject:scripts.<name>"`, etc.

### insights.rs — findings detectors and rollups (§4 and §5 details)

[insights.rs](src/insights.rs) defines `Finding`, `Severity`, `Effort`,
`FindingKind`, the per-node detector entry point
`collect_node_findings`, the post-build attach passes, and the rollup
helpers `collect_immediate_fixes`, `collect_refactor_candidates`,
`collect_roots_overview`, `collect_findings_top`,
`collect_findings_by_kind`.

The detectors themselves are documented in detail in §4 below; the
rollups in §5. This subsection just orients you to the file's
structure:

- Types: `Severity` (Low/Medium/High), `Effort` (Trivial/Small/
  Medium/Large) with `rank()`, `FindingKind` (11 variants),
  `Evidence`, `Finding`, `FindingTopRef`, `RootOverview`,
  `CallerSummary`, `CalleeSummary`, `RefactorCandidate`,
  `ImmediateFix` ([insights.rs:24-385](src/insights.rs#L24-L385)).
- Per-node entry: `collect_node_findings(sym, externals, ctx)`
  ([insights.rs:498](src/insights.rs#L498)) — bails on synthetic
  `<module>` and otherwise runs the four per-node detectors.
- Post-build passes: `attach_recursive_findings`,
  `attach_missing_caching_findings`, `attach_log_amplification_findings`,
  `attach_hot_log_findings`, `attach_hot_zones` (no-op today),
  `bump_severities_by_impact` — invoked from `Report::build` in that
  order ([insights.rs:894-1165](src/insights.rs#L894-L1165)).
- Rollup helpers used by `Summary::build`.

### report.rs — `Report` assembly and `Summary` rollups

[report.rs](src/report.rs) exposes the top-level `Report { schema_version,
mode, generator, summary, entries }` and `Summary` with every aggregate
field (top callers/callees, hot_paths, dead_code, pagerank_top,
recursive_symbols, language_breakdown, **findings_by_kind**,
**findings_top**, **roots_overview**, **immediate_fixes**,
**refactor_candidates**, entry_declarations).

`Report::build` ([report.rs:116](src/report.rs#L116)) is the
finding-passes scheduler:

```rust
let pagerank_p90 = insights::compute_pagerank_p90(graph.pagerank.values().copied());
insights::attach_recursive_findings(&mut entries);
insights::attach_missing_caching_findings(&mut entries);
insights::attach_log_amplification_findings(&mut entries, pagerank_p90);
insights::attach_hot_log_findings(&mut entries, pagerank_p90);
insights::attach_hot_zones(&mut entries, pagerank_p90);
insights::bump_severities_by_impact(&mut entries, pagerank_p90); // LAST
```

Order matters: `bump_severities_by_impact` must run after every other
pass so it sees every finding (otherwise findings stay at base
severity regardless of where they sit in the graph).

`Summary::build` ([report.rs:158](src/report.rs#L158)) aggregates:

- `languages` — sorted set of profiled languages.
- `categories` — sums `categories_reached` across all entries; every
  `Category::ALL` is represented with 0 if missing for stable UI.
- `top_callers` / `top_callees` — top-10 by `callers.len()` /
  `edges.len()`.
- `hot_paths` — walk each tree, collect chains ending at a node with
  `category_self` or any external call, sort by depth desc, keep 10.
- `dead_code` — non-class symbols with empty `callers` and NOT in
  the entry set.
- `pagerank_top` / `recursive_symbols` — straight rollups from the
  graph maps.
- **`findings_by_kind`** = `insights::collect_findings_by_kind`.
- **`findings_top`** = `insights::collect_findings_top(entries, 50)`.
- **`roots_overview`** = `insights::collect_roots_overview(entries)`.
- **`immediate_fixes`** = `insights::collect_immediate_fixes(entries, 50)`.
- **`refactor_candidates`** = `insights::collect_refactor_candidates(entries, 30)`.

The schema is documented separately in
[schema/profile.schema.json](schema/profile.schema.json).

### diff.rs — baseline-vs-current report diffing

[diff.rs](src/diff.rs) compares two JSON reports and classifies
deltas as `regressions` or `improvements`. Entries are matched by
`"{parent_class_or_empty}::{name}"` (robust to file moves, not to
renames).

For each matched pair ([diff.rs:56-123](src/diff.rs#L56-L123)):

- `category_deltas` — union the `categories_reached` maps, compute
  `cur - base` per key, drop zeros.
- `subtree_size_delta`, `complexity_delta_total`.
- `new_smells` / `fixed_smells` — currently only the legacy Phase D
  booleans `n_plus_one_risk` and `blocking_in_async`. The structured
  `findings` list is NOT yet diffed.

Classification:

- Positive category delta → regression `kind: "category_<cat>"`.
- Negative → improvement.
- Each new smell → regression `kind: "smell"`, delta 1.
- Each fixed smell → improvement.
- Positive `complexity_delta_total` → regression. (Negative does NOT
  emit an improvement — asymmetric, possibly an oversight.)

Symbol-level adds/removes ([diff.rs:125](src/diff.rs#L125)) come from
the symmetric difference of every `node.id.0` set across both reports
(walked recursively across all entries).

CLI usage: `drift-static-profiler diff base.json cur.json [--json]
[--no-fail]`. Exits non-zero on regressions unless `--no-fail`.

---

## 4. The findings detection algorithm — deep dive

This section is the answer to "where do the findings on a Scan Report
come from?". Every Finding flows through three phases:

```
   tree::build_inner (per node, while building the tree)
        │
        ▼  insights::collect_node_findings
        ├── detect_n_plus_one
        ├── detect_blocking_in_async
        ├── detect_noisy_log_in_loop
        └── detect_expensive_compute
                │
                ▼  CallTreeNode.findings = [Finding...]
                │
   Report::build (post-build, in this exact order)
        │
        ├── attach_recursive_findings           (read graph SCC info)
        ├── attach_missing_caching_findings     (cross-tree heuristic)
        ├── attach_log_amplification_findings   (hot-path × log volume)
        ├── attach_hot_log_findings             (bump NoisyLog on hot path)
        ├── attach_hot_zones                    (no-op currently)
        └── bump_severities_by_impact   ←── ALWAYS LAST
                                            (percent_total, pagerank,
                                            call_site_count fan-in)
                │
                ▼
   Summary::build collects the rollups.
```

### 4.1 Severity, Effort, Confidence, FindingKind

Defined in [insights.rs:24-104](src/insights.rs#L24-L104).

- `Severity` — `Low | Medium | High`. Sortable rank
  ([insights.rs:1201](src/insights.rs#L1201)): low=0, medium=1, high=2.
- `Effort` — `Trivial (< ~10 min) | Small (~30 min) | Medium (~half
  day) | Large (≥ a day)`. Rank: 0..=3. Documented inline at
  [insights.rs:32-49](src/insights.rs#L32-L49).
- `confidence` — `f64` in [0..1]. Higher = more certain.
- `FindingKind` — 11 variants (snake-case JSON):
  `n_plus_one`, `blocking_in_async`, `recursive`, `smelly_loop`,
  `noisy_log`, `outdated_package`, `memory_explosion`, `hot_zone`,
  `expensive_compute`, `missing_caching`, `log_amplification`.
  Not every kind has a live detector yet (`smelly_loop`,
  `outdated_package`, `memory_explosion`, `hot_zone` are reserved).
- Each `Finding` carries `kind, severity, effort, confidence, line,
  message, evidence: Vec<Evidence>, remediation: Option<String>`.

The `Evidence` shape is `{ call: String, line: usize, category:
Option<Category> }` ([insights.rs:106](src/insights.rs#L106)). For
call-bearing findings, `call` is `"receiver.name"` or just `"name"`.

### 4.2 Per-node detectors (called from `tree::build_inner`)

Called once per non-synthetic symbol by
`collect_node_findings(sym, externals, ctx)`
([insights.rs:498](src/insights.rs#L498)). Synthetic `<module>` is
skipped: its `loc`/`complexity` are file-wide proxies that would
falsely fire `expensive_compute`.

**4.2.1 `detect_n_plus_one`** ([insights.rs:745](src/insights.rs#L745))

Inputs: the symbol and its `&[ExternalCall]`.

Algorithm:

1. Collect offenders: external calls where `in_loop && is_method_call(name)
   && category in {Db, Cache}`. (`is_method_call` excludes PascalCase
   names — class instantiations like `new HttpClient()` are not
   queries.)
2. If empty, return no finding.
3. Confidence = max over offenders of:
   - `ImportedModule` (Tier B) → 0.95
   - `ReceiverPattern` (Tier C) → 0.80
   - `MethodSignature` (Tier D) → 0.65
4. Anchor `line` at the first offender's `line` (call site, not
   symbol-start), so the viewer's deep-link lands on the offending
   line.
5. Emit ONE finding grouping all offenders in this symbol, with each
   call line going into `evidence`. Default `severity = High`,
   `effort = Small` (replace loop with batch API).
6. Remediation copy points at language-specific batch APIs
   (`bulk_save_objects`, `bulk_create`, `TypeORM save([...])`).

**Where `in_loop` came from**: `graph.rs` set it by checking whether
the call's `byte_offset` (recorded by `tags.rs` from `@ref.call`)
falls inside any of the source symbol's `loop_ranges` (recorded by
`metrics.rs::collect_loop_and_await_ranges`).

**4.2.2 `detect_blocking_in_async`** ([insights.rs:652](src/insights.rs#L652))

Only fires when `sym.is_async == true`.

Algorithm:

1. Offenders: external calls where `!in_await && is_method_call(name)
   && category in {Db, Network, Io}`. The `!in_await` predicate is
   the heart of "I made it async but I'm not awaiting".
2. If empty, return.
3. Confidence = max over offenders by tier (same scale as N+1).
4. Anchor `line` at the first offender.
5. ONE finding grouping all offenders; evidence per offender. Base
   severity `High`, effort `Trivial` (swap to async client + add
   `await`).
6. Remediation: pick the async client (`httpx.AsyncClient`,
   `aiomysql`, etc.) and `await` each call.

**4.2.3 `detect_noisy_log_in_loop`** ([insights.rs:604](src/insights.rs#L604))

Algorithm:

1. Offenders: external calls where `in_loop && is_method_call(name)
   && category == Log`.
2. If empty, return.
3. ONE finding grouping all offenders, evidence per call. Base
   severity `Medium`, effort `Trivial` (move log out of loop or
   demote level), confidence 0.85.
4. `attach_hot_log_findings` later promotes `Medium → High` if the
   symbol sits on a hot path (pagerank ≥ p90) — see §4.3.

**4.2.4 `detect_expensive_compute`** ([insights.rs:531](src/insights.rs#L531))

The "this symbol is a heavyweight" detector. Inputs: just the
`Symbol`.

Algorithm:

1. Trigger if ANY of:
   - `complexity >= 10` (Sonar `cognitive complexity` default)
   - `loc >= 80` (god-function candidate)
   - `nesting_depth >= 4`
2. Base severity:
   - `complexity >= 15 || loc >= 150` → `High`
   - `complexity >= 10 || loc >= 80` → `Medium`
   - else `Low` (nesting-only case)
3. Effort:
   - `loc >= 150 || nesting >= 5` → `Large`
   - `loc >= 80 || complexity >= 15` → `Medium`
   - else `Small`
4. Build a reason string ("complexity 12 (≥10: high), 95 lines of
   code, nesting depth 4 (≥4)") so the viewer doesn't synthesize copy.
5. Confidence is **deliberately lower** (0.70) than data-bearing
   findings — complexity is a smell, not always a bug.

**Why `call_site_count` is NOT a trigger here**: it is the IMPACT
multiplier, applied later by `bump_severities_by_impact`. Keeping
detection and severity separate matches pprof's design.

### 4.3 Post-build attach passes (called from `Report::build`)

Order in [report.rs:130-141](src/report.rs#L130-L141):

**4.3.1 `attach_recursive_findings`** ([insights.rs:1125](src/insights.rs#L1125))

Walks every tree. For each node with `is_recursive = true` (set by
graph.rs via Tarjan SCC), pushes a `Recursive` finding (severity
Medium, effort Medium, confidence 1.0). Synthetic `<module>` is
explicitly skipped. Remediation: "Confirm termination invariants; if
recursion depth scales with input size, consider an iterative
equivalent."

**4.3.2 `attach_missing_caching_findings`** ([insights.rs:981](src/insights.rs#L981))

The "memoize candidate" heuristic. Walks every tree; for each node:

- Qualifies iff ALL of:
  - `call_site_count >= 5`
  - `complexity >= 5 || loc >= 20`
  - NOT `is_recursive`
  - NOT a constructor (PascalCase first letter)
  - `looks_pure(node)` — no external call in `{Db, Network, Io,
    Queue}`. **Note Log and Cache are not in this exclude set** —
    logged-but-otherwise-pure code is still a candidate.
  - Not already carrying a `MissingCaching` finding (idempotent).
- Emits one finding: severity `Medium` (becomes `High` on hot path
  via `bump_severities_by_impact`), effort `Small`, confidence
  **0.55** (deliberately low — we cannot prove purity statically).
- Remediation lists language-specific options
  (`functools.lru_cache`, Caffeine, Map memo, the `cached` crate).

**4.3.3 `attach_log_amplification_findings`** ([insights.rs:1055](src/insights.rs#L1055))

The "logs scale with traffic" detector. Walks every tree; for each
node:

- Count log calls = `external_calls.filter(category == Log).count()`.
- "On hot path" = `pagerank >= p90` OR `call_site_count >= 10`.
- Qualifies iff `log_count >= 3 && on_hot_path` and not already
  carrying this finding.
- Emit one finding with evidence sliced to the first 5 log calls.
  Severity `Medium`, effort `Trivial`, confidence 0.80.
- Remediation: demote to DEBUG, aggregate per-batch, or rate-limit
  via sampler.

Distinct from `NoisyLog`: `NoisyLog` is in-loop, this is hot-path.

**4.3.4 `attach_hot_log_findings`** ([insights.rs:947](src/insights.rs#L947))

For every node with `pagerank >= p90 && p90 > 0`, find any
`NoisyLog` finding with severity `Medium` and promote it to `High`.
Other severities and other kinds untouched. p90 ≤ 0 → no-op (empty
graph case).

**4.3.5 `attach_hot_zones`** ([insights.rs:894](src/insights.rs#L894))

Currently a no-op (Step 11 placeholder). When implemented it will
push `HotZone` findings onto nodes that satisfy a hot-zone criterion
(pagerank top-percentile × multiple findings). The
`bump_severities_by_impact` pass deliberately skips `HotZone`
findings ([insights.rs:923](src/insights.rs#L923)) because bumping a
hot-zone finding by its own hot-zone signal would double-count.

**4.3.6 `bump_severities_by_impact`** ([insights.rs:911](src/insights.rs#L911))

Always runs LAST. For every finding on every node (except `HotZone`),
the severity is bumped by impact signals on the *node* itself.

```rust
boost = (percent_total >= 20.0) as u8
      + (pagerank >= p90 && p90 > 0.0) as u8
      + (call_site_count >= 10) as u8;
match (base, boost) {
    (Low, 3)      => High,
    (Low, 2)      => Medium,
    (Medium, 2|3) => High,
    (s, _)        => s,
}
```

Skipped entirely when `p90 <= 0.0`. The promotion is monotonic: a
finding's severity can only go up, never down.

`compute_pagerank_p90` ([insights.rs:871](src/insights.rs#L871))
sorts the pagerank values ascending and indexes at
`floor(N * 0.90).min(N - 1)`. Empty → 0.0 (so the bump rule
degrades to no-op).

### 4.4 Summary rollups produced from `findings`

After all attach passes, `Summary::build` collects ([report.rs:341-346](src/report.rs#L341-L346)):

- `findings_by_kind: BTreeMap<String, usize>` — counts per kind
  across every node in every tree.
- `findings_top: Vec<FindingTopRef>` — top 50, sorted by severity
  desc. Each row: `(node_id, kind, severity, line)`. The viewer
  resolves `node_id` via its node index.
- `roots_overview: Vec<RootOverview>` — per-entry rollup
  ([insights.rs:389](src/insights.rs#L389)). Each row holds:
  `node_id`, `name`, `file`, `line`, `parent_class`, `kind`,
  `subtree_size`, `percent_of_all_roots`, `categories_reached`,
  `findings_by_severity` (BTreeMap<"high"|"medium"|"low", usize>),
  `findings_total`, `callers` (real callers of this root — empty for
  true entry points), `first_callees` (first 5 children, capped to
  keep JSON small).

Roots overview sorts by `subtree_size` desc — same ordering the
viewer's Roots tab uses.

---

## 5. The immediate fixes algorithm

Implemented in `insights::collect_immediate_fixes`
([insights.rs:331](src/insights.rs#L331)). This is the "what should I
do RIGHT NOW?" list, modeled on SonarQube's 5-min / 10-min
remediation lanes.

```rust
pub fn collect_immediate_fixes(entries: &[CallTreeNode], cap: usize) -> Vec<ImmediateFix> {
    let mut out = Vec::new();
    fn walk(node: &CallTreeNode, out: &mut Vec<ImmediateFix>) {
        for f in &node.findings {
            let is_immediate = !matches!(f.severity, Severity::Low)
                && matches!(f.effort, Effort::Trivial | Effort::Small);
            if is_immediate {
                out.push(ImmediateFix {
                    node_id: node.id.0.clone(),
                    name: node.name.clone(),
                    file: node.file.clone(),
                    line: f.line,           // call-site, not symbol-start
                    parent_class: node.parent_class.clone(),
                    kind: f.kind,
                    severity: f.severity,
                    effort: f.effort,
                    message: f.message.clone(),
                });
            }
        }
        for c in &node.children { walk(c, out); }
    }
    for e in entries { walk(e, &mut out); }
    out.sort_by(|a, b|
        severity_rank(b.severity).cmp(&severity_rank(a.severity))
            .then_with(|| a.effort.rank().cmp(&b.effort.rank()))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    );
    out.truncate(cap);
    out
}
```

**Definition of "immediate"**:
`severity >= Medium && effort <= Small`.

**Sort order**: `severity DESC, effort ASC, file ASC, line ASC`.
So a High/Trivial fix comes before a Medium/Small, which comes
before a High/Medium (which is excluded entirely — it's a refactor
candidate).

**Cap**: 50 (passed by `Summary::build`).

**Concrete examples** the rules pick up given the detectors in §4.2:

- `BlockingInAsync` (High, Trivial) — always immediate.
- `N+1` (High, Small) — always immediate.
- `NoisyLog` (Medium, Trivial) — immediate.
- `LogAmplification` (Medium, Trivial) — immediate.
- `MissingCaching` (Medium, Small) — immediate (unless dropped to Low
  somehow — currently can't be dropped).
- `Recursive` (Medium, Medium) — NOT immediate (effort too high).
- `ExpensiveCompute` (variable) — immediate only when the heuristic
  picked Small effort with Medium+ severity (rare; usually Medium
  effort or Large).

### 5.1 Refactor candidates

Implemented in `insights::collect_refactor_candidates`
([insights.rs:250](src/insights.rs#L250)). This is the
"this symbol needs a real look, not a one-line patch" surface.

A node qualifies iff ANY of:

- `findings.len() >= 2` (a cluster of findings on one symbol).
- Any finding has `effort == Large`.
- "God function with a finding": `loc >= 100 && findings.len() >= 1`.

For each qualifying node, build a `RefactorCandidate`:

- `findings_count`, `kinds` (sorted unique).
- `worst_severity` = max severity across findings.
- `max_effort` = heaviest effort across findings.
- `complexity`, `loc`, `percent_total`.
- `why` — pre-rendered explanation, e.g.:
  - "3 findings (blocking_in_async, n_plus_one, noisy_log) clustered
    on one symbol"
  - "3 findings (...) including a Large-effort one — full refactor"
  - "single Large-effort finding (e.g. high-complexity rewrite)"
  - "god function: 142 LOC with 1 finding(s)"

Sorted by `findings_count DESC, (loc + complexity) DESC, file ASC`,
then truncated to 30.

### 5.2 How the viewer uses these

The ScanReportPage section "Immediate fixes" consumes
`summary.immediate_fixes` directly — no client-side filter or sort,
the order from Rust is preserved
([viewer/src/pages/ScanReportPage.tsx:208](viewer/src/pages/ScanReportPage.tsx#L208)).
Each row resolves back into the global flat-list of findings via the
`(kind, line, node_id)` triple so the row can deep-link to
`/scan/<key>/finding/<idx>`.

The "Refactor candidates" section consumes
`summary.refactor_candidates` — each row links to
`/scan/<key>/node/<node_id>` rather than to a single finding
(by definition a refactor candidate is multi-finding).

---

## 6. Schema (`schema/profile.schema.json`)

The JSON Schema at [schema/profile.schema.json](schema/profile.schema.json)
documents `schema_version: "1.0"`. Top-level `Report = { schema_version,
mode, generator, summary, entries[] }`. See the schema file itself for
the field-level contract; types.ts mirrors it on the viewer side.

Notable invariants:

- `summary.findings_top.len() <= 50`.
- `summary.immediate_fixes.len() <= 50`.
- `summary.refactor_candidates.len() <= 30`.
- `summary.roots_overview` sorted by `subtree_size DESC`.
- `summary.categories` always contains every `Category` (zero-valued
  if absent) for stable UI rendering.
- `CallTreeNode.id` format: `"<file>::<parent_or_empty>::<name>"`.
  URL-safe by convention (encoded with `encodeURIComponent` by the
  viewer for deep-links).

---

## 7. Open-source dependencies and the classification data pipeline

drift-static-profiler is built on a small set of well-maintained
open-source crates plus a hand-curated **data pipeline** that
generates the per-language classification catalogs. This section
documents both, so you can audit "where does this signal come from"
end-to-end.

### 7.1 Runtime Rust dependencies (`Cargo.toml`)

Declared in [Cargo.toml](Cargo.toml). Every crate listed here is the
upstream open-source project — drift does not vendor or fork any of
them.

#### Parsing — `tree-sitter` and 8 language grammars

[tree-sitter](https://github.com/tree-sitter/tree-sitter) (v0.25) is
the incremental parser library by Max Brunsfeld. It compiles a small,
declarative grammar into a fast C parser exposing a CST you can query
with a Lisp-like pattern language. Every grammar in drift is a
separately-published crate so each language can version
independently:

| Crate | Version | Upstream |
|---|---|---|
| [tree-sitter-python](https://crates.io/crates/tree-sitter-python) | 0.23 | tree-sitter/tree-sitter-python |
| [tree-sitter-java](https://crates.io/crates/tree-sitter-java) | 0.23 | tree-sitter/tree-sitter-java |
| [tree-sitter-typescript](https://crates.io/crates/tree-sitter-typescript) | 0.23 | tree-sitter/tree-sitter-typescript (ships both `.ts` and `.tsx` languages) |
| [tree-sitter-javascript](https://crates.io/crates/tree-sitter-javascript) | 0.25 | tree-sitter/tree-sitter-javascript |
| [tree-sitter-go](https://crates.io/crates/tree-sitter-go) | 0.25 | tree-sitter/tree-sitter-go |
| [tree-sitter-rust](https://crates.io/crates/tree-sitter-rust) | 0.24 | tree-sitter/tree-sitter-rust |
| [tree-sitter-scala](https://crates.io/crates/tree-sitter-scala) | 0.26 | tree-sitter/tree-sitter-scala |
| [tree-sitter-containerfile](https://crates.io/crates/tree-sitter-containerfile) | 0.8 | camdencheek/tree-sitter-dockerfile |

**How drift uses them.** [parser.rs](src/parser.rs) statically links
each grammar via its `LANGUAGE` constant and pairs it with an embedded
**tags query** — the Lisp pattern language tree-sitter ships natively.
The query convention (`@def.function`, `@ref.name`, `@import.module`,
etc.) is documented in §3 / [parser.rs:28](src/parser.rs#L28).
[tags.rs](src/tags.rs) compiles the query at runtime via
`Query::new(language, query_str)` and iterates captures to build
`Symbol`/`Reference`/`ImportRecord` records.

**Why tree-sitter, not LSP/AST/regex?** Three reasons specific to
drift: (a) every grammar exposes the same query API, so all
seven-language support reuses one code path; (b) grammars are
self-contained C — no toolchains, no JVM, no Python runtime needed;
(c) error recovery is built in, so half-typed files still parse and
the analyzer produces a partial graph rather than failing.

#### File-system walking — `ignore`

[ignore](https://crates.io/crates/ignore) (v0.4) is the directory
walker extracted from `ripgrep` by BurntSushi. It honors `.gitignore`,
`.git/info/exclude`, and global Git ignore files using the exact
matching engine `git` itself uses, and parallelizes I/O across cores.

**How drift uses it.** [walker.rs](src/walker.rs) builds an
`ignore::WalkBuilder` with `standard_filters(true)` (toggles hidden +
gitignore + global gitignore in one), then overrides
`require_git(false)` so `.gitignore` is honored even outside a Git
checkout. Custom `.driftignore` is registered as a sibling rule file.
Same builder is reused by [linguist.rs](src/linguist.rs),
[manifest.rs](src/manifest.rs), and [docker.rs](src/docker.rs) so
every file-discovery surface in drift respects the same ignore
semantics.

#### Graph algorithms — `petgraph`

[petgraph](https://crates.io/crates/petgraph) (v0.8) is the
de-facto standard Rust graph crate. drift uses three of its
algorithms:

- `DiGraph<SymbolId, ()>` — the in-memory call graph
  ([graph.rs:148](src/graph.rs#L148)).
- `petgraph::algo::tarjan_scc(&g)` — Tarjan's strongly-connected
  components algorithm, used to detect mutual recursion
  ([graph.rs:165](src/graph.rs#L165)). Any SCC of size > 1 marks
  every member symbol as recursive. Tarjan was chosen over Kosaraju
  for its single-pass DFS — cheaper on the dense call graphs we see.
- `petgraph::algo::page_rank(&g, 0.85, 100)` — Brin & Page's
  classic PageRank with α=0.85 and 100 iterations
  ([graph.rs:180](src/graph.rs#L180)). The result is what powers
  the **hot path** signals in §4.3 (`pagerank ≥ p90` triggers
  `attach_hot_log_findings`, `attach_log_amplification_findings`,
  and the `bump_severities_by_impact` bump).

#### Serialization — `serde` + `serde_json` + `serde_yaml` + `toml`

[serde](https://serde.rs) (v1) is Rust's serialization framework.
drift uses derive-mode (`#[derive(Serialize, Deserialize)]`) on every
public type. The three concrete codecs:

- [serde_json](https://crates.io/crates/serde_json) (v1) — the
  `Report` JSON output and every input fixture in tests
  ([report.rs](src/report.rs), [tests/integration.rs](tests/integration.rs)).
  Also reads the classification catalogs at startup via
  `include_str!` + `serde_json::from_str` ([categories.rs:176](src/categories.rs#L176)).
- [serde_yaml](https://crates.io/crates/serde_yaml) (v0.9) —
  docker-compose parsing ([docker.rs:381](src/docker.rs#L381)).
- [toml](https://crates.io/crates/toml) (v0.8, `parse`-only) —
  `pyproject.toml` and `Cargo.toml` ([manifest.rs:209](src/manifest.rs#L209),
  [manifest.rs:255](src/manifest.rs#L255)). Default features are
  disabled and only `parse` is enabled to keep the dependency tree
  thin.

#### Error handling — `anyhow`

[anyhow](https://crates.io/crates/anyhow) (v1) by David Tolnay is
the application-level error type drift returns from `analyze` /
`analyze_roots`. Used for `Result<T>`-bearing top-level operations
where a downcastable error type is not needed (this is a binary, not
a library being embedded by third parties — `anyhow::Error` is the
right ergonomic choice). Per-file parse failures are NOT propagated
via `anyhow`; they're logged to stderr and the file is skipped, so
one broken file never aborts a whole scan.

#### CLI — `clap`

[clap](https://crates.io/crates/clap) (v4, `derive` feature) parses
the CLI. Each subcommand (`Analyze`, `Tags`, `Diff`, `Scan`,
`AnalyzeRoot`) is a `#[derive(Subcommand)]` enum variant in
[main.rs:17-145](src/main.rs#L17-L145). Auto-generated `--help`,
auto-coerced types, deterministic option ordering — all free.

#### Dev-only — `jsonschema`

[jsonschema](https://crates.io/crates/jsonschema) (v0.46,
default-features disabled) is a dev-dependency used only by the
integration test
`report_json_validates_against_schema_for_each_new_language`. It
validates emitted JSON against
[schema/profile.schema.json](schema/profile.schema.json) so a schema
drift between Rust and TypeScript types is caught at CI time, not at
runtime. Not paid for at runtime.

#### Release profile

The `[profile.release]` block at [Cargo.toml:41-44](Cargo.toml#L41-L44)
enables `lto = "fat"`, `codegen-units = 1`, and `strip = true`. Fat
LTO inlines aggressively across crate boundaries — meaningful here
because the hot path is `tags.rs` → `metrics.rs` → `graph.rs`, all
in separate modules but bound by per-symbol/per-reference loops.
Strip removes debug symbols from the shipped binary.

### 7.2 Viewer dependencies

The viewer at [viewer/](viewer/) is a small React SPA. Top-level
runtime deps (see [viewer/package.json](viewer/package.json)):

- **[React](https://react.dev) 18** + **react-dom** — UI framework.
- **[react-router-dom](https://reactrouter.com)** — client-side
  routing. Every page is a `<Route>`; URL is the source of truth.
  See [Router.tsx](viewer/src/Router.tsx).
- **[react-flame-graph](https://github.com/bvaughn/react-flame-graph)**
  — flame-graph rendering used by [FlameView.tsx](viewer/src/FlameView.tsx).
  Bvaughn's library is unmaintained but small and stable; drift
  pins to a known-good version and wraps it with a thin adapter
  layer in [transform.ts](viewer/src/transform.ts).
- **[Vite](https://vite.dev)** + **TypeScript** — dev server / build
  toolchain. No CSS framework, no UI kit — every style is a literal
  React style object so the JSON-rendering paths stay obvious.

### 7.3 The classification data pipeline — `src/research_classefiers+categories/`

[src/research_classefiers+categories/](src/research_classefiers+categories/)
is the **catalog generator** that produces the JSON files
[categories.rs](src/categories.rs) reads at startup. It's a separate
Cargo project (its own [Cargo.toml](src/research_classefiers+categories/Cargo.toml))
plus a parallel Python reference implementation
([generate.py](src/research_classefiers+categories/generate.py)).
A full design doc lives at
[research_classefiers+categories/README.md](src/research_classefiers+categories/README.md);
the summary below documents what each file does and how the data
flows.

#### Data sources, in priority order

1. **[OpenTelemetry Registry](https://github.com/open-telemetry/opentelemetry.io/tree/main/data/registry)**
   — primary source. ~918 structured YAML files, one per registered
   instrumentation, with `language`, `tags`, and `package.name`
   fields. CNCF-governed, vendor-neutral, updated monthly by
   `@otelbot`. drift's
   [build_otel_registry.py](src/research_classefiers+categories/build_otel_registry.py)
   scrapes this in one pass over the directory.
2. **opentelemetry-python-contrib `bootstrap_gen.py`** — the
   authoritative Python list. Parsed directly because OTel's
   auto-instrumentation team curates it with every release.
3. **Hand-curated seed YAMLs** — covers what upstream doesn't:
   - [rust.yaml](src/research_classefiers+categories/rust.yaml)
     and [scala.yaml](src/research_classefiers+categories/scala.yaml)
     because OTel coverage of those ecosystems is thin.
   - `*_stdlib.yaml`
     ([python_stdlib.yaml](src/research_classefiers+categories/python_stdlib.yaml),
     [java_stdlib.yaml](src/research_classefiers+categories/java_stdlib.yaml),
     [go_stdlib.yaml](src/research_classefiers+categories/go_stdlib.yaml),
     [js_stdlib.yaml](src/research_classefiers+categories/js_stdlib.yaml))
     because OTel only catalogs third-party packages, but real code
     imports `os`, `path`, `java.io`, `database/sql` constantly.
   - `*_otel.yaml` snapshots
     ([python_otel.yaml](src/research_classefiers+categories/python_otel.yaml),
     [java_otel.yaml](src/research_classefiers+categories/java_otel.yaml),
     [go_otel.yaml](src/research_classefiers+categories/go_otel.yaml),
     [js_otel.yaml](src/research_classefiers+categories/js_otel.yaml))
     — frozen copies of the OTel-derived data for offline
     reproducibility.

The README's "rejected sources" table is worth quoting briefly: PyPI
Trove classifiers (author-supplied, often wrong), npm keywords (no
taxonomy), Maven Central categories (stale), Datadog `dd-trace-*`
(proprietary trademarks), Snyk/OSV/Trivy (vulnerability-focused, not
functional), GitHub Topics (unstructured). OTel is the only source
that combines machine-readable + curated + vendor-neutral +
auto-refreshing.

#### Generator pipeline

```
   ┌──────────────────────────────┐
   │ OpenTelemetry Registry       │  fetch (HTTP) ─┐
   │ ~918 YAMLs                   │                │
   └──────────────────────────────┘                │
                                                   │
   bootstrap_gen.py (otel-python-contrib) ─────────┤
                                                   │
   seeds/*.yaml (stdlib + Rust/Scala) ─────────────┤
                                                   ▼
                                       ┌───────────────────────┐
                                       │ Categorizer           │  tags  → Category
                                       │ ~30 prioritized regex │  rules
                                       └───────────┬───────────┘
                                                   │
                                                   ▼
                                       ┌───────────────────────┐
                                       │ Dedup (first source   │
                                       │  wins) + sort by      │
                                       │ (category, module)    │
                                       └───────────┬───────────┘
                                                   │
                                                   ▼
                              python.json / javascript.json / java.json
                              go.json / rust.json / scala.json
```

Two parallel implementations of the same logic ship in this folder:
[main.rs](src/research_classefiers+categories/main.rs) (Rust, ~740
LOC, for embedding into CI) and
[generate.py](src/research_classefiers+categories/generate.py)
(Python reference, for iterating on rules without the recompile
loop). Both support `--self-test` (32/32 known mappings checked) and
`--offline` (seeds only, no network).

[sync_otel_registry.py](src/research_classefiers+categories/sync_otel_registry.py)
is the wire-fetcher: scrapes the registry YAMLs into local
`*_otel.yaml` snapshots so the generator stays offline-replayable.

#### The data files drift consumes at runtime

All of the following are embedded into the drift binary via
`include_str!` at compile time
([categories.rs:176](src/categories.rs#L176) onward), parsed once
into `OnceLock`-cached structures, and reused for every classify
call.

**Per-language catalogs** — Tier B input
(`classify_module(module_path)`).

| File | Purpose |
|---|---|
| [python.json](src/research_classefiers+categories/python.json) | Python packages → Category (~126 entries) |
| [javascript.json](src/research_classefiers+categories/javascript.json) | JS/TS packages → Category |
| [java.json](src/research_classefiers+categories/java.json) | Java packages → Category |
| [go.json](src/research_classefiers+categories/go.json) | Go modules → Category |
| [rust.json](src/research_classefiers+categories/rust.json) | Rust crates → Category |
| [scala.json](src/research_classefiers+categories/scala.json) | Scala packages → Category |

Each file's shape:

```json
{
  "language": "python",
  "generated_at": "2026-05-12T13:36:44Z",
  "category_set": ["db","network","io","cache","queue","log","compute"],
  "sources": ["seed/python_otel","seed/python_stdlib",
              "otel-registry/instrumentation-*.yml"],
  "count": 126,
  "entries": [
    { "module": "redis",      "category": "cache",  "source": "seed/python_otel" },
    { "module": "sqlalchemy", "category": "db",     "source": "otel-python-contrib" },
    { "module": "kafka",      "category": "queue",  "source": "otel-python-contrib" }
  ]
}
```

At load time, the catalog and overrides are concatenated and **sorted
by `module.len()` descending** ([categories.rs:211](src/categories.rs#L211))
so `django.db` resolves before `django` and
`org.springframework.data` resolves before `org.springframework`.
Module matching accepts equality, `prefix.`, `prefix/`, and `prefix::`
suffixes so the same catalog serves Python (`django.db`), Go
(`net/http`), and Rust (`sqlx::PgPool`) without per-language fanout
([categories.rs:248](src/categories.rs#L248)).

**Cross-language overrides** — augments every per-language catalog.

[module_overrides.json](src/research_classefiers+categories/module_overrides.json)
holds ~145 hand-curated entries that are not in OTel but matter to
drift's classifier — e.g. `django.db` → `db`, `prisma` → `db`,
`aioredis` → `cache`. Loaded together with the per-language files;
duplicates across files are tolerated because the longest-prefix-wins
sort handles ordering and same-category duplicates are harmless.

**Tier C — receiver patterns** (heuristic, language-agnostic).

[receiver_patterns.json](src/research_classefiers+categories/receiver_patterns.json)
maps lowercased receiver names exactly (no prefix match) — e.g.
`session`/`db`/`conn`/`tx`/`cursor` → `db`,
`axios`/`httpclient`/`fetch`/`http` → `network`, `logger`/`log` →
`log`, `cache`/`redis`/`memcached` → `cache`. Hand-curated;
deliberately kept short. Loaded once via `OnceLock` at
[categories.rs:268](src/categories.rs#L268). The shape:

```json
{
  "description": "Tier C receiver-name patterns. Matched against the lowercased
                  receiver name exactly (no prefix match)...",
  "category_set": ["db", "network", "io", "cache", "queue", "log", "compute"],
  "patterns": [
    { "name": "cache",   "category": "cache" },
    { "name": "ioredis", "category": "cache" },
    { "name": "jedis",   "category": "cache" }
  ]
}
```

**Tier D — unambiguous methods** (heuristic, case-sensitive).

[unambiguous_methods.json](src/research_classefiers+categories/unambiguous_methods.json)
holds method names that are diagnostic on their own — e.g.
`executeQuery` → `db`, `hgetall` → `cache`, `insertOne` → `db`,
`basicPublish` → `queue`. The file's own description spells out the
discipline: *"Deliberately tight — generic verbs like
save/add/find/get/delete/update are intentionally NOT here; they
require Tier B or Tier C evidence."* This is what keeps Tier D from
producing avalanches of false positives on common code. Loaded once
via `OnceLock` at [categories.rs:282](src/categories.rs#L282).

#### How drift uses these files end-to-end

When `categories::classify(name, receiver, &imports)` runs
([categories.rs:73](src/categories.rs#L73)):

1. **Tier B** consults the imports list against the per-language
   catalog (matched by suffix shapes against `module_path`).
2. **Tier C** lowercases the receiver and hits the receiver-pattern
   table.
3. **Tier D** exact-matches the method name against the unambiguous
   table.

First hit wins. The catalog data is **the only mutable surface** for
classifier behavior — the regex categorizer in
[generate.py](src/research_classefiers+categories/generate.py) /
[main.rs](src/research_classefiers+categories/main.rs) decides what
category a new OTel entry maps to; the data files then carry that
decision into the binary. Refreshing categories is a CI job
(see the README's monthly-cron example) — no Rust code change
needed when OTel adds a new instrumentation.

This separation is deliberate: it lets drift's categorization stay
auditable (every entry has a `source` field) and trivially extensible
(a new library is one PR adding a seed line, not a recompile of
classifier code).

---

## 8. TypeScript viewer, file by file

The viewer is a Vite + React SPA. State lives in the URL (via
`react-router-dom`), the report JSON is fetched per route with
`cache: 'no-store'`, and every clickable surface resolves to a
bookmarkable `<Link>`.

### main.tsx — bootstrap

[main.tsx](viewer/src/main.tsx) mounts `<React.StrictMode>` wrapping
`<BrowserRouter>` wrapping `<Router />` into `#root`. No error
boundary at this layer — page-level handling lives in `useReport`
and each page's `<ErrorScreen>`.

### Router.tsx — route table

[Router.tsx](viewer/src/Router.tsx) maps 5 paths:

- `/` → `<FixtureIndexPage />`
- `/scan/:fixtureKey` → `<App />` (legacy in-tab dashboard)
- `/scan/:fixtureKey/report` → `<ScanReportPage />`
- `/scan/:fixtureKey/finding/:findingIdx` → `<FindingDetailPage />`
- `/scan/:fixtureKey/node/:nodeId` → `<NodeDetailPage />`
- `*` → `<Navigate replace>` to `FIXTURES[0]` (404 safety).

### types.ts — schema mirror + entry-source filter helpers

[types.ts](viewer/src/types.ts):

- Schema types mirror Rust 1:1: `Report`, `Summary`, `CallTreeNode`,
  `Finding`, `ImmediateFix`, `RefactorCandidate`, `RootOverview`,
  `EntryDecl`, `EntryMatch`.
- Color palettes: `CATEGORY_COLORS`, `SEVERITY_COLORS`,
  `FINDING_KIND_LABEL`, `EFFORT_LABEL`.
- **Entry-point source filter helpers** ([types.ts:151-340](viewer/src/types.ts#L151-L340)):
  - `entryFamily(kind)` — collapses 11 `EntryKind`s into
    `'container' | 'manifest'`.
  - `summarizeEntryDeclMatches(decls)` — walks declarations once
    producing `{byKind, byFamily, anyMatched}` indexes keyed on
    `CallTreeNode.id`.
  - `buildEntryPointFilterOptions(entries, decls)` — pre-intersects
    each bucket with the visible `entries` so the dropdown never
    lists an option that would yield zero rows. Family rows only
    appear when both families exist.
  - `filterEntriesByDeclSource(entries, filter, decls)` — narrows
    entries while preserving caller order.

### transform.ts — flame-node conversion

[transform.ts](viewer/src/transform.ts) converts a `CallTreeNode`
subtree into the `FlameNode` shape consumed by `react-flame-graph`.
`subtreeWeight` prefers `subtree_size` when positive, else recursive
sum. Four `FlameMode` color branches: `kind`, `category`,
`complexity` (McCabe-banded), `smells` (red for N+1/blocking,
lavender for recursive). `truncated_reason` always overrides.

### callGraph.ts — JetBrains-style call-graph layout

[callGraph.ts](viewer/src/callGraph.ts) does:

- `buildCallGraph(root)` — BFS over the tree with dedupe by `id`;
  level updated to minimum on revisit; edges deduped.
- `layoutGraph(graph, opts)` — Sugiyama-lite: bucket by level, then
  for each level sort by **barycenter** of parents' column indices
  with `subtree_size` desc tiebreaker. Each level centered against
  the widest. `direction: 'TB' | 'LR'` swaps x/y at the end.
- `nodeColor(percentTotal)` — 3-band heat ramp: ≥40% high red, ≥5%
  medium amber, else cool green.
- `displayName(n, max)` — right-truncate by prepending `…` so the
  method name stays visible when the class prefix is long.

### fixtures.ts — known fixture list

[fixtures.ts](viewer/src/fixtures.ts) — 10 hard-coded `FixtureSpec`
entries: language-specific samples plus a `custom` slot for `make
scan` output.

### tooltips.ts — tooltip copy

[tooltips.ts](viewer/src/tooltips.ts) — flat `TIPS` record with ~80
keys grouped by topic. Multi-line strings written as `'...' + '...'`
concatenations. Consumers use literal property access so TypeScript
catches typos.

### pages/useReport.ts — fixture fetch + finding flattener

[useReport.ts](viewer/src/pages/useReport.ts):

- `useReport()` reads `:fixtureKey`, fetches `fixture.json` with
  `cache: 'no-store'`. Returns `{report, fixture, fixtureKey,
  loading, error}`.
- `flattenFindings(report)` — preorder DFS over every entry tree,
  emits `{node, finding, idx}` for each finding. `idx` is monotonic
  and defines the `/finding/:idx` URL contract (stable per JSON, not
  across re-runs that reorder entries).
- `findNodeById(report, id)` — DFS across every entry tree, first
  match wins.

### pages/FixtureIndexPage.tsx — landing card grid

[FixtureIndexPage.tsx](viewer/src/pages/FixtureIndexPage.tsx) — card
grid (`auto-fill`), each card a full-block `<Link>` to
`/scan/<key>/report`. `custom` fixture distinguished only by a label
badge.

### pages/ScanReportPage.tsx — the dedicated full-page report

The most important consumer page — at
[viewer/src/pages/ScanReportPage.tsx](viewer/src/pages/ScanReportPage.tsx).

**State hooks** run before any early return (hooks-rule compliance):

- `allFindings = flattenFindings(report)` — drives every
  "click → /finding/:idx" link.
- `sevCounts` — bucketed counts for the Health card.
- `healthScore = max(0, 10 − high·0.5 − medium·0.2 − low·0.05)`. The
  formula is rendered inline beneath the score for auditability.
- `totalFindings = sum(findings_by_kind values)`.
- `cats = Object.entries(summary.categories).filter(v>0).sort(desc)`.

**Cards rendered**:

- **Health** — gauge filled `(score/10)*100%`, score to 1 decimal,
  three `SevPill`s. Non-interactive.
- **Findings by kind** — bars per kind from `findings_by_kind`,
  sorted desc, fill width = `n / max`. Row click links to
  `/finding/<firstIdxOfKind>` via a precomputed map.
- **Categories** — bars per category from sorted `cats`.
- **Languages** — top 8 of `language_breakdown`.
- **Top findings** — `findings_top.slice(0, 10)`. Each row resolved
  back to its flat-index via `(kind, line, node_id)` matching.
- **Entry points** with **source-manifest filter** — see below.
- **Entry declarations** — shared `filterAndSortEntries` helper.
  Two-axis filter (text query + family). Confidence color: exact→red,
  likely→amber, unmatched→grey. `truncateMiddle` keeps both ends of
  long raw commands visible.

**Source-manifest filter (entry points card)**:

```
filterOptions = buildEntryPointFilterOptions(entries, entryDecls)
filterValue   ∈ {'all', 'family:container', 'family:manifest',
                 'kind:<EntryKind>', 'any-matched'}
filtered      = filterEntriesByDeclSource(entries, activeFilter, entryDecls)
sorted        = [...filtered].sort((a,b) => b.subtree_size - a.subtree_size)
```

A `useEffect` reverts to `'all'` if the active option disappears
after a fixture switch. The dropdown only renders when
`filterOptions.length > 1` (single-family scans don't get chrome
they can't use).

**Immediate fixes section** — consumes `summary.immediate_fixes`
directly, order preserved. Each row resolves to a `/finding/:idx`
link via `(kind, line, node_id)`. Each row shows severity pill, effort
pill, kind badge, `parent.name`, `file:line`, message.

**Refactor candidates section** — consumes
`summary.refactor_candidates` directly. Each row is a `<Link>` to
the node detail (no finding-level link because it's a multi-finding
aggregate). Renders `worst_severity`, `max_effort`, code,
`file:line`, the `why` string, plus the `kinds` badges.

**Initial roots section** — consumes `summary.roots_overview`. Each
`RootRow` has four blocks: a header link, a reach bar with width
`min(100, percent_of_all_roots)%`, findings chips (total + per-
severity pills, high pill itself links into the node detail),
reaches/first-calls/callers chips (sliced for compactness).

### pages/NodeDetailPage.tsx — per-symbol drill-in

[NodeDetailPage.tsx](viewer/src/pages/NodeDetailPage.tsx):

- `nodeId` from `useParams` (URL-decoded by react-router). Format is
  `file::class::name`.
- `node = findNodeById(report, nodeId)` — DFS first-match across all
  trees. Same-node-reachable-from-two-roots → first root wins.
- 9-metric tile row: `complexity`, `loc`, `nesting`, `params`,
  `callers`, `callees`, `subtree`, `pagerank` (4 decimals), `reach`
  (`percent_total` with `%`).
- Findings on this node: filtered from `flatFindings` by
  `f.node.id === node.id`, each row links to `/finding/:idx`.
- External calls: every external call rendered (no slicing). Shows
  `category` badge, `receiver.name`, `in-loop` / `in-await` tags,
  `:line`.
- **Callers sliced to 30** (no overflow indicator) — beyond 30 not
  visible from this page.
- Navigate footer: `← Scan report`, `← Dashboard`, first-5-children
  chips with `+ N more` overflow.

### pages/FindingDetailPage.tsx — per-finding deep-link

[FindingDetailPage.tsx](viewer/src/pages/FindingDetailPage.tsx):

- `findingIdx` from URL, coerced via `Number()` and
  `Number.isFinite()` guarded.
- `all = flattenFindings(report)`, pick `all[idx]`. `prev` and
  `next` precomputed for nav buttons.
- Renders severity badge, kind badge, "confidence X · #i of N"
  header, message, evidence list, remediation block,
  prev/next/node/dashboard nav.

### App.tsx — legacy in-tab dashboard

[App.tsx](viewer/src/App.tsx) at `/scan/:fixtureKey`. The full
workbench: toolbar, summary bar, flame graph, bottom tab strip with 8
tabs (`report`, `tree`, `graph`, `roots`, `hot`, `smells`,
`insights`, `stats`), splitters, Details pane.

Notable mechanics:

- **Tab-default policy** after load: `report` if any findings, else
  `roots` if ≥5 entries, else `tree`.
- **Fixture label override**: for `roots` / `custom` fixtures the
  label is the trailing path segment of `generator.source_root`.
- **Cross-root index** — single `useMemo` walks every entry tree to
  build `byId`, `byFileLine`, `byName` maps. `jump(lookup)` resolves
  the first non-null hit and switches active root if needed.
- **Toolbar source filter** — same machinery as ScanReportPage. The
  active root is always re-inserted into the dropdown even when
  filters would exclude it, so the `<select>` value stays valid.

### Components

- **ScanReport.tsx** — the legacy dashboard variant of the scan
  report rendered inside App's flame area. Composes 7 cards: Health,
  Findings breakdown, Categories, Languages, Hot zones, Entry
  points, Entry declarations. Health score uses the same formula as
  ScanReportPage. Hot zones prefers explicit `hot_zone` findings,
  falls back to `pagerank_top`.
- **Insights.tsx** — findings list panel. Flattens every finding
  tree-wide via pre-order DFS, sorts `severity DESC → file ASC →
  line ASC`. Three-stage filter: `presetKinds` (parent-supplied) +
  `kindFilter` + `sevFilter`. Selecting a row reveals an inline
  detail panel.
- **Smells.tsx** — code-smells table sibling. One row per (symbol,
  smell) pair so a symbol with both N+1 and BLK shows up twice.
  Evidence string built per kind (in-loop external calls for N+1,
  non-awaited externals for BLK, "SCC>1" for recursive).
- **RootsView.tsx** — sortable entry-point table. Sort keys: `reach`,
  `complexity`, `pagerank`, `name`, `smells`. No pagination —
  horizontal overflow scrolls. Smell count walks each subtree
  on every sort change.
- **CallGraphView.tsx** — JetBrains-style boxes + arrows. Uses
  `callGraph.ts` for layout, this file handles pan/zoom and SVG.
  Zoom uses cursor-anchored math:
  `panX' = cx − (cx − panX) × (next/zoom)`. Auto-fit runs once per
  `[root.id, direction]` cycle.
- **CallTreeView.tsx** — top-down tree. Recursive flat-rendered
  table (no virtualization). Default-open depth `< 3`. Open state is
  lost when a parent collapses.
- **FlameView.tsx** — thin wrapper around `react-flame-graph` with
  search + category dimming applied via `applyFilters`.
- **HotPaths.tsx** — hot paths list, no sort/filter, preserves Rust
  order.
- **DetailsPane.tsx** — right rail. Fixed shape (same fields in same
  order for every node, with empty states or hidden rows when data
  absent). Click on caller/child → `onJumpTo(id)`; click on external
  call → `onJumpExternal(file, line)`.
- **Statistics.tsx** — six-panel grid: pagerank, callers, callees,
  dead code, recursive symbols, language summary.
- **SummaryBar.tsx** — header with `Stat` widgets + category chips
  (active = yellow outline, others = dimmed when one is active).
- **Help.tsx** — tooltip primitive. Two modes (wrapper or `?` chip).
  Portal-rendered, position-clamped to viewport, hidden on scroll.
- **useResizableColumns.tsx** — column-resize hook + `ResizeHandle`,
  `useResizablePanel` + `Splitter` for layout-grid resizing.
  Persists widths in localStorage per table.

### Coupling summary

The viewer exclusively reads the Rust schema. It NEVER reinvents:

- Findings: it reads `findings`, `findings_by_kind`, `findings_top`.
- Immediate fixes: it reads `summary.immediate_fixes`.
- Refactor candidates: it reads `summary.refactor_candidates`.
- Roots overview: it reads `summary.roots_overview`.
- Entry-point declarations: it reads `summary.entry_declarations`
  and the `entry_labels` array on each root.

The only client-side derivations are:

- The `idx` in `/finding/:idx` URLs (preorder DFS counter).
- The Health score (formula rendered inline on the page).
- Sort orders shown to the user (sortable tables).
- Dropdown options for the source-manifest filter
  (`buildEntryPointFilterOptions` pre-intersects with visible
  entries).

---

## 9. Glossary

- **Symbol** — a defined unit: function, method, or class. Created
  by `tags.rs` from tree-sitter captures.
- **Reference** — a call site. Has a name, optional receiver, and a
  byte offset / line.
- **External call** — a `Reference` that did NOT resolve to an
  in-project symbol AND was classified by `categories::classify` into
  one of seven categories. Carries `in_loop`, `in_await`, `tier`,
  `evidence`.
- **CallTreeNode** — one node in a per-root call tree. Carries all
  Phase A (symbol metrics), Phase B (graph-derived: pagerank,
  call_site_count, is_recursive), Phase C (percentages), Phase D
  (legacy booleans), Phase E (findings) data.
- **Finding** — a structured issue attached to one CallTreeNode.
  Has `kind`, `severity`, `effort`, `confidence`, `line`, `message`,
  `evidence[]`, `remediation`.
- **Immediate fix** — a finding with `severity >= Medium && effort
  <= Small`. Surfaced in `summary.immediate_fixes`.
- **Refactor candidate** — a node-level aggregate. Qualifies when:
  multi-finding cluster on one node, OR any `Large`-effort finding,
  OR god-function (loc ≥ 100) with a finding.
- **Hot path** — a chain from a root ending at a node with a
  `category_self` or external call. Top 10 stored in
  `summary.hot_paths`.
- **Hot zone** — `pagerank >= p90` plus (eventually) a multi-finding
  cluster. The `HotZone` finding kind is reserved; the detector is
  not yet implemented.
- **Tier** — classifier confidence band on an external call:
  `ImportedModule` > `ReceiverPattern` > `MethodSignature`.
- **`<module>` symbol** — synthetic per-file symbol covering
  module-level executable code. Skipped by every detector that would
  fire on its file-wide proxies.
- **p90** — 90th percentile of pagerank across all symbols, used
  as the "hot path" threshold by 3 different passes.
