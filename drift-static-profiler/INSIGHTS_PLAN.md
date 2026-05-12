# Drift Static Profiler — "Insights & Findings" Phase: Plan & Research

## 0. Goal

Today the scan emits a call tree with per-node metrics and Phase-D risk
flags (`n_plus_one_risk`, `blocking_in_async`, `is_recursive`). The
viewer surfaces those as a *Smells* tab.

The next step turns the scan from a **data dump** into a **report with
findings**. The work has three parts, all clean extensions of what
exists today — no parallel hierarchies, no new module folders:

1. **Per-node `findings: Finding[]`** attached to each `CallTreeNode`,
   produced inline in `tree::build_inner` next to the existing risk
   flags. The existing booleans (`n_plus_one_risk`, etc.) become derived
   convenience values computed from `findings` — they stay populated so
   `Smells.tsx` and flame-mode `'smells'` keep working unchanged.
   Summary-level rollups (`findings_top`, `findings_by_kind`) mirror the
   existing `pagerank_top` / `recursive_symbols` pattern.

2. **Five detector families**, each a pure function called from
   `tree::build_inner` (except `hot_zone`, which is one small
   post-build pass for cross-graph signals):
   - `n_plus_one` / `smelly_loop` — db/cache/network in loops, repeated work
   - `noisy_log` — logs in loops, debug logs on hot paths, log in recursion
   - `outdated_package` — imports flagged by a curated catalog
   - `memory_explosion` — unbounded materializers + unbounded recursion
   - `hot_zone` — high pagerank × high reach × concentrated findings

3. **Two new viewer pages**:
   - **Scan Report Summary** — one-screen executive overview (health
     score, findings breakdown, category reach, language breakdown, top
     hot zones, entry points). The full-page sibling of `SummaryBar.tsx`.
   - **Insights** — the actionable findings list, filterable by kind /
     severity, click-to-jump using the existing `jump()` API. `Smells`
     becomes a filtered preset of this same component (step 13).

This document is the **research and plan** for that work. Implementation
happens in 14 micro-steps after this doc is reviewed (§7).

---

## 1. Chain-of-thought: what does it mean to turn a profile into "insights"?

A raw profile is just samples / call counts / metrics. A *report* is the same
data, but the tool has done the prioritization for the human. Industry
profilers do this in two ways:

### 1.1 PyCharm / dotTrace — "read the report" model
Per JetBrains [Read the profiling report]
(https://www.jetbrains.com/help/pycharm/read-the-profiling-report.html), the
profiler exposes:

- a per-function table with **Time**, **Own Time**, **Calls**, **Name**;
- a flame graph (rectangles ordered by width = time);
- a call tree (hierarchical, with execution-time share, aggregated time, recursion markers);
- color coding (red = slow, green = fast) so the eye finds bottlenecks first;
- a "Most affected" list that walks the tree and lifts the chains that end in expensive leaves.

We already have analogs of every one of these (`Summary.top_callees`,
`hot_paths`, the flame view, recursion flag). What's missing is the
**editorialization layer** — telling the user "this method is in the top 1%
of the program by reach AND it does N+1, so look at it FIRST." That's the
job of an `insights` array with severity scoring.

### 1.2 dotTrace "Forecast Performance" — what-if model
The [Forecasting Performance Timeline]
(https://www.jetbrains.com/help/profiler/Forecasting_Performance_Timeline.html)
page is actually about *hypothetical exclusion*: "if I deleted method X, the
tree would look like this". The web-fetched summary confirmed it is not a
predictive ML model. For a static profiler the equivalent is **counterfactual
ranking**: given a finding (e.g. N+1 in `OrderRepo.save`), how much of the
project's reachable surface is downstream of that fix? We can compute this
cheaply: it's `subtree_size` / `total_subtree_size` of the entry. We expose
this as `impact_percent` on each finding so the user knows the blast radius
of fixing it.

### 1.3 V8 ProfileNode, pprof Sample, speedscope Frame — the data model
The wire formats all share the same shape:

| Format | Frame identity | Cost field | Aggregation |
| --- | --- | --- | --- |
| V8 ProfileNode | `callFrame` (function id + url + line) | `hitCount`, `positionTicks` | tree of nodes |
| pprof Sample | `location_id[]` → `Function` | `value[]` per `ValueType` (cpu, alloc, …) | flat samples, reconstruct tree |
| speedscope Frame | `Frame { name, file, line }` | `samples`/`weights` or `events` | both modes |

What this tells us:
- Every profiler **interns** the frame identity (function + file + line). Our
  `CallTreeNode.id = file::class::name` already does the equivalent, so we
  can use it as the foreign key for findings.
- Profilers carry **multiple cost columns** in parallel (`sample_type[]` in
  pprof, `value_types` in our schema). Findings should likewise carry
  *multiple severity signals* — confidence, impact, reach — not a single
  opaque "badness" number.
- speedscope's split between sampled and evented profiles confirms our schema
  decision to keep `mode: "static" | "sampled" | "evented"` at the top level;
  the `insights` section we add is mode-agnostic because it's derived from
  the call-tree / external-call layer that exists in all modes.

### 1.4 Profilerpedia / format zoo
[profilerpedia.markhansen.co.nz](https://profilerpedia.markhansen.co.nz/formats/speedscope/)
catalogs which tools emit which formats. The takeaway for us: if we ever want
to feed sampled data into the same viewer, speedscope is the de-facto
interchange. Our `insights` section must therefore be **purely additive** —
not embedded inside `entries[*]` only — so a future runtime profile (which
might not have a hand-pinned `entries` array) still gets an insight feed.

### 1.5 So what is the right shape?
A **flat array of `Finding` objects** at the top level of the report,
each pointing back to one or more `CallTreeNode.id`s via foreign-key, each
with a structured `kind`, a `severity`, and a `confidence`. Renderable as a
table, sortable, filterable, navigable.

This is consistent with how Lighthouse, ESLint, SonarQube, Clippy, and the
GitHub code-scanning SARIF format do it. We are not inventing a paradigm;
we are matching one.

---

## 2. What the codebase already has (don't reinvent)

Before any new code, here's the inventory of facts already computed during a
scan. Every new detector should **reuse** these, not re-walk ASTs.

### 2.1 Per-symbol facts (`Symbol`, computed in `src/metrics.rs`)
- `complexity` (McCabe cyclomatic)
- `loc`, `nesting_depth`, `parameter_count`
- `is_async`
- `loop_ranges: Vec<(byte_start, byte_end)>` — every loop body in the symbol
- `await_ranges: Vec<(byte_start, byte_end)>` — every `await` expression site

### 2.2 Per-call-site facts (`ExternalCall`, computed in `src/graph.rs`)
- `name`, `receiver`, `category` (db | network | io | cache | queue | log | compute)
- `tier` (`imported_module` > `receiver_pattern` > `method_signature`)
- `evidence` (human reason)
- `in_loop`, `in_await` (byte-range check against the enclosing symbol)
- `line` (the call-site line, not the symbol line)

### 2.3 Per-node facts (`CallTreeNode`, computed in `src/tree.rs`)
- `subtree_size`, `percent_total`, `percent_parent`
- `callers`, `callers_count`, `callees_count`
- `categories_reached` (Σ over subtree)
- `call_site_count`, `is_recursive`, `pagerank`
- `n_plus_one_risk`, `blocking_in_async`

### 2.4 Per-graph facts (`CallGraph`, `Summary`)
- `top_callers`, `top_callees`, `hot_paths`, `dead_code`, `pagerank_top`,
  `recursive_symbols`
- `categories: BTreeMap<String, usize>` (project-wide rollup)
- `language_breakdown`, `profiled_language`

### 2.5 Per-file facts (`FileTags`, computed in `src/tags.rs`)
- All symbols, references, **imports** (`ImportRecord`), bindings
- Imports give us the package-detection signal for free.

### 2.6 What's already wired in the viewer
- `App.tsx:127` — `jump({ id | file/line | name })` switches active root and
  selects a node. Insights tab will use this exact path.
- `App.tsx:233-265` — bottom-tab pattern with tree/roots/hot/smells/stats.
  Adding `'insights'` is a one-line `BottomTab` union extension plus one
  new `<Tab>` button and one new render block.
- `types.ts` — TypeScript mirror of the Rust schema; new types added here.

This means **every detector I add gets its data from one of three sources**:
the per-symbol byte-ranges, the externals list, or the tree-derived
aggregates. No new AST walks for most findings.

### 2.7 Module-layout conventions (clean extension, not parallel system)

Three rules the plan adheres to throughout:

1. **One file, not a folder.** The codebase is flat —
   [`metrics.rs`](src/metrics.rs), [`categories.rs`](src/categories.rs),
   [`graph.rs`](src/graph.rs), [`tree.rs`](src/tree.rs),
   [`report.rs`](src/report.rs). The insights work lives in
   **`src/insights.rs`** (single file). Split into a folder only if it
   grows past ~600 lines.
2. **Catalogs go where catalogs already live.** Data files belong in
   [`src/research_classefiers+categories/`](src/research_classefiers+categories/)
   (the existing folder name; do not rename) — same folder as today's
   category catalogs, embedded via `include_str!` at compile time. No
   new `src/insights_data/`.
3. **Findings extend, never duplicate.** Per-node markers live on the
   node (`CallTreeNode.findings`); per-summary rollups live on `Summary`
   (`findings_top`, `findings_by_kind`). Existing boolean markers
   (`n_plus_one_risk`, `blocking_in_async`, `is_recursive`) become
   **derived** values computed from `findings` — they stay populated so
   `Smells.tsx` and flame-mode `'smells'` keep working unchanged.

---

## 3. JSON schema additions (clean-extension shape)

The codebase's existing pattern: per-node markers live ON the node
(`CallTreeNode.n_plus_one_risk`, `blocking_in_async`, `is_recursive` at
`tree.rs:57-58`), and `Summary` carries top-N **views** into the same data
(`pagerank_top`, `recursive_symbols`, `dead_code` at `report.rs:28-30`).
The insights phase extends both — **no second hierarchy, no foreign keys**.

### 3.1 Per-node: `CallTreeNode.findings: Finding[]`

Lives next to the existing boolean markers. A finding is the structured
form of "this node has this kind of problem here":

```jsonc
// CallTreeNode (extended) — only the new + relevant fields shown
{
  "id": "src/services/order.py::OrderService::save_orders",
  "name": "save_orders",
  "file": "src/services/order.py",
  "line": 38,
  "n_plus_one_risk": true,         // KEPT — derived from `findings`, zero-cost
  "blocking_in_async": false,      //   convenience for old consumers + flame coloring
  "is_recursive": false,
  "findings": [                    // NEW
    {
      "kind": "n_plus_one",        // enum (closed-set today, see §3.3)
      "severity": "high",          // low | medium | high
      "confidence": 0.95,          // 0..1
      "line": 43,                  // call-site within the symbol, not symbol-start
      "message": "session.add() inside `for o in orders:` will issue one INSERT per element.",
      "evidence": [                // optional; lines/snippets backing the finding
        { "call": "session.add",    "line": 43, "category": "db" },
        { "call": "session.commit", "line": 45, "category": "db" }
      ],
      "remediation": "Batch the inserts with session.bulk_save_objects(list)."
    }
  ]
}
```

The node IS the context. Everything that could be on the finding but is
already on the parent node (`file`, `parent_class`, `pagerank`,
`percent_total`, `categories_reached`) is **not** repeated.

### 3.2 Per-summary: `findings_top` + `findings_by_kind`

Same shape as `pagerank_top` / `recursive_symbols`. A flat index for the
Insights tab to sort/filter without re-walking the tree:

```jsonc
"summary": {
  …existing fields…,
  "findings_by_kind": {                // NEW — rollup
    "n_plus_one": 7, "noisy_log": 3,
    "outdated_package": 5, "memory_explosion": 4,
    "hot_zone": 23
  },
  "findings_top": [                    // NEW — top N for the Insights tab
    { "node_id": "src/services/order.py::OrderService::save_orders",
      "kind": "n_plus_one", "severity": "high", "line": 43 },
    …
  ]
}
```

`node_id` is the same `CallTreeNode.id` the viewer's existing
`nodeIndex.byId` map at `App.tsx:99` already keys on — no new navigation
infra.

### 3.3 `Finding` shape — minimal

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,              // enum below
    pub severity: Severity,             // Low | Medium | High
    pub confidence: f64,                // 0..1
    pub line: usize,                    // call-site or relevant line within the symbol
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    NPlusOne,            // db/cache call in loop          (replaces n_plus_one_risk in role)
    BlockingInAsync,     // db/network/io w/o await        (replaces blocking_in_async in role)
    Recursive,           // SCC of size > 1                (replaces is_recursive in role)
    SmellyLoop,          // other in-loop work             (§4.1)
    NoisyLog,            // log smells                     (§4.2)
    OutdatedPackage,     // catalog match                  (§4.3)
    MemoryExplosion,     // catalog match / unbounded rec  (§4.4)
    HotZone,             // pagerank + reach + finding mix (§4.5)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub call: String,                   // method name; "import" / "loop" / etc. for non-call evidence
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
}
```

### 3.4 Why this shape (clean-extension rationale)

- **Findings live next to the data they describe.** No foreign keys; the
  finding's "primary node" is the node it's on. This mirrors how every
  existing risk flag works today (Phase D in `tree.rs:136-146`).
- **Existing booleans stay** as cached, derived values. The flame-mode
  `'smells'` painter (`App.tsx:17,32`) and the `Smells.tsx` page keep
  working unchanged; internally the booleans become
  `findings.iter().any(|f| f.kind == FindingKind::NPlusOne)` etc.
- **`Summary.findings_top` mirrors `pagerank_top`** — same pattern, same
  render shape, same `jump({ id })` navigation.
- **`Finding` carries only what the node doesn't.** No duplicated
  `file`/`parent_class`/`impact_percent`; the viewer reads them off the
  node it's rendering.

### 3.5 Backward compatibility
- `CallTreeNode.findings`, `Summary.findings_top`, `Summary.findings_by_kind`
  are all **optional**. Schema `required` list does not change.
- Existing fixtures continue to validate.
- The `Smells` tab keeps reading the existing booleans (which we continue
  to populate from `findings`) until step 10 swaps it to read findings
  directly.

---

## 4. The five finding families — detector design

For each family I list: (a) the signal we detect on, (b) the data source
we already have, (c) the severity heuristic, (d) the false-positive risks I
expect, (e) the micro-step to ship.

### 4.1 `smelly_loop` — repeated work & missing caching

**Signals to detect (ordered by confidence, highest first):**

| # | Pattern | Signal |
| - | --- | --- |
| 1 | db / cache / network call in loop | `external_call.in_loop && category ∈ {db, cache, network}` |
| 2 | pure / deterministic call repeated in loop | call site count ≥ 2 in same loop range, same callee, no mutation of args |
| 3 | recomputed expression in loop (e.g. `len(x)` per iter when `x` is loop-invariant) | requires AST loop-invariant analysis |
| 4 | nested loop O(n²) on the same collection (`for i: for j in same`) | AST: two loops over same iterable |
| 5 | inefficient stdlib idiom (Python: `for k in d.keys(): d[k]`; JS: `arr.forEach` with await; etc.) | language-specific peephole |

**Data source.** Today we already classify (#1) into `n_plus_one_risk` for
db & cache. We extend coverage as follows:

- Reuse `loop_ranges` from `metrics.rs`. For each call site (both `ExternalCall`
  and intra-graph callees we resolve) check whether `byte_offset` lies in any
  loop range. **Same machinery as `in_loop`, but applied to internal callees
  too.**
- For #4 (nested loops on same iterable), extend `metrics.rs` to also collect
  the loop-iterable AST node text and compare across nested ranges.
- #2 / #3 are loop-invariant analysis. **Defer to a later micro-step.**
- #5 is a small per-language peephole table. Ship #1, #4 in v1.

**Severity heuristic.**
```
severity = max(category-severity, repetition-severity, reach-severity)

category-severity:
  db  → high
  cache → high
  network → high
  io → medium
  log → low
  compute → low

repetition-severity:
  call inside doubly-nested loop AND on db/cache/network → high
  call inside loop in top-pagerank-10% symbol → high
  call inside loop in any other symbol → medium

reach-severity:
  primary_node.percent_total ≥ 20 → high
  ≥ 5 → medium
  else → low

confidence:
  external_call.tier == imported_module → 0.95
  == receiver_pattern → 0.80
  == method_signature → 0.65
```

**False-positive risks.**
- Class instantiation in loop (`for x in xs: SomeClass()`) — we already
  filter constructors via `is_method_call()` in `tree.rs:129-135`. Reuse.
- Looping over a known-small collection (length 2-3). We can't tell from
  static. **Mention in remediation, don't downgrade the finding.**
- `for await` / `async for` iterations — same machinery; the worry is the
  same.

**Micro-step.**
1. Add `src/insights.rs` (flat file, matches the codebase's `metrics.rs` /
   `categories.rs` / `graph.rs` flat layout).
2. Inside it, a pure function
   `fn detect_loop_smells(sym: &Symbol, externals: &[ExternalCall]) -> Vec<Finding>`.
3. Call it from `tree::build_inner` at the same point Phase D runs
   (`tree.rs:136-146`). The result is pushed into the node's new
   `findings: Vec<Finding>` field; the existing `n_plus_one_risk` bool
   is then computed as
   `findings.iter().any(|f| f.kind == FindingKind::NPlusOne)`.
4. Group consecutive in-loop externals in the same loop range into one
   finding with multi-line `evidence`.
5. Snapshot-test against `tests/fixtures/python-fastapi` where we know
   the N+1 site is.

### 4.2 `noisy_log` — broken logs filling up storage

**Signals.** Today we *categorize* log calls (`category: "log"`) but don't
flag them. New detector rules:

| # | Pattern | Signal |
| - | --- | --- |
| 1 | log call inside a loop | `external_call.in_loop && category == log` |
| 2 | log call inside an exception handler that does `except: log.error(e); raise` then later **re-handles** → double-logging | requires limited control-flow walk; defer |
| 3 | DEBUG-level log on a hot path (top-pagerank symbol) | `log` external whose method name ∈ {`debug`, `trace`, `verbose`} and primary node is in `pagerank_top` |
| 4 | log of a large object (`.read()` / `.body` / `request.json()` passed in) | argument heuristic; needs ref tracking; defer |
| 5 | log call with no string formatting and many positional args, in production code | language-specific; defer |
| 6 | log inside a recursive symbol | `external_call.category == log && symbol.is_recursive` |

**Data source.** Already have category classification, `in_loop`, `pagerank`,
`is_recursive`. The method-name check requires looking at `external_call.name`,
which we have.

**Severity heuristic.**
```
log-in-loop in top-10% pagerank symbol         → high
log-in-loop in top-50% pagerank symbol         → medium
log-in-loop anywhere else                       → low
DEBUG log in top-10% pagerank symbol            → medium  (configurable; some shops want this off)
log in recursive symbol                         → medium
```
Confidence is high (0.9) — log classification is one of our cleaner
categories.

**False-positive risks.**
- Init-time logs (one-shot logs during app startup) sometimes live inside a
  loop. Mention in remediation: "if this only runs at startup, mark the
  loop or ignore." Don't drop the finding.
- Structured logs with throttling (`logger.info(..., once=True)` in some
  frameworks) are fine. We can't detect throttling statically. Surface low
  severity.

**Micro-step.**
1. `fn detect_log_smells(sym: &Symbol, externals: &[ExternalCall], ctx: &Ctx)`
   in `src/insights.rs`. `ctx` carries the global `pagerank_p90` threshold.
2. Called from `tree::build_inner` next to `detect_loop_smells`.
3. Evaluate rules #1, #3, #6 (the ones we can do today).
4. Snapshot test against a fixture we add: `tests/fixtures/insights-logs/`
   with a Python file that logs in a loop and on a hot path.

### 4.3 `outdated_package` — known-bad imports

**Signals.** This is the only family where we need an **external knowledge
base**. The signal is "an import in the code matches a curated record."

**Data source.**
- `FileTags.imports: Vec<ImportRecord>` already gives us every import per
  file with module path, local binding, alias.
- The catalog of flagged packages lives **alongside** the existing
  classifier catalogs in `src/research_classefiers+categories/` and is
  embedded at compile time with `include_str!` (same pattern as today's
  category data). No new data folder.

**Catalog format (proposed file: `src/research_classefiers+categories/outdated_packages.json`):**
```json
{
  "schema_version": "1",
  "updated_at": "2026-05-13",
  "languages": {
    "python": {
      "requests": {
        "status": "alternative",
        "reason": "for async code, use httpx; requests is sync-only",
        "replacement": "httpx",
        "severity": "low"
      },
      "ujson": {
        "status": "unmaintained",
        "reason": "last release 2022; orjson is faster and maintained",
        "replacement": "orjson",
        "severity": "medium"
      },
      "moment": {
        "status": "deprecated",
        "reason": "moment.js officially in maintenance mode; use date-fns or Luxon",
        "replacement": "date-fns",
        "severity": "medium"
      }
    },
    "javascript": {
      "request": {
        "status": "deprecated",
        "reason": "deprecated 2020; use undici or node:fetch",
        "replacement": "undici",
        "severity": "high"
      },
      "moment": { … },
      "lodash": {
        "status": "alternative",
        "reason": "modern JS has most lodash equivalents natively",
        "replacement": "native ES",
        "severity": "low"
      }
    },
    "java": {
      "com.fasterxml.jackson.databind.ObjectMapper$useDeprecatedXyz": { … }
    }
  }
}
```

**Status enum:** `cve | deprecated | unmaintained | alternative | superseded`.
Severity maps cleanly: cve → high, deprecated → medium, unmaintained →
medium, alternative → low, superseded → low.

**Severity heuristic.**
- Start with `status`-based default.
- Bump by one tier if the import is used inside a top-10% pagerank symbol
  (i.e. the dependency is on a hot path).
- `confidence = 1.0` if module path exact-matches, `0.7` if prefix-matches a
  submodule (e.g. catalog has `requests`, file imports `requests.sessions`).

**False-positive risks.**
- The catalog has to be curated, not hallucinated. Each catalog entry
  carries a `reason:` that is checkable — humans, not the tool, decide what
  goes in. The doc that ships with v1 includes the *exact list* we seed.
- Many imports are dev-only (test, lint). We will skip imports inside files
  under `tests/`, `spec/`, `__tests__/`, etc. (configurable).

**Micro-step.**
1. Seed `outdated_packages.json` in
   `src/research_classefiers+categories/` with ~30 entries across 3
   languages, every one with a `reason:` link to a publicly verifiable
   source (Stack Overflow, project README, NVD).
2. In `src/insights.rs`, load the catalog once (lazy_static / OnceCell
   from `include_str!`). Detector signature:
   `fn detect_outdated_packages(sym: &Symbol, file_imports: &[ImportRecord], ctx: &Ctx)`.
3. Called from `tree::build_inner` with the imports of the file the
   symbol lives in. (Imports per-file are already in `FileTags`; the
   caller passes a `&BTreeMap<PathBuf, &[ImportRecord]>` view.)
4. Test: fixture under `tests/fixtures/insights-packages/python/` that
   imports `requests`, `moment` (via pyodide example), etc.

### 4.4 `memory_explosion` — places memory could blow up

**Signals.** Static memory analysis is genuinely hard, so we restrict to a
small set of *very-high-confidence patterns* and clearly mark the rest as
heuristic:

| # | Pattern | Signal |
| - | --- | --- |
| 1 | `.read()` / `.read_to_string()` / `read_to_end()` on a stream / file when streaming alternative exists | external call name match against `data/memory_explosion.json` |
| 2 | `list(some_iterator)` / `[*iter]` / `Array.from(stream)` materializing an unbounded iterator | external + receiver heuristic |
| 3 | growing accumulator inside a loop without a clear bound (`results.append(...)` where `results` outlives the function and isn't size-bounded) | requires reference tracking; defer |
| 4 | recursive symbol with no depth guard (`is_recursive=true && nesting_depth ≥ N && no early return`) | already have `is_recursive`; depth guard is heuristic |
| 5 | log payload or string concat in loop (`s += big_thing`) | language idiom; defer |
| 6 | unbounded cache: a dict / Map populated in a hot symbol with no eviction signal (`del`, `clear`, `pop`, `lru_cache(maxsize=...)`) | reference tracking; defer |

**Data source.**
- A second catalog `src/research_classefiers+categories/memory_explosion.json`
  lists "method names that are known unbounded materializers" per language.
  Same folder + same `include_str!` pattern as `outdated_packages.json`.
- For #4 we already have `is_recursive` and `nesting_depth`.

**Catalog example (excerpt):**
```json
{
  "python": [
    { "method": "read",        "receivers": ["open", "file", "Path"], "reason": "loads whole file into memory; prefer iteration or stream chunks" },
    { "method": "readlines",   "reason": "same as .read() but split by line; prefer iteration over the file object" },
    { "method": "json",        "receivers": ["response", "request"], "reason": "loads whole HTTP body; prefer .iter_content() for large responses" }
  ],
  "javascript": [
    { "method": "buffer",      "receivers": ["response"], "reason": "buffers full HTTP body; prefer .body for streams" },
    { "method": "readFileSync","reason": "synchronously loads file; prefer createReadStream for large files" }
  ]
}
```

**Severity heuristic.**
```
catalog match on hot path  → high
catalog match elsewhere    → medium
unbounded recursion        → medium
```
Confidence 0.7 (lower than db/log; this family is heuristic).

**Micro-step v1.**
1. Seed the memory catalog with ~15 entries (read / readlines / json /
   buffer / readFileSync / ...).
2. In `src/insights.rs`,
   `fn detect_memory_smells(sym: &Symbol, externals: &[ExternalCall], ctx: &Ctx)`.
3. For unbounded recursion, reuse `sym.is_recursive` already on the
   `CallTreeNode` — emit a `MemoryExplosion` finding when
   `is_recursive && nesting_depth ≥ 3 && complexity ≥ 5` (rough proxy for
   "no early return"). Confidence 0.6 — we mark this clearly heuristic.
4. Ship #1 and #4 only in v1; document the others as "planned".

### 4.5 `hot_zone` — re-surfaced from existing analysis

**Signal.** A `hot_zone` is a node satisfying any of:
- `pagerank` in top 5% of the project, AND
- `subtree_size` in top 5%, AND
- has at least one of: `n_plus_one_risk`, `blocking_in_async`,
  `categories_reached.db > 3`, or contains a child with a finding from §4.1–4.4.

**Severity** is computed from how many of those signals fire:
- 4+ signals → high; 2-3 → medium; 1 → low (rarely emitted, usually subsumed).

**Why also emit them as `Finding`s?**
Because the value of the Insights tab is **one ranked list to look at**.
Hot zones without findings are interesting but not actionable; hot zones
*with* findings are the top of the list. Emitting them as a `HotZone`
finding alongside the others gives the user a single sorted view by
`severity DESC, percent_total DESC`. (`percent_total` lives on the node
already, so the viewer sorts without needing a dedicated `impact` field.)

**Micro-step.** Hot-zone detection needs the **whole tree built first**
(pagerank percentile is a graph-wide quantity), so it's the only detector
that can't run inside `build_inner`. Implementation: a small post-build
pass `fn attach_hot_zones(entries: &mut [CallTreeNode], graph: &CallGraph)`
in `src/insights.rs` invoked from `Report::build` after the trees are
assembled. It walks each tree, computes the pagerank-p90 threshold once,
and pushes `HotZone` findings onto qualifying nodes. This is "Phase E2".

---

## 5. Severity scoring — one rule, no extra struct

A consistent severity bump across all detectors keeps the UX coherent.
Every signal the helper needs is **already on `CallTreeNode`** (`percent_total`,
`pagerank`, `call_site_count`). The only graph-wide input is the pagerank
p90 threshold, which the caller computes once before walking the tree.

```rust
/// Bump a detector's base severity by impact signals already on the node.
/// `pagerank_p90` is computed once by the caller from `graph.pagerank.values()`.
pub fn bump_by_impact(node: &CallTreeNode, base: Severity, pagerank_p90: f64) -> Severity {
    let boost = (node.percent_total >= 20.0) as u8
              + (node.pagerank      >= pagerank_p90) as u8
              + (node.call_site_count >= 10) as u8;
    match (base, boost) {
        (Severity::Low, 3) => Severity::High,
        (Severity::Low, 2) => Severity::Medium,
        (Severity::Medium, 2..=3) => Severity::High,
        (s, _) => s,
    }
}
```

No new struct. Three already-stored signals on the node, one global
threshold from the graph. Each detector emits a base severity; the caller
applies `bump_by_impact` once at the end before pushing the finding.

---

## 6. Viewer extensions — two new pages

We add **two** sibling tabs to the existing bottom-tab strip
(`tree / roots / hot / smells / stats`):

1. **Scan Report Summary** — executive-overview "cover page" of the scan.
   The full-page sibling of the existing thin `SummaryBar.tsx`.
2. **Insights** — the actionable findings list (§3-§5).

The existing `Smells` tab becomes a **filtered preset** of Insights — same
component, pre-applied filter `kind ∈ {n_plus_one, blocking_in_async,
recursive}`. No parallel codepath. (Migration is in step 10; until then
Smells keeps its current implementation untouched.)

### 6.1 Where everything plugs in

All wiring is in [`App.tsx`](viewer/src/App.tsx):

- **`types.ts`** — add `Finding`, `FindingKind`, `Evidence`, `Severity`,
  `FindingTopRef`. Extend `CallTreeNode` with `findings?: Finding[]` and
  `Summary` with `findings_top?: FindingTopRef[]` and
  `findings_by_kind?: Record<string, number>`. All optional, so older
  fixtures keep loading.
- **`App.tsx:18`** — extend the `BottomTab` union:
  `'tree' | 'roots' | 'hot' | 'smells' | 'stats' | 'report' | 'insights'`.
- **`App.tsx:217-233`** (the tab strip) — add two `<Tab>` buttons, ordered:
  `Report · Tree · Roots · Hot Paths · Smells · Insights · Statistics`.
  The Report tab moves to the front because it's the natural landing.
- **`App.tsx:242-267`** (the render block) — add:
  ```tsx
  {bottomTab === 'report'   && <ScanReport report={report} onJump={jump} />}
  {bottomTab === 'insights' && <Insights   report={report} onJump={jump} />}
  ```
- New files:
  - `viewer/src/ScanReport.tsx`
  - `viewer/src/Insights.tsx`
- **`App.tsx:64-83`** (the fetch effect) — when a fresh fixture loads,
  if `report.summary.findings_by_kind` is present and non-empty, default
  `bottomTab` to `'report'` instead of `'tree'`. (The current default
  switches to `'roots'` when `entries.length ≥ 5`; we preserve that, just
  add the new branch.)

### 6.2 Page A: **Scan Report Summary** (`ScanReport.tsx`)

**Purpose.** One-screen, no-scroll-needed answer to "what did this scan
find?". A consultant's executive summary, not a data table. Every chunk
is **clickable to drill in** via the same `jump()` API.

**Layout (sketch):**

```
+---------------------------------------------------------------------------+
|  SCAN REPORT — .../automation-enrichements                                 |
|  drift-static-profiler 0.1.0 · python 78% · 142 files · 1,031 symbols      |
+---------------------------------------------------------------------------+
|                                                                            |
|  ┌─ HEALTH ────────────────┐  ┌─ FINDINGS ──────────────────────────────┐ |
|  │                          │  │                                          │ |
|  │       ██████████░        │  │   ●  high      8                         │ |
|  │       7.4 / 10            │  │   ◐  medium   19                         │ |
|  │                           │  │   ○  low      15                         │ |
|  │   Composite score          │  │                                          │ |
|  │   (severity-weighted)      │  │   by kind:                                │ |
|  │                           │  │     n_plus_one     7   ▓▓▓▓▓▓▓▓          │ |
|  └──────────────────────────┘  │     noisy_log      3   ▓▓▓                │ |
|                                  │     outdated_pkg   5   ▓▓▓▓▓              │ |
|  ┌─ CATEGORY REACH ─────────┐  │     memory_expl    4   ▓▓▓▓               │ |
|  │  db        ▓▓▓▓▓▓▓▓ 24   │  │     hot_zone      23  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓     │ |
|  │  network   ▓▓▓     8     │  └──────────────────────────────────────────┘ |
|  │  cache     ▓       2     │                                                |
|  │  io        ▓       3     │  ┌─ TOP HOT ZONES (click to jump) ─────────┐ |
|  │  log       ▓▓      6     │  │  ● OrderService.save_orders   order.py  │ |
|  │  queue                   │  │      reach 23% · pagerank 0.91          │ |
|  └──────────────────────────┘  │  ● OrderRepository.bulk_get   repo.py    │ |
|                                  │      reach 18% · pagerank 0.84          │ |
|  ┌─ LANGUAGE BREAKDOWN ─────┐  │  ◐ WebhookHandler.dispatch    wh.py      │ |
|  │  python    78%  ▓▓▓▓▓▓▓▓ │  │      reach 12% · pagerank 0.72          │ |
|  │  yaml      12%  ▓        │  └──────────────────────────────────────────┘ |
|  │  json       6%            │                                                |
|  │  shell      4%            │  ┌─ ENTRY POINTS ──────────────────────────┐ |
|  └──────────────────────────┘  │  12 roots (click to inspect)              │ |
|                                  │  • create_order      routes.py:42       │ |
|                                  │  • update_status     routes.py:88       │ |
|                                  │  • webhook_inbox     webhooks.py:12     │ |
|                                  │  ...                                     │ |
|                                  └──────────────────────────────────────────┘ |
+---------------------------------------------------------------------------+
```

**Cards (all read from `report.summary`, no new data computed in JS):**

| Card | Data source | Click action |
| --- | --- | --- |
| Header | `report.generator`, `summary.profiled_language`, `files`, `symbols` | — |
| Health score | derived from `findings_by_kind` weighted by severity (see §6.2.1) | scroll to Findings card |
| Findings breakdown | `summary.findings_by_kind`, `findings_top` severities | click row → Insights tab pre-filtered to that `kind` |
| Category reach | `summary.categories` (already there) | click category → flame `categoryFilter` |
| Language breakdown | `summary.language_breakdown` (already there) | — |
| Top hot zones | `summary.findings_top.filter(kind=hot_zone).slice(0,5)` | click → `jump({ id })` |
| Entry points | `report.entries[]`, sorted by `subtree_size` | click → set `activeRootId` + flip to Tree tab |

**6.2.1 Health score (informational, NOT a benchmark).** A composite
weighted sum, deliberately rough; we display the inputs side-by-side so
the user can see *why*:

```
score_raw = 10
  - findings_by_kind.* by severity:    high × 0.5, medium × 0.2, low × 0.05
  - capped at 0
```

We label the score "composite — for trend tracking" with a tooltip so no
one mistakes it for an objective health metric. The point is to give the
scan a single comparable number across runs (good for `make diff`).

**6.2.2 Why this is one tab, not the home screen.** The existing
`SummaryBar.tsx` is the always-visible header — it stays. The Scan Report
Summary is the *full-page* version a user opens deliberately. Both
coexist; we don't restructure the app.

### 6.3 Page B: **Insights** (`Insights.tsx`)

**Purpose.** The actionable findings list. Pre-sorted by
`severity DESC, percent_total DESC`. Filterable by kind and severity.

**Layout (sketch):**

```
+-------------------------------------------------------------------------------+
| INSIGHTS  ( 42 )                                  [ Kind ▼ ] [ Severity ▼ ]   |
+-------------------------------------------------------------------------------+
| SEV | KIND            | WHERE                            | MESSAGE            |
|-----|-----------------|----------------------------------|--------------------|
|  ●  | n_plus_one      | OrderService.save_orders :43     | session.add() inside `for o in orders` ... |
|  ●  | hot_zone        | OrderService.save_orders :38     | top 5% pagerank · 23% reach · db×3        |
|  ◐  | noisy_log       | WebhookHandler.dispatch :67      | DEBUG log on hot path                     |
|  ◐  | outdated_pkg    | http_client.py :1                | `requests` is sync — use httpx for async  |
|  ○  | memory_expl     | webhooks.py :88                  | response.json() loads full body            |
| …   | …               | …                                | …                                          |
+-------------------------------------------------------------------------------+
|  Selected finding ──────────────────────────────────────────────────────────  |
|  n_plus_one · severity HIGH · confidence 0.95                                  |
|  OrderService.save_orders @ src/services/order.py:43                           |
|                                                                                |
|  session.add() inside `for o in orders:` will issue one INSERT per element.    |
|  Evidence:                                                                     |
|    – session.add    line 43  db                                                |
|    – session.commit line 45  db                                                |
|                                                                                |
|  Remediation: Batch the inserts with session.bulk_save_objects(list).           |
|                                                                                |
|  [ Jump to node ]                                                              |
+-------------------------------------------------------------------------------+
```

**Data source.** The component receives `report` and walks
`report.entries[*]` to collect every node with `findings.length > 0`,
then expands one row per finding. (Alternatively, when present, use
`summary.findings_top` for the initial render and lazy-resolve into the
tree on click — but for 100s of findings, walking the tree once at mount
is cheap.)

**Click behavior.** Row click selects the finding (detail card).
"Jump to node" calls `jump({ id: nodeIdOfFinding })` — same API the
existing Smells tab uses. Header chips for `[ Kind ▼ ]` and
`[ Severity ▼ ]` are simple `<select>`s; URL state is not yet persisted.

### 6.4 Smells tab — becomes a filtered preset (step 10)

After Insights ships:
- `Smells.tsx` is rewritten as a 5-line wrapper:
  `<Insights report={report} preset={{ kinds: ['n_plus_one', 'blocking_in_async', 'recursive'] }} onJump={jump} />`.
- The Smells tab button stays where it is — same label, same count.
- The flame-mode `'smells'` color path keeps reading the existing
  booleans (`n_plus_one_risk`, `blocking_in_async`, `is_recursive`),
  which we continue to populate from `findings`.

### 6.5 Visual continuity (both new pages)
- Severity palette: `high = #e26d6d` (red, already in `CATEGORY_COLORS.db`),
  `medium = #e0a458` (orange, already in `CATEGORY_COLORS.io`),
  `low = #7e8189` (gray, already in `CATEGORY_COLORS.log`). No new colors.
- Reuse `useResizableColumns` for tables.
- Reuse the badge style from `DetailsPane`.
- Spacing/typography matches `Statistics.tsx`.

### 6.6 Empty / older-fixture states
- **Insights tab** with no findings:
  > "No insights for this scan. Either the scan is clean, or the scan
  > was produced by a version of drift that pre-dates this feature."
- **Scan Report Summary** with no `findings_by_kind`: the Findings and
  Top Hot Zones cards render in a "no findings yet" state; the Header,
  Category Reach, Language Breakdown and Entry Points cards still
  render normally from existing summary fields.

---

## 7. Implementation in micro-steps

Each step is small, individually committable, individually testable. Each
step ends with a passing `cargo test` + a working viewer. Steps are
ordered so the schema lands first, then the simplest end-to-end (one
detector + the viewer pages), then the rest of the detectors, then
polish.

| # | Step | Deliverable | Test |
| - | --- | --- | --- |
| 1 | **Schema shape** — add `Finding`, `FindingKind`, `Evidence`, `Severity` to `src/insights.rs` and the `schema/profile.schema.json`. Extend `CallTreeNode` with `findings: Vec<Finding>` and `Summary` with `findings_top` + `findings_by_kind`. Mirror in `viewer/src/types.ts`. **No detector logic yet.** | All three serialise as empty defaults. | Existing fixtures still validate against the schema. |
| 2 | **Wiring** — `tree::build_inner` calls a no-op `insights::collect_node_findings(...)` returning `vec![]` next to the existing Phase D booleans, assigning to the new `findings` field. `Summary::build` rolls up empty `findings_top` / `findings_by_kind`. | Scan emits `"findings": []` per node and empty summary rollups. | `cargo test` green; fixture diff = only adds empty fields. |
| 3 | **Severity helper** — `bump_by_impact(&CallTreeNode, base, pagerank_p90) -> Severity` in `src/insights.rs`. Pagerank-p90 computed once in `Report::build`. | Unit tests on synthetic nodes. | `cargo test insights::severity`. |
| 4 | **First real detector — `n_plus_one`** — emit `FindingKind::NPlusOne` from `detect_loop_smells()` whenever an `external_call.in_loop && category ∈ {db, cache}`. Derive the existing `n_plus_one_risk: bool` field from `findings`. | Existing N+1 cases now have BOTH the bool (unchanged behavior) and a structured finding with evidence + remediation. | Snapshot assertion against `python-fastapi` fixture: ≥ 1 finding with `kind == "n_plus_one"` at the known site; the existing `n_plus_one_risk == true` assertion still passes. |
| 5 | **Viewer: Insights tab v0** — new file `viewer/src/Insights.tsx`. Walks `report.entries[*]` collecting nodes with `findings.length > 0`, renders one row per finding, "Jump to node" via existing `jump({ id })`. Add the tab in `App.tsx`. | Tab shows up; clicking a row navigates to the offending node in the call tree. | Manual smoke against `python-fastapi`; `bun run build` clean. |
| 6 | **Viewer: Scan Report Summary tab** — new file `viewer/src/ScanReport.tsx`. Cards: Header / Health / Findings / Category Reach / Language Breakdown / Top Hot Zones / Entry Points. Tab placed first in the strip; default landing when `findings_by_kind` is non-empty. | Loading any fixture shows a one-screen report; every card is read-only against existing `summary` fields plus the new `findings_*` fields. | Manual smoke against all built-in fixtures; `bun run build` clean. |
| 7 | **Detector — `blocking_in_async` + `recursive`** — emit structured `Finding`s for the other two existing booleans. Same derive-bool-from-findings pattern. | Three families of structured findings now coexist with the existing booleans. | Snapshot. |
| 8 | **Detector — `noisy_log`** — log-in-loop, log-on-hot-path, log-in-recursive. New fixture `tests/fixtures/insights-logs/`. | Findings emitted with evidence + remediation. | Snapshot. |
| 9 | **Detector — `outdated_package`** — seed `src/research_classefiers+categories/outdated_packages.json` with ~30 entries across 3 languages. Loader uses `include_str!`. New fixture `tests/fixtures/insights-packages/`. | Findings emitted per matched import; severity bumped on hot path. | Snapshot. |
| 10 | **Detector — `memory_explosion`** — seed `memory_explosion.json` catalog. Two rules: catalog-match externals, unbounded recursion. New fixture. | Findings emitted; clearly marked as heuristic via `confidence`. | Snapshot. |
| 11 | **Detector — `hot_zone`** — post-build pass `attach_hot_zones()` in `src/insights.rs` from `Report::build`. Cross-references the other findings. | Top hot zones appear as `HotZone` findings, sorted high in Insights and in the Scan Report Summary's "Top Hot Zones" card. | Snapshot. |
| 12 | **Insights tab v1** — `kind` and `severity` filter dropdowns; detail card pane; sort header. | Full UX. | Manual checks against every fixture. |
| 13 | **Smells migration** — rewrite `Smells.tsx` as a wrapper around `Insights` with a preset filter. Smells tab still works identically; flame-mode `'smells'` still reads the booleans. | One fewer codepath; same user-visible behavior. | Manual smoke. |
| 14 | **Docs & opt-out** — stubbed docs at `drift.dev/docs/insights/<kind>` (linkable from each finding); CLI flag `--no-insights` for opt-out (skips detectors entirely; `findings` stays empty). | Flag works; doc pages exist. | `cargo test`; manual. |

After step 14, iterate: more detectors, more catalog entries, deeper checks.
Each is its own micro-step.

---

## 8. Open research questions to revisit before/after v1

These are intentionally **not blocking** for step 1. We will research them
in parallel as the catalog grows and as we look at more fixtures:

1. **Loop-invariant code motion (LICM)**. PyCharm finds these via runtime
   timings. Statically, we'd need a small dataflow pass — defer.
2. **Memory ceilings per language**. Is `list(iter)` always bad? No — only
   if `iter` is unbounded or streaming. Need an "iterator-source" taxonomy:
   socket.iter, file.iter, generator, list comprehension over collection.
3. **Confidence calibration.** After we ship and look at real-world repos,
   record true-positive rates per kind and adjust the catalog accordingly.
4. **CVE feed integration.** For `outdated_package`, ideally we cross-link
   to OSV / NVD by package name + version. But we don't parse lockfiles
   today. Future: read `requirements.txt`, `package-lock.json`, `go.sum`
   and emit findings with CVE IDs.
5. **Cross-tool exports.** SARIF export of the `insights` array would make
   the report consumable by GitHub Code Scanning. Out of scope for v1.
6. **Diff mode.** `make diff` already exists. Add `insights_added` and
   `insights_resolved` deltas so regressions show up in PRs.

---

## 9. Acceptance checklist for the plan

- [x] Defines the new JSON shape as a **per-node extension**
      (`CallTreeNode.findings`) + **summary rollups** (`findings_top`,
      `findings_by_kind`), not a parallel top-level array (§3).
- [x] Lists five detector families with detect-signal + severity +
      FP-risk + micro-step, all running inline in `tree::build_inner`
      except `hot_zone` which is one small post-build pass (§4).
- [x] Severity helper takes `&CallTreeNode` directly — no
      `ImpactSignals` struct (§5).
- [x] **Two** new viewer pages specified with ASCII sketches and
      `App.tsx` integration points: Scan Report Summary (§6.2) and
      Insights (§6.3). Smells becomes a filtered preset of Insights
      (§6.4), not a parallel codepath (§6.4).
- [x] Existing booleans (`n_plus_one_risk`, `blocking_in_async`,
      `is_recursive`) stay populated as derived convenience values; old
      consumers and flame-mode `'smells'` unchanged (§3.4, §2.7).
- [x] Module layout: flat `src/insights.rs`, catalogs in existing
      `src/research_classefiers+categories/` folder (§2.7).
- [x] Cites the four web sources read during research (§1).
- [x] Sequences the work into 14 small, testable steps with Step 1 =
      schema-only-empty-defaults so old fixtures keep validating (§7).
- [x] Lists open questions explicitly (§8).

When you say "go", step 1 begins.
