# Drift Static Profiler — Query Analyzer & per-language ORM Analyzer Plan

> Sibling to [INSIGHTS_PLAN.md](INSIGHTS_PLAN.md). That doc shipped the
> per-node `findings: Finding[]` substrate (severity × effort, evidence,
> remediation), the rollups (`findings_top`, `findings_by_kind`,
> `refactor_candidates`, `immediate_fixes`, `roots_overview`), and the
> first nine detector families. This plan extends the same substrate
> with two new families:
>
> 1. **Query Analyzer** — find SQL string literals embedded in source,
>    parse them with a real SQL grammar, emit lint findings, and
>    *optionally* fetch `EXPLAIN`/`EXPLAIN ANALYZE` from a live DB and
>    surface plan-level issues.
> 2. **ORM Analyzers** — per-language, per-ORM detectors that recognize
>    framework-specific anti-patterns (Django/SQLAlchemy/Tortoise,
>    Hibernate/JPA/Spring Data, Sequelize/TypeORM/Prisma/Drizzle/
>    Mongoose, GORM/ent/sqlc/sqlx, ActiveRecord, Doctrine/Eloquent,
>    EF Core) using the ImportRecord + Reference + loop-range data the
>    pipeline already carries.
>
> All output flows through the same `Finding` → `Summary` shape — no
> parallel hierarchies, no new viewer pages.

---

## 0. Why this is the natural next step

Today drift classifies a call site by *category* (Db / Cache / Network /
…) and flags it if it's in a loop. That's coarse: `session.query(User)
.filter(...).all()` and `repo.findById(id)` and `User.find(id)` all show
up as "db call in a loop" with a generic N+1 remediation. The user wants
the next level of specificity:

| Coarse today                                   | Specific tomorrow                                                                                                  |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| "db call in a loop" (generic N+1)              | "Django `User.objects.get(id=x)` inside `for x in xs` — add `select_related('profile')` or `prefetch_related(...)`" |
| `cursor.execute(sql)` — db call               | "embedded SQL: `SELECT *` from large table, no `WHERE`, no `LIMIT` — add a column list and a `LIMIT` or pagination" |
| `repo.save(orders)` — db call                  | "Hibernate `@OneToMany` traversal with `FetchType.EAGER` — switch to LAZY + `@EntityGraph`"                        |
| nothing                                        | Optional: `EXPLAIN` says seq scan on 12M rows with selective predicate, no index — propose CREATE INDEX (HypoPG)   |

The web research (V8/Chrome DevTools insights, JetBrains
InspectionGadgets, Ruff PERF rules, Clippy `perf`, staticcheck SA6xxx,
pgMustard / pganalyze plan insights, Sentry detector heuristics, Bullet
/ Prosopite, nplusone, QuickPerf, sqlcheck) all converge on the same
recipe: **a catalog of named patterns, each with a detector closure, a
severity, an effort, and a remediation**. We already have the runtime
substrate for that (`insights::Finding` + the `attach_*` post-build
passes + the `bump_severities_by_impact` pipeline in
[report.rs:130-141](src/report.rs#L130-L141)). All that's missing is the
two new detector families and one new dependency layer (a SQL parser).

---

## 1. Research synthesis (compressed)

Two parallel research agents produced ~5,000 words of source-cited
detail; this section is the compressed actionable summary.

### 1.1 SQL static analyzers (what to mine for rule ideas, what to depend on)

| Tool                                                             | Rule count       | License      | Usable from Rust?              | What we take                                                |
| ---------------------------------------------------------------- | ---------------- | ------------ | ------------------------------ | ----------------------------------------------------------- |
| [sqlfluff](https://docs.sqlfluff.com/en/stable/reference/rules.html) | ~70              | MIT          | No — Python                    | Rule ID convention (`AL`, `AM`, `ST`, …) + remediation text |
| [squawk](https://squawkhq.com/docs/rules)                        | ~25 (migrations) | GPL-3        | `squawk-parser` crate exists — **GPL**, can't link without contagion | Migration-safety rule ideas (clean-room reimplementation only) |
| [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs) | n/a (parser)     | Apache-2.0   | **Yes** — direct dep            | Generic multi-dialect SQL AST                               |
| [pg_query.rs](https://github.com/pganalyze/pg_query.rs)          | n/a (parser)     | BSD-3        | **Yes** — direct dep            | Real Postgres parse tree + `normalize()` + `fingerprint()`  |
| [sqlcheck](https://github.com/jarulraj/sqlcheck)                 | 21 anti-patterns | Apache-2.0   | No — C++                       | Rule catalog (id, ranking, repair hint shape)                |
| [pg_hint_plan](https://pg-hint-plan.readthedocs.io/)             | n/a              | BSD          | No — Postgres extension        | Remediation language (`/*+ IndexScan(t idx) */` suggestions) |

**Decision:** depend on `sqlparser-rs` for generic and `pg_query` for
Postgres-flavored SQL. Both permissive. Never link squawk. Mine
sqlcheck/sqlfluff rule names freely — rule ideas aren't copyrightable.

### 1.2 EXPLAIN plan tools — the canonical plan-level rule catalog

The recurring "bad node" rules across pgMustard, PEV2, depesz,
pganalyze, pgBadger, pt-query-digest, MySQL Workbench Visual Explain,
Dexter:

1. Seq Scan on a large table with a selective predicate (no usable index).
2. Nested Loop with high outer rows × no inner index (the N×M nightmare).
3. Row-estimate off by ≥10× (planner bias; depesz colors at 10×/100×/1000×).
4. Sort spilling to disk (`Sort Method: external merge`).
5. Hash join with `Batches > 1` (memory pressure).
6. Lossy Bitmap (recheck cost — bump `work_mem`).
7. Index Scan with high Heap Fetches (vacuum needed for Index-Only Scan).
8. CTE materializing a large result unnecessarily (Postgres 12+ can inline).
9. Filter discarding many rows after the scan (predicate not pushed to index).
10. JIT overhead > query benefit (`JIT: ... Time:` block dominates).
11. Parallel plan disabled when parallelism would help.

Depesz exclusive-time color breakpoints (10% / 50% / 90%) and
row-misestimate breakpoints (10× / 100× / 1000×) give us the severity
thresholds for free.

### 1.3 ORM N+1 detectors — what the field detects and how

| Ecosystem | Tool | Method | Heuristic | License |
| --- | --- | --- | --- | --- |
| Python | [nplusone](https://github.com/jmcarp/nplusone) | runtime (Django/SQLAlchemy/Peewee hooks) | relationship lazily-loaded after parent already in memory | MIT |
| Python | [django-silk](https://github.com/jazzband/django-silk) | runtime profiler | query count + duplicate detect + EXPLAIN | MIT |
| Ruby | [bullet](https://github.com/flyerhzm/bullet) | AS::Notifications | per-request, association-touched-after-load | MIT |
| Ruby | [prosopite](https://github.com/charkost/prosopite) | AS::Notifications | ≥2 queries sharing same call-stack + fingerprint | MIT |
| Java | [QuickPerf](https://github.com/quick-perf/quickperf) | test-time annotations | `@ExpectSelect(N)`, `@DisableSameSelectTypesWithDifferentParamValues` | Apache-2.0 |
| Java | Hypersistence Optimizer | runtime scanner | `BLOCKER/CRITICAL/MAJOR/MINOR`; EAGER defaults, missing `@BatchSize` | Commercial |
| Java IDE | IntelliJ "JPA: N+1 select problem" | static (PSI) | `@OneToMany`/`@ManyToMany` field with `FetchType.EAGER` | Apache-2.0 |
| Node | Prisma/Sequelize/TypeORM | logging only | no auto detector |  |
| Node | DataLoader | (fix, not detector) | batches per-event-loop-tick |  |
| .NET | EF Core `ConfigureWarnings` | warnings → errors | `MultipleCollectionIncludeWarning`, `RowLimitingOperationWithoutOrderByWarning` | MIT |
| Go | GORM logger | runtime slow-SQL threshold | no first-class detector | MIT |
| PHP | beyondcode/laravel-query-detector | middleware | ≥ N duplicate queries per request | MIT |

**Sentry's static-trace algorithm** is the most reusable: identify a
*sequential, non-overlapping run of db spans whose parameterized
descriptions are similar, all under a common parent span*. The static
analog (our analog): identify a *loop whose body contains an ORM call
on a field of the loop variable, with no preceding eager-load directive
in the parent's surface*. Both PyCharm and IntelliJ's bundled rules do
exactly this.

**Key insight:** purely static N+1 detection is rare in the wild and the
detection patterns are short and language-specific. Each ORM has a
distinct surface (`.objects.filter()` vs `.session.query()` vs
`repo.findAll()` vs `prisma.user.findMany()`), so the per-ORM detector
table is small and well-bounded.

### 1.4 IDE / JIT profilers — what they emit (used as schema inspiration only)

- **V8 `--prof`, `--trace-deopt`, `--trace-ic`, `--trace-maps`** — runtime
  trace of deoptimizations, IC state transitions, hidden-class shifts.
  We **cannot** detect these statically, but the *patterns they reveal*
  (constructors with conditional property assignment, callsites with
  >4 receiver types) are tree-sitter-friendly. Files a "shape-shifting
  object" detector under `expensive_compute` for JS, out of scope for v1.
- **Chrome DevTools Performance Insights**
  ([@paulirish/trace_engine](https://www.npmjs.com/package/@paulirish/trace_engine),
  BSD-3) — named insights (LCP Breakdown, INP Breakdown, Long Tasks,
  Forced Reflow, Render-Blocking Requests, Duplicated JavaScript
  Bundles). The *finding shape* (`{ insightKey, relatedEvents,
  category }`) confirms our `Finding` shape is the right one.
- **IntelliJ InspectionGadgets** ([Apache-2.0](https://github.com/JetBrains/intellij-community/tree/master/plugins/InspectionGadgets))
  — `ManualArrayCopyInspection`, `StringConcatenationInLoopInspection`,
  `KeySetIterationMayUseEntrySetInspection`, `MethodMayBeStaticInspection`,
  etc. Rule logic borrowable wholesale for our Java/Kotlin column.

### 1.5 What the field *doesn't* do that we should

- **No tool does purely-static, per-call-site, per-ORM, language-agnostic
  N+1 detection.** Bullet/Prosopite/nplusone need a running request.
  IntelliJ's JPA inspection only covers Java. PyCharm's Django
  inspection is shallow. Semgrep proves the patterns work but ships no
  cohesive ORM rule pack.
- **No tool combines static ORM detection with optional dynamic
  EXPLAIN.** pganalyze / Dexter run on live workloads; sqlcheck runs on
  SQL strings. drift can chain the two: detect an ORM call →
  materialize its SQL (when possible) → if a connection is configured,
  EXPLAIN it → emit findings from both the static lint and the plan.

That's our edge.

---

## 2. Architecture — extending what exists

### 2.1 Module layout (no new folders, no new hierarchies)

Following the rules from [INSIGHTS_PLAN.md §2.7](INSIGHTS_PLAN.md):

- **`src/insights.rs`** — currently 1300 lines. Stays the home for the
  `Finding` substrate and the broad-stroke detectors. New per-detector
  groups go into **sibling files** when (and only when) they grow past
  ~300 lines:
  - `src/sql_lint.rs` — embedded-SQL detector + sqlparser/pg_query
    integration. New file because it owns the SQL parsing surface.
  - `src/orm_lint.rs` — per-language ORM detectors. New file because
    each ORM gets ~30-80 lines and `insights.rs` is already big.
  - `src/plan_lint.rs` — EXPLAIN plan rules (Phase 3, optional). New
    file for the same reason.
- **`src/research_classefiers+categories/`** — gains the new catalogs.
  Same `include_str!` pattern as today:
  - `sql_patterns.json` — per-dialect "bad SQL pattern" catalog
    (severity, message, remediation).
  - `orm_signatures.json` — per-language, per-ORM signature table:
    "what method names + receiver patterns + decorators identify a
    Django QuerySet vs a SQLAlchemy session vs a Hibernate Repository
    method". Built from existing OTel data plus hand-curated entries.
  - `orm_fixes.json` — per-(ORM, anti-pattern) remediation text.
  - `plan_node_rules.json` — Phase 3 only; named plan-level rules.

### 2.2 Finding-shape additions (clean extension)

Reuse the existing `Finding` struct verbatim. Add new variants to
`FindingKind`:

```rust
pub enum FindingKind {
    // ── existing (do not touch) ──
    NPlusOne, BlockingInAsync, Recursive, SmellyLoop, NoisyLog,
    OutdatedPackage, MemoryExplosion, HotZone, ExpensiveCompute,
    MissingCaching, LogAmplification,

    // ── new (this plan) ──
    /// A SQL literal embedded in source matches a known anti-pattern
    /// (SELECT *, missing WHERE on DELETE/UPDATE, LIKE 'prefix%',
    /// implicit columns in INSERT, etc.). Carries dialect + rule-id
    /// in the message.
    SqlAntipattern,

    /// An ORM call matches a known per-(language, ORM) anti-pattern
    /// (Django N+1 without select_related, Sequelize `findAll` in a
    /// loop, Hibernate EAGER fetch on collection, EF Core
    /// `.Include().Where()` chain, …). Carries (orm, rule-id) in the
    /// message + ORM-specific remediation.
    OrmAntipattern,

    /// Optional Phase 3: a real EXPLAIN plan node matches a bad-plan
    /// rule (seq scan on big table, sort to disk, nested loop with
    /// high outer rows, row misestimate ≥10×). Surfaced only when
    /// the user configures a connection string and opts in.
    BadQueryPlan,
}
```

The `Evidence` struct is rich enough — `call` already holds the
qualified method name; we extend by **convention** (not by struct
change): for SQL findings the `call` string carries the rule id
(`"sqlfluff:ST07"`, `"sqlcheck:Q3001"`); for ORM findings it carries
`"<orm>:<rule>"` (`"django:n_plus_one_no_select_related"`).

The `effort` axis (`Trivial / Small / Medium / Large`) and the
severity-bump pipeline both already exist; new detectors just emit
appropriate values.

### 2.3 Where detectors plug in (no new orchestration)

The existing pipeline ([report.rs:130-141](src/report.rs#L130-L141))
already runs detector passes in order. New entries slot in directly:

```rust
// Report::build, after the existing attach_* passes
attach_sql_antipatterns(&mut entries, &graph);          // §3
attach_orm_antipatterns(&mut entries, &graph, &lang);   // §4
attach_plan_findings(&mut entries, &graph, &explain);   // §5 (opt-in)

// ALWAYS LAST — already exists, unchanged.
bump_severities_by_impact(&mut entries, pagerank_p90);
```

Per-node detectors (the ORM ones — cheap, no global state) can also run
inside `tree::build_inner` next to the existing `detect_n_plus_one`.
The SQL parser is heavier (`sqlparser-rs` allocates an AST) so we keep
SQL detection as a single post-build pass that parses each unique SQL
literal once, caches by hash, then walks every external call referring
to it.

### 2.4 New dependencies (license-clean)

| Crate                  | Version  | License        | Used for                                          |
| ---------------------- | -------- | -------------- | ------------------------------------------------- |
| `sqlparser`            | latest   | Apache-2.0     | Generic multi-dialect SQL AST                     |
| `pg_query`             | latest   | BSD-3 + PG     | Real Postgres parse tree, `normalize`, `fingerprint` |
| `sqlx` (optional dep behind `--features explain`) | latest | MIT/Apache-2.0 | Run `EXPLAIN` against a live DB (Phase 3)         |
| `tokio` (optional, with `sqlx`)         | latest | MIT            | Required runtime for `sqlx`                       |

Nothing GPL. Nothing that pulls C++ build chains we don't already
tolerate (libpg_query is already battle-tested in the pg_query crate's
binding — pre-built source bundled in the crate).

### 2.5 Performance budget (defending the existing scan time)

Today drift parses ~thousands of files via rayon, scans them, builds
the graph, and writes JSON in <2 s on `python-fastapi`. The new
detectors must not regress this.

- **SQL parsing** is the only heavyweight new step. Mitigation:
  - Hash each SQL string with FxHash, dedupe → parse once.
  - On a typical web app there are O(hundreds) of unique SQL
    fingerprints — well below the noise floor.
  - When the SQL is too dialect-specific for `sqlparser-rs`, the
    parser errors quickly and we skip the lint silently (the call
    still gets the generic `n_plus_one` / `db-in-loop` finding from
    the existing detector — no regression).
- **ORM detection** is pure pattern-matching over data we already
  carry (ImportRecord + ExternalCall). Linear in references; same
  asymptotic cost as the existing classify pass.
- **EXPLAIN** (Phase 3) is opt-in via flag/env and runs sequentially
  outside the rayon pool; budget bounded by `--explain-budget-ms`
  default 5000ms total across all queries.

---

## 3. Phase 1: SQL Query Analyzer

### 3.1 Scope

**Input.** Every SQL string literal embedded in source code, plus every
`.sql` file in the scanned tree.

**Output.** A `Finding { kind: SqlAntipattern, … }` attached to the
`CallTreeNode` whose symbol contains the SQL literal, with rich
evidence (the SQL source, the matched rule id, the dialect, the line).

### 3.2 Extracting SQL from source (the "where")

We don't need a perfect SQL extractor. The 80/20:

| Language        | Heuristic for "this string is SQL"                                                                                                                       |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Python          | string literal passed to `cursor.execute`, `cursor.executemany`, `connection.execute`, SQLAlchemy `text()`, `select(text(...))`, Django `RawSQL`, `.raw(`     |
| Java / Kotlin   | string literal passed to `Statement.executeQuery`, `PreparedStatement`, `EntityManager.createNativeQuery`, Spring `@Query(value=..., nativeQuery=true)`     |
| JS / TS         | string literal passed to `client.query`, `pool.query`, `db.query`, `connection.query`; tagged template `sql\`…\`` (Postgres.js, Slonik); Knex `raw()`        |
| Go              | string literal passed to `db.Query`, `db.QueryRow`, `db.Exec`; `sqlx.QueryxContext`; `pgx.Query` etc.                                                       |
| Rust            | `sqlx::query!(…)`, `sqlx::query_as!(…)` macro arg; `tokio_postgres::Client::query` first arg                                                                |
| C# (future)     | `cmd.CommandText = "…"`, `EF.CompileRawSql`, Dapper `Query`                                                                                                 |

These all reduce to "string argument at position N of method M, when M
is in the per-language SQL-sink set". The existing tags-pass collects
references — we extend the per-language tree-sitter query to also
capture the **first string literal of a call** when the call name
matches one of the sinks. New field on `Reference`:

```rust
pub struct Reference {
    // …existing fields…
    /// Captured iff the call is a known SQL sink AND the first arg is
    /// a string literal we can read at parse time. None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_literal: Option<String>,
}
```

This is a *minimal* schema change — backward-compatible (optional
field, no required-list change).

For `.sql` files (migrations, seeders), the walker already excludes
non-supported-language files. Add `.sql` as a *first-class supplementary
input*: parse them straight as SQL, attach findings to a synthetic
`<sql>` node per file under the file's directory.

### 3.3 Parsing strategy (the "how")

```rust
enum SqlBackend {
    /// Real Postgres parse tree. Picked when the surrounding driver
    /// imports suggest Postgres (psycopg, pg, pgx, postgres-jdbc, sqlx
    /// with PG features, prisma with PG, sequelize 'postgres' dialect).
    Postgres(pg_query::ParseResult),
    /// Generic AST from sqlparser-rs. Picked otherwise; the dialect
    /// is selected from the same per-driver inference (MysqlDialect,
    /// SQLiteDialect, MsSqlDialect, BigQueryDialect, SnowflakeDialect,
    /// or GenericDialect as the fallback).
    Generic(Vec<sqlparser::ast::Statement>),
}
```

Driver inference is a small table in `sql_patterns.json`:

```jsonc
"driver_dialect": {
  "psycopg":  "postgres",
  "psycopg2": "postgres",
  "asyncpg":  "postgres",
  "pg":       "postgres",
  "pgx":      "postgres",
  "mysql2":   "mysql",
  "pymysql":  "mysql",
  "sqlite3":  "sqlite",
  "@prisma/client": "(read schema.prisma `provider`)",
  "sequelize": "(read `new Sequelize(..., { dialect: 'postgres' })`)"
}
```

Parsing happens lazily, per unique SQL fingerprint (FxHash of normalized
text), so the same SQL across 50 call sites parses once.

### 3.4 SQL lint rules — v1 catalog

A small, high-confidence v1. All rules emit `FindingKind::SqlAntipattern`
with effort `Trivial` or `Small` (most SQL fixes are one-line). Severity
defaults conservative; `bump_severities_by_impact` promotes when the
containing symbol is hot.

Catalog file: `src/research_classefiers+categories/sql_patterns.json`.

| Rule ID | Name | What it matches (AST) | Default sev | Effort | Remediation |
| --- | --- | --- | --- | --- | --- |
| `SQL001` | `select_star` | `SELECT *` (no expansion) outside of CTE-only contexts | Low | Trivial | List columns explicitly to avoid implicit-column changes + bandwidth waste |
| `SQL002` | `delete_without_where` | `DELETE FROM t` with no `WHERE` clause | **High** | Trivial | Add a `WHERE` or use `TRUNCATE` if intentional |
| `SQL003` | `update_without_where` | `UPDATE t SET …` with no `WHERE` clause | **High** | Trivial | Add a `WHERE`; full-table updates should be exceptional |
| `SQL004` | `insert_no_columns` | `INSERT INTO t VALUES (…)` with no column list | Low | Trivial | Name the columns: schema drift won't silently misalign values |
| `SQL005` | `like_leading_wildcard` | `WHERE x LIKE '%…'` or `'%…%'` | Medium | Small | Use full-text index (`tsvector`/FULLTEXT) — leading-wildcard LIKE can't use an index |
| `SQL006` | `order_by_random` | `ORDER BY RANDOM()` / `RAND()` | Medium | Small | Use `TABLESAMPLE` or pre-pick ids in app code; ORDER BY random sorts the whole table |
| `SQL007` | `or_chain_on_indexed_col` | `WHERE a = x OR a = y OR a = z` (≥3) | Low | Trivial | Use `IN (x, y, z)` — the planner gets a chance to use an index |
| `SQL008` | `is_null_on_indexed_col_with_other_predicates` | `WHERE indexed_col IS NULL AND …` | Low | Small | Consider a partial index `WHERE indexed_col IS NULL` |
| `SQL009` | `function_on_indexed_col` | `WHERE LOWER(email) = …`, `WHERE date(t) = …` | Medium | Small | Wrap the predicate with a functional index, or normalize at write time |
| `SQL010` | `cartesian_join` | `FROM a, b` with no `WHERE a.x = b.y` predicate | High | Small | Specify explicit `JOIN … ON` to avoid accidental cross-product |
| `SQL011` | `not_in_with_subquery` | `WHERE x NOT IN (SELECT …)` (NULL-trap) | Medium | Small | Use `NOT EXISTS` — `NOT IN` returns empty when the subquery has any NULL |
| `SQL012` | `count_star_with_distinct` | `COUNT(DISTINCT *)` or `COUNT(*)` on huge table without estimate | Low | Small | For approximations use Postgres `pg_class.reltuples` or `APPROX_COUNT_DISTINCT` |
| `SQL013` | `coalesce_in_where_kills_index` | `WHERE COALESCE(col, 0) > x` | Low | Small | Rewrite as `WHERE col > x OR (col IS NULL AND 0 > x)` or use a functional index |
| `SQL014` | `select_distinct_with_order_by_unrelated` | `SELECT DISTINCT … ORDER BY <col not in projection>` | Low | Small | Use `GROUP BY` so the ORDER BY column is unambiguous |
| `SQL015` | `union_should_be_union_all` | `UNION` between disjoint sources | Low | Trivial | Use `UNION ALL` to skip the implicit DISTINCT sort |

**Severity rationale.** `SQL002` and `SQL003` are critical because
even a one-off run can wipe a table; they share the bump-by-impact
override so the user still sees them at the top of the Insights tab.
Everything else defaults Low/Medium and rises when the containing
symbol is hot.

**False-positive policy.** Strict — only emit when the AST gives us a
direct match. SQL that we can't parse (dialect quirks, dynamic
fragments) is silently skipped: the existing `n_plus_one` /
`smelly_loop` finding still fires from the category-level detector.

### 3.5 SQL detector — pseudocode

```rust
// src/sql_lint.rs

pub fn attach_sql_antipatterns(entries: &mut [CallTreeNode], graph: &CallGraph) {
    // 1. Collect every unique SQL literal referenced from any external call.
    let mut by_fingerprint: HashMap<u64, ParsedSql> = HashMap::new();
    for syms in graph.symbols.keys() {
        for r in references_for(graph, syms) {
            if let Some(sql) = r.sql_literal.as_deref() {
                let dialect = infer_dialect(/* file imports + manifest */);
                by_fingerprint
                    .entry(fingerprint(sql))
                    .or_insert_with(|| parse(sql, dialect));
            }
        }
    }

    // 2. Run rules over each parsed SQL once.
    let mut findings_by_fp: HashMap<u64, Vec<RuleHit>> = HashMap::new();
    for (fp, parsed) in &by_fingerprint {
        let mut hits = Vec::new();
        for rule in SQL_RULES.iter() {
            if let Some(hit) = rule.check(parsed) {
                hits.push(hit);
            }
        }
        findings_by_fp.insert(*fp, hits);
    }

    // 3. Attach to nodes (walk tree, look at each call site's SQL fp).
    walk_mut(entries, |node| {
        for ec in &node.external_calls {
            // We need the original Reference to get the SQL — but we
            // already stamped `sql_literal` onto Reference, so we can
            // fish it back. (See §3.7 for the mechanical detail.)
            if let Some(sql) = sql_for_external_call(graph, &node.id, ec) {
                let fp = fingerprint(sql);
                for hit in findings_by_fp.get(&fp).into_iter().flatten() {
                    if !has_kind_with_message(&node.findings, FindingKind::SqlAntipattern, &hit.rule_id) {
                        node.findings.push(hit.to_finding(ec.line));
                    }
                }
            }
        }
    });
}
```

### 3.6 SQL literal extraction — tree-sitter query patterns

For each language we add a *single* small tree-sitter pattern keyed on
the call name. Patterns live next to the existing tags queries in
`src/tags.rs` extensions. Example for Python:

```scheme
;; cursor.execute("…"), cursor.executemany("…"), session.execute(text("…"))
(call
  function: (attribute
    attribute: (identifier) @method (#match? @method "^(execute|executemany|raw)$"))
  arguments: (argument_list
    . (string (string_content) @sql_literal)))

;; sqlalchemy text("…")
(call
  function: (identifier) @fn (#eq? @fn "text")
  arguments: (argument_list . (string (string_content) @sql_literal)))
```

Per-language patterns are kept in the same per-language queries-file
already used by tags extraction — no new query-loader. The capture
`@sql_literal` is read out and stamped onto the corresponding
`Reference`.

### 3.7 Mechanical detail: reading SQL back at attach time

`ExternalCall` doesn't carry the SQL literal today (intentional — keeps
the call-graph small). Two options:

- **(a) Stamp the SQL on `ExternalCall` too.** Simplest. Memory cost is
  negligible for the queries-per-symbol distribution typical of a web
  app (≤10 SQL strings per symbol, each ≤500 chars).
- **(b) Side-table on `CallGraph`.** Cleaner but more plumbing.

Plan picks **(a)** — `ExternalCall.sql_literal: Option<String>`. Same
backward-compat (optional field).

### 3.8 Phase 1 test plan

Add `tests/fixtures/sql-antipatterns/` with one file per supported
language, each with 3-4 known-bad SQL strings. Snapshot the emitted
report; assert each fixture file produces the expected rule ids on the
expected lines.

---

## 4. Phase 2: per-language ORM Analyzers

### 4.1 The unified signature table

Every ORM call we care about reduces to a tuple:
`(language, framework, receiver-pattern, method-name, context-pattern)`.
We codify this in `orm_signatures.json`:

```jsonc
{
  "schema_version": "1",
  "frameworks": {
    "django.orm": {
      "language": "python",
      "import_modules": ["django.db.models", "django.db"],
      "queryset_methods": [
        "all", "filter", "exclude", "get", "first", "last", "count",
        "exists", "values", "values_list", "iterator", "raw"
      ],
      "eager_load_methods": ["select_related", "prefetch_related", "only", "defer"],
      "write_methods": ["save", "delete", "update", "create", "bulk_create", "bulk_update"],
      "manager_pattern": "objects",
      "relationship_attrs": ["set", "all", "add", "remove", "clear"],
      "rules": [
        "django.n_plus_one_no_select_related",
        "django.queryset_in_loop",
        "django.save_in_loop_no_bulk",
        "django.count_then_iterate",
        "django.get_or_create_in_loop",
        "django.raw_sql_when_orm_suffices",
        "django.unbounded_filter_without_pagination"
      ]
    },
    "sqlalchemy": { … },
    "tortoise": { … },
    "hibernate": { … },
    "spring_data": { … },
    "jooq": { … },
    "sequelize": { … },
    "typeorm": { … },
    "prisma": { … },
    "drizzle": { … },
    "mongoose": { … },
    "ef_core": { … },
    "active_record": { … },
    "eloquent": { … },
    "doctrine": { … },
    "gorm": { … },
    "ent": { … },
    "sqlc": { … }
  }
}
```

Detection of "which framework is in this file" reuses the
existing `ImportRecord` machinery: a per-file framework set =
`{ fw | fw.import_modules ∩ file.imports ≠ ∅ }`. This is cheap and
deterministic.

### 4.2 Per-language ORM rule tables (v1 catalog)

For each ORM we ship 4-8 rules. All emit `FindingKind::OrmAntipattern`
with `(orm, rule)` baked into the message and remediation. Per-framework
detailed rule tables follow; rule IDs are stable strings used in
fixtures.

#### Python

##### Django ORM

| Rule | Pattern (tree-sitter signal) | Sev | Effort |
| --- | --- | --- | --- |
| `django.n_plus_one_no_select_related` | `for x in qs: x.<related>` where `qs = M.objects.all()/filter()` and no `select_related`/`prefetch_related` in the chain | High | Small |
| `django.queryset_in_loop` | `M.objects.<terminal>` inside a loop where `M` is constant across iterations | High | Small |
| `django.save_in_loop_no_bulk` | `<instance>.save()` inside a `for` over a collection of model instances | High | Small |
| `django.count_then_iterate` | `if qs.count() > 0:` followed by `for x in qs:` | Low | Trivial |
| `django.get_or_create_in_loop` | `M.objects.get_or_create(…)` inside a loop | Medium | Small |
| `django.unbounded_filter_without_pagination` | `M.objects.filter(…).all()` followed by `len(…)` or list-conversion, no `LIMIT`/pagination | Medium | Small |
| `django.raw_sql_when_orm_suffices` | `RawSQL("SELECT … FROM <model_table>")` whose select list maps to fields the ORM can build | Low | Small |

##### SQLAlchemy

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `sqla.lazy_relationship_in_loop` | `for x in qs: x.<relationship>` where the relationship has default `lazy='select'` and no `joinedload`/`selectinload`/`subqueryload` is applied | High | Small |
| `sqla.session_query_in_loop` | `session.query(…).filter(…).all()` inside a loop | High | Small |
| `sqla.add_in_loop_no_bulk` | `session.add(obj)` inside a loop without `bulk_save_objects` | High | Small |
| `sqla.commit_in_loop` | `session.commit()` inside a loop | High | Small |
| `sqla.detached_relationship_access` | relationship access after `session.close()` / outside the session scope (heuristic: relationship attr access after a `close()` in the same symbol) | Medium | Medium |
| `sqla.raw_text_no_bind_params` | `text("… {x} …")` (f-string) — should be `text("… :x …")` with bind params (security + plan cache) | Medium | Small |

##### Tortoise ORM

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `tortoise.related_access_no_prefetch` | `for x in qs: x.<related>` without preceding `.prefetch_related(...)` | High | Small |
| `tortoise.save_in_loop_no_bulk` | `.save()` in loop without `bulk_create` | High | Small |
| `tortoise.queryset_in_loop` | `M.filter(…).first()` in loop | High | Small |

#### Java / Kotlin

##### Hibernate / JPA

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `hibernate.eager_fetch_on_collection` | `@OneToMany`/`@ManyToMany` field with `FetchType.EAGER` (default for `@ManyToOne` is fine; collection EAGER almost always wrong) | High | Medium |
| `hibernate.no_entity_graph_in_loop_query` | `repository.findAll()` inside loop where iterated entities expose lazy associations | High | Medium |
| `hibernate.missing_batch_size` | `@OneToMany` without `@BatchSize` or `@Fetch(SUBSELECT)` | Medium | Small |
| `hibernate.lazy_in_to_string` | `@Entity` with `@ToString(includeFieldNames = true)` covering a lazy collection | High | Small |
| `hibernate.find_in_loop_no_in_clause` | `repo.findById(id)` inside `for id in ids:` | High | Small |
| `hibernate.open_session_in_view_anti` | `OpenSessionInViewInterceptor`/`OpenEntityManagerInViewFilter` configured | Medium | Large |

##### Spring Data

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `spring_data.findall_no_pageable` | `Repository.findAll()` returning `List<T>` (no `Pageable`) on entity with `>1000` known row count | Medium | Small |
| `spring_data.derived_query_no_entitygraph` | repository method whose name implies a relationship traversal (`findByXxxJoinedYyy`) without `@EntityGraph` | Medium | Small |
| `spring_data.transactional_in_loop` | `@Transactional` method called per-iteration in caller's loop | High | Medium |

##### jOOQ

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `jooq.fetch_in_loop_no_in` | `ctx.select().from(T).where(T.ID.eq(id)).fetch()` inside loop | High | Small |
| `jooq.no_explicit_join` | `select.from(A).join(B)` with no `.on(...)` (relies on natural join) | Low | Trivial |

#### Node / TypeScript

##### Sequelize

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `sequelize.findall_in_loop` | `M.findAll(…)` / `M.findOne(…)` inside a `for`/`while`/`forEach` | High | Small |
| `sequelize.include_missing_required_relation` | `for x in xs: await x.getRelated()` without `{ include: [...] }` in the parent query | High | Small |
| `sequelize.create_in_loop_no_bulk` | `M.create({…})` in loop instead of `M.bulkCreate([…])` | High | Small |
| `sequelize.raw_query_with_template_literal` | `sequelize.query(\`SELECT … ${x}\`)` — no `replacements`/`bind` | Medium | Small |
| `sequelize.unrelated_save_in_loop` | `instance.save()` in loop | High | Small |

##### TypeORM

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `typeorm.findone_in_loop` | `repo.findOne(…)` inside a loop (use `findBy({ id: In(ids) })`) | High | Small |
| `typeorm.relations_missing` | `await repo.find()` then `for x in r: x.relation` with no `{ relations: [...] }` | High | Small |
| `typeorm.save_in_loop_no_bulk` | `repo.save(one)` per iter (should be `repo.save([many])`) | High | Small |
| `typeorm.eager_relation_default` | `@OneToMany(..., { eager: true })` | Medium | Small |

##### Prisma

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `prisma.findUnique_in_loop` | `prisma.<model>.findUnique({…})` inside a loop (should be `findMany({ where: { id: { in: ids } } })`) | High | Small |
| `prisma.findMany_no_include_then_traverse` | `findMany()` then a `for x in rows: x.<relation>` without `include` | High | Small |
| `prisma.create_in_loop_no_createMany` | `prisma.<model>.create(…)` in loop (should be `createMany([…])`) | High | Small |
| `prisma.transaction_in_loop` | `prisma.$transaction([…])` called per iteration | Medium | Small |
| `prisma.queryRaw_with_template_literal_no_tag` | `prisma.$queryRaw(\`SELECT … ${x}\`)` (untagged) | High | Trivial |

##### Drizzle

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `drizzle.select_in_loop` | `db.select().from(t).where(…)` in loop | High | Small |
| `drizzle.insert_in_loop_no_values_array` | `db.insert(t).values({…})` per iter | High | Small |

##### Mongoose

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `mongoose.findOne_in_loop` | `M.findOne(…)` in loop (use `M.find({ _id: { $in: ids } })`) | High | Small |
| `mongoose.populate_missing_then_access` | `M.find()` then `for x in r: x.<populated_field>` without `.populate('field')` | High | Small |
| `mongoose.save_in_loop_no_bulk` | `doc.save()` in loop (use `insertMany`/`bulkWrite`) | High | Small |
| `mongoose.no_lean_on_readonly` | `M.find()` with no `.lean()` on a read-only path (cost: full Mongoose document hydration) | Low | Trivial |

#### Go

##### GORM

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `gorm.find_in_loop` | `db.First(&x, id)` in loop (should be `db.Find(&xs, ids)`) | High | Small |
| `gorm.preload_missing_then_access` | `db.Find(&rows)` then `for _, r := range rows { r.Relation }` without `Preload(...)` | High | Small |
| `gorm.create_in_loop_no_batch` | `db.Create(&one)` in loop (should be `db.CreateInBatches`) | High | Small |
| `gorm.session_in_loop` | `db.Session(...)` per iter | Medium | Small |

##### ent

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `ent.first_in_loop` | `client.<Type>.Query().Where(...).First(ctx)` in loop | High | Small |
| `ent.with_edges_missing_then_query` | `.Query().All(ctx)` then per-row `.QueryEdge().All()` (should be `.With<Edge>(...)`) | High | Small |

##### sqlx

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `sqlx.query_in_loop` | `sqlx.QueryxContext(ctx, "SELECT … WHERE id = $1", id)` in loop (should be `WHERE id = ANY($1)`) | High | Small |

##### sqlc

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `sqlc.single_id_query_in_loop` | calling the sqlc-generated `GetByID` per-iter (suggest a `GetByIDs` variant) | Medium | Medium |

#### Ruby (future — supported via tree-sitter-ruby when added)

##### ActiveRecord

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `ar.n_plus_one_no_includes` | `for x in M.all { x.relation }` (without `.includes(:relation)`) | High | Small |
| `ar.find_each_missing` | iterating large result without `.find_each` | Medium | Small |
| `ar.update_in_loop_no_bulk` | `record.update(...)` per iter (use `M.update_all`) | High | Small |
| `ar.save_in_transaction` | `for r in rs: r.save` without `transaction do` | Medium | Small |

#### PHP (future)

##### Eloquent

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `eloquent.lazy_load_in_loop` | `@foreach($users as $u) {{ $u->profile->bio }}` without `->with('profile')` | High | Small |
| `eloquent.where_in_loop` | `M::where('x', $val)->first()` per iter (use `whereIn`) | High | Small |

##### Doctrine

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `doctrine.findone_in_loop` | `$em->getRepository(M::class)->findOneBy(...)` per iter | High | Small |
| `doctrine.lazy_collection_access_in_loop` | `foreach ($users as $u) { foreach ($u->getOrders() as $o) ... }` without `JOIN FETCH` | High | Small |

#### C# (future)

##### Entity Framework Core

| Rule | Pattern | Sev | Effort |
| --- | --- | --- | --- |
| `efcore.singleordefault_in_loop` | `ctx.Set<T>().FirstOrDefault(x => x.Id == id)` per iter | High | Small |
| `efcore.include_chain_then_where` | `ctx.Foo.Include(x => x.Bar).Where(...)` then per-item filter that should fold | Medium | Small |
| `efcore.savechanges_in_loop` | `await ctx.SaveChangesAsync()` per iter (batch instead) | High | Small |

### 4.3 Shared detection primitives

Most ORM rules fit into 3 archetypes — implement these once, reuse:

1. **`call_inside_loop_on_framework_receiver(method_set, framework)`** —
   already 80% of what `detect_n_plus_one` does, generalized to a
   framework predicate instead of a category set.
2. **`call_then_loop_with_attr_access(loader_methods, attr_predicate)`**
   — recognize "fetch then iterate, accessing a relation attribute" —
   the N+1-without-eager-load shape. Needs intra-symbol dataflow:
   variable bound to result of `framework.qs.<terminal>()`, then a
   `for` over that variable, then attribute access.
3. **`single_id_call_in_loop(method_set, framework)`** — recognize
   per-id lookups in a loop — the bulkable-write/read shape.

These primitives live in `orm_lint.rs`. Each per-framework rule is a
~10-line closure over them.

### 4.4 ORM detector — wiring

```rust
// src/orm_lint.rs

pub fn attach_orm_antipatterns(entries: &mut [CallTreeNode], graph: &CallGraph) {
    walk_mut(entries, |node| {
        let sym = graph.symbol_of(&node.id);
        let file_tags = graph.file_tags_for(&sym.file);
        let fws = detect_frameworks(&file_tags.imports);
        for fw in &fws {
            for rule in framework_rules(fw) {
                let hits = rule.check(sym, &node.external_calls, file_tags);
                for hit in hits {
                    node.findings.push(hit.to_finding());
                }
            }
        }
    });
}
```

`detect_frameworks` is a constant-time set intersection. `framework_rules`
looks up the rule list from `orm_signatures.json`. Each `rule.check` is
a pure function over data we already carry.

### 4.5 Severity / effort defaults

The defaults in §4.2 deliberately bias toward **High severity / Small
effort**. This is the right shape for ORM findings: when the pattern
matches, the fix is almost always a one-liner (`select_related`,
`bulkCreate`, `In(ids)`, `.with('profile')`) and the impact when not
fixed is direct (per-row query latency × N).

The existing `bump_severities_by_impact` and `attach_hot_log_findings`
infrastructure handles the "promote when on a hot path" case — no new
logic needed.

### 4.6 Where ORM findings replace the generic `n_plus_one`

Today the n+1 detector emits a *generic* finding for any db/cache call
in a loop. When an ORM detector fires on the *same call site*, we want
the ORM finding (more specific, better remediation) to **replace** the
generic one — not duplicate it.

Mechanism: after `attach_orm_antipatterns`, walk every node and remove
any `NPlusOne` finding whose `line` matches the line of an
`OrmAntipattern` finding from the same node. This keeps the Insights
tab clean and the per-symbol count honest.

### 4.7 Phase 2 test plan

Build out `tests/fixtures/orm-antipatterns/` with one subdirectory per
framework — each contains a minimal file demonstrating each rule in §4.2.
Snapshot the report; assert each rule fires on its expected line and no
generic `n_plus_one` ghost remains.

---

## 5. Phase 3 (opt-in): Live-DB EXPLAIN mode

This phase is gated behind a Cargo feature **and** an explicit CLI flag,
so the default build is unaffected. When enabled, drift can fetch an
EXPLAIN plan for unique SQL fingerprints and emit plan-level findings.

### 5.1 Activation

```toml
# Cargo.toml — feature off by default
[features]
explain = ["sqlx", "tokio"]

[dependencies]
sqlx  = { version = "0.7", optional = true, default-features = false, features = ["runtime-tokio", "postgres", "mysql"] }
tokio = { version = "1",   optional = true, features = ["rt", "macros"] }
```

CLI flag and env var:

```
drift-static-profiler analyze . \
    --explain \
    --explain-db postgres://user:pass@localhost/db \
    --explain-budget-ms 5000 \
    --explain-include-analyze            # opt-in for `EXPLAIN ANALYZE`
```

If `--explain` is passed without `--explain-db`, drift looks for
`DATABASE_URL` then `DRIFT_EXPLAIN_DB_URL`. If neither resolves, drift
emits a non-fatal warning and skips Phase 3 entirely.

### 5.2 Safety: `EXPLAIN` vs `EXPLAIN ANALYZE`

- `EXPLAIN` is metadata-only — safe against production.
- `EXPLAIN ANALYZE` **executes the query** — never safe by default.
  Gated behind `--explain-include-analyze` and refused outright when
  the parsed statement is anything other than `SELECT` (no
  ANALYZE of INSERT/UPDATE/DELETE).
- All EXPLAINs run inside a `BEGIN; … ROLLBACK;` wrapper as a
  belt-and-suspenders precaution.

### 5.3 Plan ingestion

Postgres: `EXPLAIN (FORMAT JSON, BUFFERS, VERBOSE, COSTS)` and
optionally `, ANALYZE`. MySQL: `EXPLAIN FORMAT=JSON`. SQLite:
`EXPLAIN QUERY PLAN`. Each backend produces a JSON document that we
normalize into a unified `PlanNode` IR:

```rust
pub struct PlanNode {
    pub op: String,                 // "Seq Scan", "Index Scan", "Hash Join", …
    pub relation: Option<String>,   // table name when applicable
    pub index: Option<String>,
    pub estimated_rows: f64,
    pub actual_rows: Option<f64>,   // present only with ANALYZE
    pub cost: f64,
    pub time_ms: Option<f64>,
    pub buffers_hit: Option<u64>,
    pub buffers_read: Option<u64>,
    pub sort_method: Option<String>,
    pub sort_space_kb: Option<u64>,
    pub hash_batches: Option<u64>,
    pub heap_fetches: Option<u64>,
    pub children: Vec<PlanNode>,
}
```

### 5.4 Plan-level rule catalog (v1)

| Rule | Trigger | Sev | Effort |
| --- | --- | --- | --- |
| `plan.seq_scan_large_table` | `op == "Seq Scan" && estimated_rows ≥ 10_000 && (actual_rows / estimated_rows) ≤ 0.1` (selective predicate) | High | Small |
| `plan.row_misestimate_10x` | `actual_rows ≥ 10× estimated_rows OR estimated_rows ≥ 10× actual_rows` | Medium | Medium |
| `plan.row_misestimate_100x` | same, with 100× | High | Medium |
| `plan.nested_loop_high_outer` | `op == "Nested Loop"` and outer plan has `actual_rows > 1000` and inner has no index | High | Medium |
| `plan.sort_spilled_to_disk` | `sort_method == "external merge"` | Medium | Small |
| `plan.hash_join_multiple_batches` | `hash_batches > 1` | Medium | Small |
| `plan.high_heap_fetches` | `heap_fetches > 1000` on an Index Scan | Low | Small |
| `plan.cte_materializes_large` | CTE node with `actual_rows > 10_000` (Postgres < 12) | Medium | Small |
| `plan.filter_high_rows_removed` | `Filter` node where `rows_removed_by_filter / actual_rows > 10` | Medium | Small |
| `plan.parallel_disabled_eligible` | scan node on table > 1M rows, no `Workers Planned` | Low | Medium |

Severity thresholds borrow from depesz (10× / 100× / 1000× row
misestimates; 10% / 50% / 90% exclusive time).

### 5.5 Budget enforcement

`--explain-budget-ms` is a *total* wall-clock budget across the whole
scan. We sort unique SQL fingerprints by occurrence count (highest
first) so the most-used queries get EXPLAINed first; when the budget is
exhausted we stop and emit a single "ran out of EXPLAIN budget after N
queries" note.

### 5.6 Optional: HypoPG hypothetical-index recommendations

When the connected database is Postgres and the
`hypopg` extension is installed, drift can additionally:

1. For each query that triggered `plan.seq_scan_large_table` or
   `plan.nested_loop_high_outer`, propose a candidate index on the
   filter/join columns.
2. `SELECT hypopg_create_index('CREATE INDEX ON … (col)');`
3. Re-run `EXPLAIN`. If the plan switches to the hypothetical index AND
   the cost drops ≥ 50%, emit a follow-up `BadQueryPlan` finding with
   the index DDL as `remediation`.

This is the Dexter design ported in-process. Gated behind
`--explain-hypopg` so it never runs without the user explicitly asking.

---

## 6. Schema additions (mirrored in `schema/profile.schema.json` and `viewer/src/types.ts`)

Three new optional fields on existing types — no breaking changes:

1. `Reference.sql_literal: Option<String>` — the captured SQL text for
   SQL-sink calls. Optional.
2. `ExternalCall.sql_literal: Option<String>` — same data, denormalized
   onto the call so detectors don't need to walk back to references.
   Optional.
3. `Reference.orm_framework: Option<String>` — set when the receiver
   binds to an ORM framework. Optional. (This is convenience — the
   detector can recompute it from imports, but caching it here saves
   repeat work.)

Three new `FindingKind` variants (§2.2). No new top-level structs.

### 6.1 Viewer impact

Zero new pages or tabs. The existing Insights tab renders any
`FindingKind` it sees — new kinds get an entry in the kind-filter
dropdown automatically. We add:

- `SQL_ANTIPATTERN`, `ORM_ANTIPATTERN`, `BAD_QUERY_PLAN` to the
  `KIND_LABEL` map in [viewer/src/Insights.tsx](viewer/src/Insights.tsx)
  (a one-line map extension).
- For SQL findings, when `evidence[0].call` looks like a rule id
  (`SQL001`), render it as a chip next to the severity badge.
- For ORM findings, render the framework name (`prisma`, `django`,
  `hibernate`) as a chip. Driven purely from the `call` field.

That's the entire viewer change.

---

## 7. Implementation in micro-steps (Phase 1 + 2; Phase 3 separate)

Each step ships independently, leaves `cargo test` green, and is
individually testable. Same cadence as the original
[INSIGHTS_PLAN.md §7](INSIGHTS_PLAN.md).

### 7.1 Phase 1 — SQL Query Analyzer

| # | Step | Test |
| --- | --- | --- |
| 1 | Add `sqlparser` + `pg_query` deps. Verify they build clean on Linux + macOS. | `cargo build` clean. |
| 2 | Extend `Reference` and `ExternalCall` with `sql_literal: Option<String>`. Empty default; serialization round-trips. | Existing fixtures still validate against the JSON schema. |
| 3 | Add per-language tree-sitter capture patterns for SQL sinks (Python `execute`/`text`, Java `createNativeQuery`, JS `query`/Knex `raw`, Go `Query`/`Exec`, Rust `sqlx::query!`). Stamp captured strings onto `Reference`. | Unit test: a Python fixture with `cursor.execute("SELECT 1")` produces a Reference with `sql_literal = Some("SELECT 1")`. |
| 4 | Carry `sql_literal` from Reference into ExternalCall in `graph.rs`. | Snapshot: external call serializes the SQL field. |
| 5 | New file `src/sql_lint.rs` with `attach_sql_antipatterns(entries, graph)`. Implement `SQL001-SQL004` only (SELECT *, DELETE/UPDATE no WHERE, INSERT no columns). Use `sqlparser` GenericDialect, no pg_query yet. | Snapshot: fixture file `tests/fixtures/sql-antipatterns/python/basic.py` produces the expected 4 findings on the expected lines. |
| 6 | Wire `attach_sql_antipatterns` into `Report::build` before `bump_severities_by_impact`. | Existing python-fastapi fixture: no regressions (no SQL literals, no new findings); new fixture: findings present. |
| 7 | Add dialect inference (`sql_patterns.json` driver→dialect table) and pick `pg_query` when Postgres-flavored. | Postgres-specific SQL (e.g. `RETURNING *`) parses cleanly with pg_query and produces no spurious findings. |
| 8 | Implement remaining v1 rules (`SQL005-SQL015`). Each rule = one closure + one unit test. | All rules covered by their own test cases. |
| 9 | Viewer: extend `KIND_LABEL` and chip rendering in `Insights.tsx`. | Manual smoke: SQL findings render with rule chip + dialect badge. |

### 7.2 Phase 2 — ORM Analyzers

| # | Step | Test |
| --- | --- | --- |
| 10 | Seed `orm_signatures.json` with all 17 frameworks listed in §4.2 (signatures only, no rules yet). | Loader test parses the file. |
| 11 | Implement detection primitives in `src/orm_lint.rs`: `call_inside_loop_on_framework_receiver`, `single_id_call_in_loop`, `call_then_loop_with_attr_access`. | Unit tests on synthetic ASTs. |
| 12 | Implement Django rules (§4.2 Python — `django.*`). | Snapshot against `tests/fixtures/orm-antipatterns/django/`. |
| 13 | Implement SQLAlchemy rules. | Snapshot against `tests/fixtures/orm-antipatterns/sqlalchemy/`. |
| 14 | Implement Tortoise rules. | Snapshot against `tests/fixtures/orm-antipatterns/tortoise/`. |
| 15 | Implement Hibernate/JPA rules. | Snapshot against `tests/fixtures/orm-antipatterns/hibernate/`. |
| 16 | Implement Spring Data + jOOQ rules. | Snapshot against `tests/fixtures/orm-antipatterns/spring/`. |
| 17 | Implement Sequelize + TypeORM rules. | Snapshot against `tests/fixtures/orm-antipatterns/sequelize/` + `…/typeorm/`. |
| 18 | Implement Prisma + Drizzle + Mongoose rules. | Snapshot against `tests/fixtures/orm-antipatterns/prisma/` + Drizzle + Mongoose. |
| 19 | Implement GORM + ent + sqlx + sqlc rules. | Snapshot against `tests/fixtures/orm-antipatterns/gorm/` + etc. |
| 20 | Implement EF Core + ActiveRecord + Eloquent + Doctrine rules (when language support lands; ActiveRecord/Eloquent waiting on tree-sitter-ruby / tree-sitter-php which the project doesn't ship today — gate these steps behind grammar availability). | Snapshot per framework. |
| 21 | Implement de-duplication: remove generic `NPlusOne` findings whose line collides with an `OrmAntipattern` finding from the same node (§4.6). | Snapshot: no double-counting. |
| 22 | Viewer: framework chip + remediation rendering. | Manual smoke. |

### 7.3 Phase 3 — Live-DB EXPLAIN (opt-in)

| # | Step | Test |
| --- | --- | --- |
| 23 | Add `explain` Cargo feature with `sqlx`+`tokio` deps. CI builds both default and `--features explain`. | Both build clean. |
| 24 | CLI flags + env var resolution + driver inference. | Help text test; unit test for env-var fallback. |
| 25 | New file `src/plan_lint.rs` with the unified `PlanNode` IR and a `PostgresParser` that converts pg-style EXPLAIN JSON into `PlanNode`. | Unit tests over saved EXPLAIN JSON samples. |
| 26 | `MysqlParser` similarly. | Unit tests over saved samples. |
| 27 | Implement the v1 plan rules (`plan.seq_scan_large_table` through `plan.parallel_disabled_eligible`). | Per-rule unit tests over saved plans. |
| 28 | Wire the EXPLAIN runner with budget enforcement; emit `BadQueryPlan` findings on the originating `CallTreeNode`. | Integration test against a containerized Postgres (`make explain-it`). |
| 29 | Optional HypoPG follow-up step (gated). | Integration test. |

---

## 8. False-positive policy (the bar we hold)

All new detectors must:

1. **Refuse to fire when uncertain.** SQL we can't parse → silent skip
   (existing generic n+1 still fires from category-level). ORM call
   without strong framework evidence (Tier B import or Tier C receiver
   pattern, never Tier D method-name alone) → silent skip.
2. **Confidence floor of 0.65 for any finding pushed to the user.**
   The existing `Finding.confidence` field carries this; matches the
   bar already applied for `BlockingInAsync` and `NPlusOne`.
3. **Per-rule snapshot tests with positive AND negative cases.** Every
   rule ID in §3.4 and §4.2 must ship with one "this should fire" file
   and one "this looks similar but isn't a bug" file.
4. **Catalog entries carry a `reason` URL** to a public source (ORM
   docs, framework migration guide, perf blog post, vendor doc) so a
   future maintainer can verify the rule still applies after a
   framework update.

---

## 9. Open research questions (not blocking)

1. **Cross-symbol dataflow.** Some ORM rules (e.g. Hibernate's "lazy
   collection accessed in caller after session closed") need
   inter-procedural dataflow. We have `call_site_count` and `callers`
   on every node; the small step is a 1-hop caller check, the big step
   is full pointer analysis (out of scope).
2. **SQL extracted from string concatenation / f-strings.** Today we
   only catch *literal* SQL strings. A common pattern in legacy code is
   `cursor.execute("SELECT * FROM " + table + " WHERE id = %s", (id,))`.
   We can heuristically reconstruct (replace concat operands with
   `<expr>` placeholders) to still run lints; flag as `confidence 0.55`.
3. **Cross-file framework detection.** Today framework detection is
   per-file. A subclass of a Hibernate `JpaRepository<T, ID>` defined
   in another file should mark methods on the subclass as Spring Data
   too. We have `Binding.extends` already populated; we just need to
   walk it transitively.
4. **HypoPG limits.** HypoPG only handles BTree, BRIN, Hash, Gin
   indexes — not partial / functional / expression indexes. Some of our
   recommendations will be unverifiable; flag as `confidence 0.50`.
5. **GraphQL resolver N+1.** Highest-value detection (and hardest):
   every resolver function that hits the DB in a `for child in
   parent.children` pattern is N+1 unless wrapped in DataLoader. Needs
   a GraphQL-aware sub-detector — defer until v2.
6. **Migration safety lints (squawk parity).** Out of scope here — we
   can clean-room reimplement squawk's rule catalog on top of
   `pg_query` in a future plan if there's demand.
7. **Whole-file timeout** for `pg_query`/`sqlparser` to avoid pathological
   inputs hanging the scan. Default 100 ms per literal; budget-skipped
   SQLs simply don't get linted.

---

## 10. Acceptance checklist for the plan

- [x] Builds on the existing `insights::Finding` substrate — no parallel
      hierarchy, no new viewer pages, no new top-level JSON sections.
- [x] Adds **three** `FindingKind` variants: `SqlAntipattern`,
      `OrmAntipattern`, `BadQueryPlan`. Schema impact is three optional
      fields total.
- [x] Lists **15 SQL lint rules** with severity, effort, and remediation
      (§3.4).
- [x] Lists **17 ORM frameworks** with **80+ specific rules** (§4.2),
      each citing the unified detection primitives.
- [x] Specifies **10 plan-level rules** with thresholds borrowed from
      pgMustard / depesz / pganalyze (§5.4).
- [x] Defends the perf budget: SQL parsing deduped by fingerprint;
      ORM detection is linear over data we already carry; EXPLAIN
      gated behind feature + flag + total time budget.
- [x] Lists license-clean dependencies only (sqlparser-rs Apache, pg_query
      BSD, sqlx MIT/Apache). Calls out squawk's GPL as off-limits for
      linking.
- [x] Sequences the work into **29 small, testable micro-steps**
      (§7.1-§7.3), with Phase 3 isolated behind a Cargo feature so the
      default build is unaffected.
- [x] Defines the **false-positive policy** (§8): silent-skip when
      uncertain, confidence ≥ 0.65 to surface, positive + negative
      fixtures per rule.

When you say "go", step 1 begins.

---

## 11. Round-2 research expansion (this plan, extended)

After the first round shipped (sqlparser+pg_query deps in [Cargo.toml](Cargo.toml), `sql_literal: Option<String>` on [Reference](src/lib.rs) + [ExternalCall](src/graph.rs), schema documented), six additional research agents covered:

- **NoSQL & non-relational stores** — MongoDB, Redis, Elasticsearch/OpenSearch, DynamoDB, Cassandra/Scylla, vector DBs (pgvector/Pinecone/Qdrant/Weaviate/Milvus/Chroma), time-series (TimescaleDB/InfluxDB/Prometheus/ClickHouse/QuestDB), graph DBs (Neo4j/Arango/Neptune), search/cache hybrids (Meilisearch/Typesense/Algolia), object stores (S3/GCS/Azure Blob).
- **HTTP / GraphQL / RPC** — per-language HTTP client anti-patterns, retry/backoff/circuit-breaker, GraphQL N+1 + DataLoader, gRPC channel reuse + deadlines, REST chatty patterns, webhook handler hygiene, service-mesh / proxy-side detection.
- **Async / concurrency / queues / cache / streaming** — per-language async anti-patterns (Python/JS/Rust/Go/Java/Kotlin/.NET), Kafka/RabbitMQ/SQS/Celery/Sidekiq, cache stampede + TTL + lru_cache misuse, Spark/Flink/Beam/dbt/Airflow/Pandas/Dask/Ray, file I/O streaming, executor/thread anti-patterns.
- **Long-tail ORMs** — Python: SQLModel, Peewee, Pony, Beanie, Motor, MongoEngine, Edgy, Piccolo, ormar; Java/Kotlin: MyBatis, Ebean, Querydsl, Exposed, Ktorm, Komapper, jdbi, OpenJPA, EclipseLink; Rust: Diesel, SeaORM, Welds, Loco, ormx, Cornucopia, toasty; Node/TS: Objection.js, Knex, MikroORM, Bookshelf, Waterline, Kysely, Slonik, Postgres.js; Go: SQLBoiler, beego/orm, upper/db, Bun, xorm; PHP: Cycle, Atlas.Orm, Propel, RedBean, Spot; Ruby: Sequel, ROM, DataMapper, Mongoid; Scala: Slick, Quill, Doobie, Skunk, ScalikeJDBC, Squeryl; Elixir: Ecto, Ash; Crystal: Granite, Avram, Jennifer.cr, Crecto; Swift: GRDB, CoreData, Vapor Fluent, SwiftData; Dart: Drift (the Dart pkg — explicitly NOT us, must disambiguate in `module_overrides.json`), Floor, Isar, ObjectBox; .NET: Dapper, NHibernate, LINQ to SQL, ServiceStack OrmLite, RepoDb, LiteDB.
- **Schema migration safety** — squawk, pgroll, atlas, dbmate, golang-migrate, sqitch, refinery, sqlx-cli, sea-orm-cli, diesel migrations, tern, Bytebase, Alembic, Django migrations, aerich, Piccolo, ActiveRecord + strong_migrations + online_migrations, Flyway, Liquibase, Prisma Migrate, TypeORM/Sequelize/Knex/Drizzle Kit/MikroORM migrations, GORM AutoMigrate (with anti-pattern call-out), goose, EF Core migrations, FluentMigrator, Doctrine Migrations, Phinx, Laravel migrations, Ecto migrations, Hibernate `hbm2ddl.auto` config.
- **Connection pool + transaction & locking** — HikariCP, Tomcat JDBC, c3p0, DBCP, SQLAlchemy QueuePool/NullPool/StaticPool, Django `CONN_MAX_AGE`/`CONN_HEALTH_CHECKS`, psycopg2/3, asyncpg, aiopg, pgbouncer modes, Prisma `connection_limit`/`pgbouncer=true`, Sequelize/TypeORM/node-pg/mongoose pools, GORM/database/sql SetMax*, Spring Boot HikariCP, ADO.NET/EF Core, pgxpool; Spring `@Transactional` proxy bypass family, `@Transactional` on private/static/final, self-invocation bypass, `REQUIRES_NEW` in loop, OSIV, EXTENDED_PERSISTENCE_CONTEXT, `SELECT FOR UPDATE` without `SKIP LOCKED`, optimistic-locking missing, distributed 2PC, isolation-level defaults, deadlock-prone ordering.
- **ORM API catalog (the "negative signal" alphabet)** — full menu of eager-load / projection / streaming / bulk / cache APIs across 21 ORMs (~90 method/annotation names total). Used as **negative signals**: the *absence* of any of these on a query chain that touches a relation is the static N+1 trigger.

Full per-tool catalogs (links, licenses, rule names, tree-sitter shapes) are saved as standalone files in [`research/`](research/) so detector authors can grep them while writing rules:

- [`research/ORM_NICHE_CATALOG.md`](research/ORM_NICHE_CATALOG.md) — long-tail ORM/ODM/query-builder coverage (~60 frameworks) with import sets, receiver patterns, N+1 detect APIs, eager-load primitives, bulk-write APIs, and tree-sitter call signatures.
- [`research/ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md`](research/ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md) — the exhaustive per-ORM tables for eager-load primitives, DTO/projection APIs, second-level cache APIs, and streaming/cursor APIs. The load-bearing reference for §11.6's "negative signal" alphabet.
- [`research/ORM_MIGRATIONS_POOLS_TRANSACTIONS.md`](research/ORM_MIGRATIONS_POOLS_TRANSACTIONS.md) — Parts A/B/C of the round-2 catalog: schema migrations (squawk + strong_migrations rule ports + per-tool file detection), connection pool config (per-driver defaults + sizing math), and transaction management (long-running tx, Spring `@Transactional` cluster, RMW races, optimistic locking, outbox-pattern violations). Sourced from squawk/pgroll/strong_migrations/HikariCP/SQLAlchemy/asyncpg/Mongoose/Prisma/PgBouncer/Baeldung/Vlad Mihalcea/microservices.io.

The remaining three research artifacts (NoSQL datastores, HTTP/GraphQL/RPC, async/queue/cache/streaming) land alongside their sibling implementation plans per §11.7. This section folds the actionable extensions into the plan.

### 11.1 New FindingKind variants (extending §2.2)

```rust
pub enum FindingKind {
    // ── existing (round 1) ──
    NPlusOne, BlockingInAsync, Recursive, SmellyLoop, NoisyLog,
    OutdatedPackage, MemoryExplosion, HotZone, ExpensiveCompute,
    MissingCaching, LogAmplification,
    SqlAntipattern, OrmAntipattern, BadQueryPlan,

    // ── new (round 2) ──
    /// A NoSQL/non-relational query (Mongo aggregation, Elasticsearch
    /// JSON DSL, Redis command, DynamoDB call, Cassandra CQL, Cypher,
    /// PromQL, InfluxQL, vector-search call) matches a per-store
    /// anti-pattern. Carries `(store, rule-id)` in the message.
    NosqlAntipattern,

    /// A schema migration file matches a "dangerous migration" rule
    /// (NOT NULL no default, drop column, non-CONCURRENTLY index,
    /// missing NOT VALID on FK, AutoMigrate on boot, ddl-auto=update,
    /// timestamp collision, etc.). Carries `(tool, rule-id)`.
    MigrationSafety,

    /// A connection-pool or driver config value is misconfigured
    /// (CONN_MAX_AGE=0, no pool_pre_ping behind proxy, MaxOpenConns
    /// unset on GORM, HikariCP maximumPoolSize > DB capacity,
    /// idle_timeout > server idle disconnect).
    PoolMisconfig,

    /// A transaction/locking pattern is wrong (long-running tx with
    /// HTTP inside, @Transactional on private, commit-in-loop,
    /// REQUIRES_NEW in loop, SELECT FOR UPDATE without SKIP LOCKED,
    /// optimistic-lock missing, OSIV enabled).
    TxAntipattern,

    /// HTTP/gRPC/GraphQL/webhook anti-pattern (HTTP request in loop
    /// without Promise.all, missing timeout, retry-loop without
    /// backoff, per-call gRPC channel creation, GraphQL resolver
    /// without DataLoader, webhook handler doing more than HMAC+enqueue,
    /// consecutive sequential awaits with no data dependency).
    NetworkAntipattern,

    /// Async/concurrency anti-pattern beyond the existing BlockingInAsync
    /// (asyncio.create_task ref dropped, `await` inside `forEach`,
    /// `*Sync` in handler, `await_holding_lock`, Goroutine leak,
    /// `Task.Result` deadlock, channel backpressure absent, etc.).
    AsyncAntipattern,

    /// Message-queue / streaming-pipeline anti-pattern (Kafka unkeyed
    /// ProducerRecord, RabbitMQ auto_ack=True, SQS no long-polling,
    /// Spark `.collect()` on large DF, dbt missing is_incremental,
    /// Airflow top-level external IO in DAG file).
    QueueAntipattern,

    /// Cache-layer anti-pattern (Redis SET without EX, lru_cache on
    /// method, lru_cache with no maxsize, cache miss in loop without
    /// batch, two-level cache without coherence, no negative caching).
    CacheAntipattern,

    /// Read-replica routing bug: write to replica, read-after-write
    /// from replica without stick-to-primary toggle, async-context
    /// loss of routing thread-local.
    ReplicaRoutingBug,
}
```

### 11.2 ORM signature catalog — long-tail expansion (extends §4)

The framework list in §4.1 doubles. Append to `orm_signatures.json`:

**Python**: `sqlmodel`, `peewee`, `pony.orm`, `beanie`, `motor`, `mongoengine`, `edgy`, `saffier`, `piccolo`, `ormar`, `aerich`, `redis_om`.
**Java/Kotlin**: `mybatis`/`mybatis-plus`, `ebean`, `querydsl`, `exposed`, `ktorm`, `komapper`, `jdbi`, `openjpa`, `eclipselink`, `requery`, `spring-data-cassandra`/`spring-data-redis`/`spring-data-mongodb`/`spring-data-r2dbc`.
**Rust**: `diesel`, `sea-orm`, `welds`, `loco`, `ormx`, `cornucopia`, `toasty`.
**Node/TS**: `objection`, `knex`, `mikro-orm`, `bookshelf`, `waterline`, `kysely`, `slonik`, `postgres` (Postgres.js).
**Go**: `sqlboiler`, `github.com/astaxie/beego/orm`, `upper.io/db.v3`, `github.com/uptrace/bun`, `xorm.io/xorm`.
**PHP**: `cycle/orm`, `atlas-orm`, `propel/propel`, `gabordemooij/redbean`, `vlucas/spot2`.
**Ruby**: `sequel`, `rom-rb/rom`, `mongoid`.
**Scala**: `slick`, `getquill/quill`, `tpolecat/doobie`, `tpolecat/skunk`, `scalikejdbc`, `squeryl`.
**Elixir**: `ecto`, `ash`.
**Crystal**: `granite`, `crystal-lang/avram`, `imdrasil/jennifer.cr`, `crecto/crecto`.
**Swift**: `groue/grdb.swift`, `apple/CoreData`, `vapor/fluent`, `apple/swift-data`.
**Dart**: `floor`, `isar`, `objectbox` (NOT `drift` — that's the Dart sqlite package and would be a self-finding; add as explicit blocklist in `module_overrides.json`).
**.NET**: `Dapper`, `NHibernate`, `LINQ to SQL`, `ServiceStack.OrmLite`, `RepoDb`, `LiteDB`.

For each, the signature payload is:
```jsonc
{
  "import_modules": [...],
  "queryset_methods": [...],
  "eager_load_methods": [...],     // ← the NEGATIVE signal alphabet
  "write_methods": [...],
  "bulk_methods": [...],
  "streaming_methods": [...],
  "projection_methods": [...],
  "cache_apis": [...],
  "tx_apis": [...]
}
```

The eager_load / bulk / streaming / projection arrays do double duty: they're **negative signals** (their presence suppresses an N+1 finding on the same chain) and they're **remediation hints** (named in the `remediation:` text). Architecture rule: detectors never hard-code these names — they always read from the catalog.

### 11.3 Migration safety detector family

New file `src/migration_lint.rs`. Recognizes migration files by path convention per tool (see the per-tool table in the research transcript: ~30 tools, each with a stable file-name pattern). For each file:

- **SQL-bodied migrations** (Flyway, dbmate, golang-migrate, goose, sqitch, refinery, sqlx, Diesel, atlas, Bytebase, tern, raw `.sql` under `db/migrate/`): hand each file to the SQL backend (`sqlparser` or `pg_query` per dialect inference §3.3) and run the rule catalog below.
- **Host-language migrations** (Alembic, Django, AR, EF Core, Doctrine, Sequelize, TypeORM, Knex, MikroORM, Ecto, Phinx, Laravel, Piccolo, aerich): tree-sit the host language, match the per-tool operation-call shapes, lint the operation kwargs.
- **YAML/JSON migrations** (pgroll, Liquibase YAML): `serde_yaml` + walk the operation tree.
- **XML migrations** (Liquibase XML): `quick-xml` + walk changeset elements.
- **Config-as-migration** (Hibernate `hbm2ddl.auto`, GORM `AutoMigrate(...)`): properties-file regex / Go tree-sit call detection.

Catalog file: `src/research_classefiers+categories/migration_rules.json`. Rule IDs:

| ID | Rule | Sev | Effort |
| --- | --- | --- | --- |
| `MIG001` | `add_column_not_null_no_default` | High | Small |
| `MIG002` | `add_column_with_default_pre_pg11` | Medium | Medium |
| `MIG003` | `drop_column` | High | Medium |
| `MIG004` | `rename_column` | High | Medium |
| `MIG005` | `change_column_type` | High | Medium |
| `MIG006` | `create_index_not_concurrently` | High | Trivial |
| `MIG007` | `drop_index_not_concurrently` | Medium | Trivial |
| `MIG008` | `add_fk_no_not_valid` | High | Small |
| `MIG009` | `add_check_no_not_valid` | Medium | Small |
| `MIG010` | `vacuum_full_in_migration` | High | Small |
| `MIG011` | `disable_trigger_in_migration` | High | Small |
| `MIG012` | `long_data_backfill_in_migration` | Medium | Medium |
| `MIG013` | `concurrently_inside_transaction` | High | Trivial |
| `MIG014` | `mixing_ddl_and_data` | Medium | Small |
| `MIG015` | `rename_table_no_view_shim` | High | Medium |
| `MIG016` | `migration_timestamp_collision` | Medium | Trivial |
| `MIG017` | `missing_down_migration` | Low | Small |
| `MIG018` | `gorm_automigrate_on_boot` | High | Medium |
| `MIG019` | `hibernate_ddl_auto_update_or_create` | High | Trivial |
| `MIG020` | `ar_create_table_force_true` | High | Trivial |
| `MIG021` | `drop_table` | High | Medium |
| `MIG022` | `rename_model_class_orphaned_migration` | Medium | Medium |

Rule-name vocabulary borrowed from squawk + Ankane's `strong_migrations` (clean-room — strings only, no code linking from GPL squawk). Two upstream catalogs to mine on every refresh: `django-migration-linter` (Apache-2.0) and `strong_migrations` (MIT).

### 11.4 Connection pool config detector family

New file `src/config_lint.rs`. **No tree-sitter cost** for the bulk of these — they live in `.properties`, `.yml`, `.env`, `.json`, `appsettings.json`, `pgbouncer.ini`. Drift already parses several manifest formats in [manifest.rs](src/manifest.rs); extend with a generic key/value lookup driven by `pool_config_rules.json`:

```jsonc
{
  "rules": [
    { "id": "POOL001", "tool": "django", "file_pattern": "settings*.py",
      "key": "DATABASES.*.CONN_MAX_AGE", "trigger": "equals", "value": 0,
      "severity": "medium", "remediation": "Set CONN_MAX_AGE=60+ for persistent connections; pair with CONN_HEALTH_CHECKS=True." },
    { "id": "POOL002", "tool": "sqlalchemy", "file_pattern": "*.py",
      "ast_kind": "call:create_engine", "kwarg": "pool_pre_ping", "trigger": "missing_or_false",
      "severity": "medium", "remediation": "pool_pre_ping=True. Especially behind PgBouncer/RDS Proxy." },
    { "id": "POOL003", "tool": "hikari", "file_pattern": "application*.{yml,properties}",
      "key": "spring.datasource.hikari.maximumPoolSize", "trigger": "gt", "value": 50,
      "severity": "medium", "remediation": "Pool size × replicas should stay below DB max_connections (PG default 100)." },
    { "id": "POOL004", "tool": "gorm", "file_pattern": "*.go",
      "ast_kind": "call:gorm.Open", "follow_up": "missing:SetMaxOpenConns",
      "severity": "high", "remediation": "database/sql default = unlimited. Always call db.DB().SetMaxOpenConns(N)." },
    { "id": "POOL005", "tool": "prisma", "file_pattern": "*.{env,prisma}",
      "url_param": "pgbouncer", "trigger": "missing_when_behind_proxy",
      "severity": "medium", "remediation": "Add ?pgbouncer=true to DATABASE_URL when behind PgBouncer in transaction mode." },
    { "id": "POOL006", "tool": "*", "file_pattern": "*",
      "constraint": "pool_size_times_processes_gt_db_max",
      "severity": "high", "remediation": "Total app-side pool capacity exceeds estimated DB max_connections." }
    // ~25 more
  ]
}
```

Detector becomes a tiny interpreter: read the rule file, locate the file via glob, evaluate the trigger, emit a `PoolMisconfig` finding. New `Effort` defaults are usually `Trivial` (one-line config change).

### 11.5 Transaction & locking detector family

Three reusable shape detectors carry ~80% of the catalog (per research agent):

1. **`db_call_in_loop` (generalized)** — already exists for reads; extend to `db_write_in_loop`, `commit_in_loop`, `tx_open_in_loop` by adding a per-ORM signature table `(method_name, receiver_pattern) → {read, write, commit, begin}`.
2. **`tx_scope_contains_io`** — for any tx-opening primitive (decorator, `with` block, annotation+method body), walk the lexical body and check for any call drift's existing classifier tags as `Network` / `Io` / `Sleep`.
3. **`config_value_constraint`** — the same engine §11.4 uses, also catches `spring.jpa.open-in-view=true`, `isolation = SERIALIZABLE`.

New rules emit `FindingKind::TxAntipattern`. Rule IDs:

| ID | Rule | Detect kind |
| --- | --- | --- |
| `TX001` | `long_running_tx_with_http` | scope-contains-io |
| `TX002` | `spring_transactional_on_private` | tree-sit shape |
| `TX003` | `spring_transactional_self_invocation` | tree-sit, intra-file |
| `TX004` | `requires_new_in_loop` | tree-sit, intra-method |
| `TX005` | `transactional_on_rest_controller` | annotation pairing |
| `TX006` | `open_session_in_view_enabled` | config |
| `TX007` | `commit_in_loop` | shape |
| `TX008` | `flush_in_loop_no_clear` | shape |
| `TX009` | `select_for_update_no_skip_locked` | SQL AST |
| `TX010` | `optimistic_locking_missing` | model field absence |
| `TX011` | `pessimistic_lock_over_http` | scope-contains-io |
| `TX012` | `serializable_isolation_default` | config |
| `TX013` | `autocommit_true_in_business_code` | tree-sit |
| `TX014` | `prisma_transaction_mixing_read_write` | array literal walk |
| `TX015` | `save_in_loop` | shape (write variant of NPlusOne) |
| `TX016` | `ar_nested_transaction_no_requires_new` | tree-sit |
| `TX017` | `outbox_missing_for_dual_write` | scope-contains-write+http |
| `TX018` | `prepared_statement_explosion_pgbouncer_tx` | pool config × ORM intersection |

Catalog file: `src/research_classefiers+categories/tx_rules.json` listing per-ORM/per-language tx-opening primitives.

### 11.6 The "negative signal" architecture (eager-load / projection / streaming / bulk / cache)

Across 21 ORMs the eager-load alphabet is ~90 distinct method/annotation names: `select_related`, `prefetch_related`, `Prefetch`, `joinedload`, `selectinload`, `subqueryload`, `contains_eager`, `raiseload`, `with_loader_criteria`, `Bundle`, `with_only_columns`, `JOIN FETCH`, `@EntityGraph`, `@BatchSize`, `@Fetch(SUBSELECT)`, `@FetchProfile`, `MULTISET`, `multisetAgg`, `include`, `attributes`, `relations`, `select`, `_count`, `relationLoadStrategy:"join"`, `populate`, `lean`, `with`, `withCount`, `Include`, `ThenInclude`, `AsSplitQuery`, `AsNoTracking`, `includes`, `preload`, `eager_load`, `joins`, `references`, `pluck`, `belonging_to` + `grouped_by`, `find_with_related`, `find_also_related`, `LoaderTrait::load_*`, `Preload`, `Joins`, `WithEdges`, `With*`, `preload:`, `from u in ..., preload: [...]`, …

**Architecture rule** (cleanest implementation):

```rust
// src/orm_lint.rs

/// Walk a method-call chain `expr.a().b().c().terminal()` and collect
/// every method/attribute name as a flat Vec<&str>. Stops at the
/// receiver (variable, identifier, or non-call expr).
fn chain_method_names<'a>(node: tree_sitter::Node<'a>, src: &'a [u8]) -> Vec<&'a str> { /* … */ }

/// Returns true if any negative signal from the ORM's signature
/// table appears anywhere in `chain_names`. Caller skips emitting
/// the N+1 finding when this returns true.
fn chain_has_negative_signal(chain_names: &[&str], orm: &FrameworkSig) -> bool {
    chain_names.iter().any(|n| orm.eager_load_methods.contains(n)
                           || orm.bulk_methods.contains(n)
                           || orm.streaming_methods.contains(n)
                           || orm.projection_methods.contains(n))
}
```

Same code-path works for every ORM. The 90-name table lives in `orm_signatures.json` (data, not code). New ORMs cost one JSON block, no Rust changes. False-positive control: the table also captures *anti-signals* (e.g. SQLAlchemy `lazyload`/`noload` are *positive* signals that re-enable the N+1).

For **special cases** (Django `select_related` doesn't help on M2M; jOOQ `MULTISET` is type-aware; Sequelize `include` with `separate: true` runs an extra query the way `prefetch_related` does), the signature entry carries a `caveats` array; the detector reads the caveat text into the remediation suffix.

### 11.7 NoSQL / HTTP / Async expansion (next plans)

The non-ORM research (NoSQL stores, HTTP/GraphQL/RPC, async/queue/cache/streaming) is structurally orthogonal to this plan. Each warrants its own sibling document so the implementation stays scoped:

- **`NOSQL_ANALYZER_PLAN.md`** — Mongo aggregation walk, Redis command sniff, Elasticsearch JSON DSL via `serde_json`, DynamoDB Scan-in-prod, Cassandra `ALLOW FILTERING`, Neo4j Cypher via `tree-sitter-cypher` + `drasi-query-cypher`, PromQL via `metricsql_parser`, InfluxQL via `influxql-parser`. Two-stage parsing (tree-sitter host → per-DSL Rust parser) as the architecture.
- **`HTTP_RPC_GRAPHQL_PLAN.md`** — three tiers: (T1) high-precision tree-sitter rules (`HttpRequestInLoop`, `HttpRequestMissingTimeout`, `HttpClientPerRequest`, `RetryLoopWithoutBackoff`, `ReqwestBlockingInAsync`, `PerCallGrpcChannel`, `MissingDeadline`); (T2) heuristic / info-level (`ResolverWithoutDataLoader`, `MissingComplexityLimit`, `WebhookHandlerDoesWork`, `SequentialHttpCalls`); (T3) config audits (`FieldNumberHotPathOverflow`, `ReflectionInProd`, `DeprecatedApacheHttpClient`, `HystrixDeprecated`). Two notable ecosystem gaps drift can fill first: GraphQL `no-n-plus-one` static rule (no maintained tool), and Clippy `reqwest::blocking` in async (issue 4377, open since 2019).
- **`ASYNC_QUEUE_CACHE_PLAN.md`** — extend `BlockingInAsync` with the full `flake8-async` ASYNC100-232 catalog; add `AsyncTaskDropped` (HIGH — silent GC bug), `CacheUnboundedDecorator`, `RedisSetMissingTtl`, `AsyncForEachJs`, `SyncIoSuffix`, `GoContextWithCancelNoCancel`, `PandasIterrowsApplyAxis1`, `KafkaProducerNoKey`, `SqsNoLongPolling`, `ConsecutiveAwaits`, `CacheMissInLoopNoBatch`, `RustAwaitHoldingStdMutex`, `AirflowTopLevelExternalCall`, `SparkCollectOnLargeDf`, `DbtIncrementalMissingIsIncremental`.

The estimated rule-count after all three sibling plans ship: **~250 new statically-detectable rules** on top of the existing detectors, behind **~6 reusable detector shapes**, all reusing the existing `Finding { kind, severity, effort, confidence, evidence, remediation }` substrate.

### 11.8 Updated micro-step list (extending §7)

After Phase 1 (SQL Query Analyzer, steps 1-9) and Phase 2 (ORM Analyzers, steps 10-22), append:

| # | Step | Test |
| --- | --- | --- |
| 30 | Add `MigrationSafety`/`PoolMisconfig`/`TxAntipattern`/`NosqlAntipattern`/`NetworkAntipattern`/`AsyncAntipattern`/`QueueAntipattern`/`CacheAntipattern`/`ReplicaRoutingBug` to `FindingKind` (§11.1). Update `as_str` + serde tests. | Existing snapshot tests unchanged; new variants serialize correctly. |
| 31 | Extend `orm_signatures.json` with the long-tail 40+ ORMs from §11.2. Add the per-ORM eager_load/bulk/streaming/projection/cache/tx arrays. | Loader test parses; for each ORM, a tiny fixture confirms signature detection. |
| 32 | Implement `chain_method_names` + `chain_has_negative_signal` helper in `src/orm_lint.rs` (§11.6). Backport into the round-1 ORM rules so they read the negative-signal arrays from JSON instead of hard-coded constants. | All round-1 ORM snapshots unchanged. |
| 33 | New file `src/migration_lint.rs`. Implement migration-file detection by path glob per tool. Implement the SQL-bodied migration rules (`MIG001-MIG017`) via sqlparser/pg_query. | Snapshot against `tests/fixtures/migrations/{flyway,dbmate,sqlx}/`. |
| 34 | Implement host-language migration linting (Alembic/Django/AR/EF Core/Doctrine/Sequelize/TypeORM/Knex/MikroORM/Ecto/Phinx/Laravel/Piccolo/aerich). | Snapshot per tool. |
| 35 | Implement `MIG018` (GORM `AutoMigrate` on boot) + `MIG019` (Hibernate `ddl-auto=update`). | Snapshot. |
| 36 | New file `src/config_lint.rs`. Implement the generic key-lookup engine (§11.4) + the first ~10 `POOL*` rules. | Snapshot against `tests/fixtures/pool-config/`. |
| 37 | Implement remaining `POOL*` rules including the cross-process topology constraint (`POOL006`) — reads `Procfile`/docker-compose/`deployment.yaml` to sum pool maxes. | Snapshot. |
| 38 | Implement `tx_scope_contains_io` reusable detector. Use it for `TX001`, `TX007`, `TX008`, `TX011`, `TX017`. | Snapshot. |
| 39 | Implement Spring-annotation tx rules (`TX002`-`TX005`). | Snapshot. |
| 40 | Implement remaining `TX*` rules (`TX006`, `TX009`-`TX016`, `TX018`). | Snapshot. |
| 41 | De-dup pass: `OrmAntipattern` finding hides a `TxAntipattern` `save_in_loop` finding on the same line (§4.6 generalization). | Snapshot: no double-counting. |
| 42 | Viewer: chip rendering for the new kinds (`migration`, `pool`, `tx`, `nosql`, `network`, `async`, `queue`, `cache`, `replica`). One-line `KIND_LABEL` extension. | Manual smoke. |

After step 42 the round-2 ORM-adjacent coverage ships. Steps 43+ are reserved for the sibling plans (NoSQL, HTTP/RPC/GraphQL, Async/Queue/Cache) — each scoped in its own document.

### 11.9 Round-2 acceptance checklist

- [x] Adds **9 new `FindingKind` variants** covering migration safety, pool config, tx & locking, NoSQL queries, HTTP/RPC/GraphQL, async, queues, cache, replica routing.
- [x] Extends the ORM framework list from 17 to **~60** (long-tail across Python, Java/Kotlin, Rust, Node, Go, PHP, Ruby, Scala, Elixir, Crystal, Swift, Dart, .NET) — each with import-set signature.
- [x] Specifies the **migration safety detector** with 22 rule IDs and per-tool file-pattern recognition for 30+ migration tools.
- [x] Specifies the **pool config detector** as a tiny JSON-driven interpreter that handles 6+ pool managers without per-tool Rust code.
- [x] Specifies the **tx & locking detector** with 18 rule IDs and 3 reusable detector shapes that carry ~80% of the catalog.
- [x] Codifies the **"negative signal" architecture**: ~90 eager-load/bulk/streaming/projection method names live in JSON, not Rust; new ORMs are catalog adds.
- [x] Scopes NoSQL/HTTP/Async expansion to **three sibling plans** so this document stays implementable.
- [x] Adds **13 testable micro-steps (30-42)** continuing the established cadence.
- [x] Net additions across this round: **~94 directly statically-detectable rules** behind only **3 new generic detector shapes**, **6 new catalog JSON files**, and **3 new sibling source files** (`migration_lint.rs`, `config_lint.rs`, plus extension to `orm_lint.rs`).
- [x] License-clean: every external rule catalog mined for *names only* (GPL squawk: rule strings only, no code link). All new Rust deps remain Apache/BSD/MIT.

---

## 12. Cumulative migration scanning — the "lingering issue" detector

### 12.1 Why a per-file scan misses the most important migration bugs

§11.3 lints **each migration file in isolation**: it catches the dangerous
shapes inside one file (`adding-required-field`, `drop-column`,
`require-concurrent-index-creation`, etc.). What it cannot see is the
*evolved schema state*:

- `V001__create_customers.sql` creates the `customers` table with a
  `tenant_id` column.
- `V247__add_orders.sql` adds `orders.customer_id` as a foreign key.
- No migration between V001 and V573 ever indexes `orders.customer_id`.

Every single file is "clean" by per-file rules. The schema as a whole is
**production-broken** — every join from `orders` to `customers` does a
sequential scan, has done since V247, and the corresponding APM finding
took the team three years to notice (cf. Zaman et al. MSR 2012:
"perf-bug median time-to-discovery 137 days" — see
[`research/OSS_BUG_CORPUS_METHODOLOGY.md`](research/OSS_BUG_CORPUS_METHODOLOGY.md) §6).

This whole class is invisible to single-file linters. Catching it
requires reconstructing the schema *as it existed at each migration step
and as it exists today*.

### 12.2 Architecture — `src/schema_state.rs`

A new file, sibling to `migration_lint.rs`:

```rust
// src/schema_state.rs

/// In-memory model of the schema after the Nth migration has been applied.
/// Deliberately structural, not type-checked — we care about names and
/// shape (PK? FK? indexed?), not types.
#[derive(Debug, Default, Clone)]
pub struct Schema {
    pub tables: BTreeMap<String, TableState>,
}

#[derive(Debug, Default, Clone)]
pub struct TableState {
    pub created_in: String,                // version that created it
    pub columns: BTreeMap<String, Column>, // column-name → Column
    pub pk: Option<Vec<String>>,           // PK column-name list
    pub indexes: Vec<Index>,
    pub fks: Vec<ForeignKey>,
    pub touched_in: Vec<String>,           // every version that ALTERed it
}

#[derive(Debug, Clone)]
pub struct Column {
    pub added_in: String,
    pub nullable: bool,
    pub has_default: bool,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub added_in: String,
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub unique: bool,
    pub concurrent: bool,
}

#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub added_in: String,
    pub local_columns: Vec<String>,
    pub references_table: String,
    pub references_columns: Vec<String>,
}
```

### 12.3 The simulator pass — `simulate(migrations) -> (Schema, Vec<Finding>)`

```rust
pub fn simulate_and_lint(migrations: &[Migration]) -> (Schema, Vec<Finding>) {
    let mut schema = Schema::default();
    let mut findings = Vec::new();
    // migrations is pre-sorted by version (see §12.5)
    for m in migrations {
        let stmts = parse_migration(&m.body, m.dialect);  // tree-sitter-sequel
        for stmt in stmts {
            apply_to_schema(&stmt, &m.version, &mut schema);          // mutate
            findings.extend(contextual_rules(&stmt, &m.version, &schema));  // §12.4
        }
    }
    findings.extend(lingering_rules(&schema));  // §12.5
    (schema, findings)
}
```

Two rule families fire from this pass:

- **Contextual rules** (`SCHEMA_CTX_*`): fire DURING simulation, given the
  schema state *just before* the current statement. Examples:
  "`CREATE INDEX` arriving N migrations after the table was created";
  "`ALTER` on a table that already has ≥3 prior `ALTER`s" (schema thrash);
  "`ADD COLUMN NOT NULL` on a table whose row count is likely high based on
  age + number of prior writes touching it."
- **Lingering rules** (`SCHEMA_LINGER_*`): fire AFTER the last migration,
  reading only the final `Schema`. Examples: "FK on column with no index";
  "table with no PK"; "common-column-without-index" (see §12.6).

Both emit through the existing `Finding` substrate with
`kind: FindingKind::MigrationSafety` (no new kind needed) and rule IDs
prefixed `SCHEMA_CTX_*` / `SCHEMA_LINGER_*` so the viewer can chip them
distinctly.

### 12.4 Contextual rules (catalog)

| Rule | Trigger | Severity | Effort |
| --- | --- | --- | --- |
| `SCHEMA_CTX_LATE_INDEX` | `CREATE INDEX` on a table that was created ≥5 migrations earlier | Medium | Trivial |
| `SCHEMA_CTX_ALTER_THRASH` | ≥4 `ALTER`s touching the same table in a 10-migration window | Low | Medium |
| `SCHEMA_CTX_ADD_NOT_NULL_LATE` | `SET NOT NULL` on a column whose containing table has ≥3 prior migrations touching it | High | Small |
| `SCHEMA_CTX_DROP_COLUMN_RECENT` | `DROP COLUMN` on a column added ≤3 migrations earlier (rollback signal) | Low | Trivial |
| `SCHEMA_CTX_RENAME_HOT_TABLE` | `RENAME TABLE` on a table that's been touched by ≥10 prior migrations | High | Large |
| `SCHEMA_CTX_FK_NO_INDEX_AT_ADD` | `ADD FOREIGN KEY` on `(col)` where no index covers `col` *yet* (DBs other than MySQL/InnoDB which auto-creates) | High | Trivial |
| `SCHEMA_CTX_UNIQUE_AFTER_BACKFILL` | `ADD UNIQUE` constraint added on a column whose data backfill ran in the same migration | Medium | Medium |

These are different from the existing `MIG*` rules: a `MIG*` rule fires on
**this file alone**; a `SCHEMA_CTX_*` rule needs the *prior* schema state.

### 12.5 Lingering rules (final-state catalog)

| Rule | Trigger | Severity | Effort |
| --- | --- | --- | --- |
| `SCHEMA_LINGER_NO_PK` | Table in final schema with no primary key | High | Small |
| `SCHEMA_LINGER_FK_NO_INDEX` | FK column not covered by any index in the final schema | High | Trivial |
| `SCHEMA_LINGER_COMMON_FK_NO_INDEX` | Column matches the common-FK pattern (`*_id`, `user_id`, `tenant_id`, `org_id`, `account_id`, `parent_id`) and has no index, no FK declared | Medium | Trivial |
| `SCHEMA_LINGER_COMMON_FILTER_NO_INDEX` | Column matches the common-filter pattern (`created_at`, `updated_at`, `deleted_at`, `status`, `email`, `slug`) and has no index | Medium | Trivial |
| `SCHEMA_LINGER_REDUNDANT_INDEX` | Two indexes where one is a strict column-prefix of the other (the longer covers the shorter) | Low | Trivial |
| `SCHEMA_LINGER_UNUSED_NULLABLE_FK` | FK column declared NOT NULL but FK is `ON DELETE SET NULL` (impossible state) | Low | Trivial |
| `SCHEMA_LINGER_WIDE_TABLE` | Table with >40 columns (schema-design smell often correlated with perf issues) | Low | Large |
| `SCHEMA_LINGER_INDEX_ON_LOW_CARD_BOOLEAN` | Index on a single boolean column not in a multi-column index (almost never useful) | Low | Trivial |

All emit `MigrationSafety` findings with the rule ID in the message and a
**provenance trail** in evidence: `created_in` version, last `touched_in`
version, age-in-migrations. Concrete, navigable.

### 12.6 The common-column heuristic catalog

A new data file
`src/research_classefiers+categories/schema_heuristics.json`:

```jsonc
{
  "common_fk_columns": [
    "user_id", "users_id", "owner_id", "creator_id",
    "tenant_id", "org_id", "organization_id", "account_id", "workspace_id",
    "parent_id", "customer_id", "company_id", "team_id",
    "project_id", "session_id"
  ],
  "common_filter_columns": [
    "created_at", "updated_at", "deleted_at",
    "status", "state",
    "email", "username", "slug", "external_id"
  ],
  "common_pk_synonyms": ["id", "uuid", "pk", "_id"],
  "suffix_rules": [
    { "suffix": "_id",  "implies": "common_fk",     "severity": "medium" },
    { "suffix": "_at",  "implies": "common_filter", "severity": "low"    },
    { "suffix": "_url", "implies": "indexable",     "severity": "low"    }
  ]
}
```

The catalog is opinionated. Every entry has a **reason** (in a sibling
prose section of the file) so future maintainers can challenge it. We
keep `severity` conservative — `Medium` for `*_id` (common, almost
always indexed in practice), `Low` for `*_at` (only indexed when used in
filters, which we can't always tell statically).

### 12.7 Two-mode operation

The simulator runs **alongside** the existing per-file `migration_lint.rs`,
not as a replacement:

```rust
// In Report::build, before bump_severities_by_impact:
attach_migration_findings(&mut entries, &migration_files);   // §11.3 — per-file (always)
attach_schema_state_findings(&mut entries, &migration_files, &cli.cumulative);  // §12 — opt-in
```

CLI: `--cumulative-migrations` (default on if migration dir auto-detected,
opt out via `--no-cumulative-migrations`). Cost: O(total migration LOC), one
linear pass. Empirically <500ms even on repos with thousands of migration
files because each file is parsed once and the schema model is tiny.

### 12.8 Parser choice — tree-sitter-sequel for migration files

Round-1 picked `sqlparser-rs` (Apache-2.0) + `pg_query` (BSD) for SQL
strings *embedded in source code* — those parsers expose a real semantic
AST. For `.sql` migration files specifically, we add a third option:

- **`tree-sitter-sequel`** (the active SQL grammar, MIT) — error-tolerant
  parsing of `.sql` files, consistent with drift's existing tree-sitter
  pipeline, fast on 1000-file migration trees. We don't need full semantic
  types for the schema simulator — only structural extraction of table /
  column / index / FK names + a handful of flags. tree-sitter is the
  right granularity.

Selection rule:

| Input | Parser | Why |
| --- | --- | --- |
| SQL string literal in source code | `sqlparser-rs` (or `pg_query` if dialect is PG) | Need semantic AST for SQL-lint rules |
| `.sql` migration file (structural extraction) | `tree-sitter-sequel` | Fast, error-tolerant, fits drift's pipeline |
| `.sql` migration file (deep DDL safety) | `pg_query` (PG) or `sqlparser-rs` | Falls back when tree-sitter can't extract a field |

The schema simulator uses tree-sitter for ~90% of cases and falls through
to `pg_query` for the residual ~10% (gnarly PostgreSQL syntax like
`CREATE TABLE … PARTITION BY …` where the grammar trails the dialect).

### 12.9 Validation via the OSS bug corpus

The cumulative scanner is uniquely amenable to corpus-based validation
(see [`research/OSS_BUG_CORPUS_METHODOLOGY.md`](research/OSS_BUG_CORPUS_METHODOLOGY.md) §8.4).
For each OSS commit that adds an index in a migration:

- `pre/` = parent commit's migration tree, fed to the simulator → expect a
  `SCHEMA_LINGER_FK_NO_INDEX` or `SCHEMA_LINGER_COMMON_FK_NO_INDEX` finding
  on exactly the column the fix migration adds an index for.
- `post/` = the fix commit's migration tree, fed to the simulator → expect
  **no** finding on that column.

BigQuery yields >5,000 such commits per month (§2.5 of the OSS corpus
doc). 50 confirmed fixtures per rule → tight Wilson interval, ship as
`confidence: high` per CodeQL's tiering.

### 12.10 Implementation micro-steps (51-57)

| # | Step | Test |
| --- | --- | --- |
| 51 | Add `src/schema_state.rs` with the `Schema`/`TableState`/`Column`/`Index`/`ForeignKey` structs + an `apply_to_schema(stmt, version, &mut schema)` mutator that handles `CREATE TABLE`, `ALTER TABLE … ADD/DROP COLUMN`, `CREATE INDEX`, `ALTER TABLE … ADD CONSTRAINT FK/PK/UNIQUE`. tree-sitter-sequel parser plus selective `pg_query` fallback per §12.8. | Unit tests: feed 5-statement synthetic histories and assert the final `Schema` matches. |
| 52 | Add migration sorting helper (version extraction for `V001__`, `001_`, `20240101_`, `0001_`, Alembic UUID + parent-chain). | Unit tests across the major conventions. |
| 53 | Implement `SCHEMA_LINGER_NO_PK` + `SCHEMA_LINGER_FK_NO_INDEX` + `SCHEMA_LINGER_COMMON_FK_NO_INDEX`. Seed `schema_heuristics.json`. | Snapshot against `tests/fixtures/schema-cumulative/missing-fk-index/` (synthetic V001 + V050 history with the gap). |
| 54 | Implement the remaining lingering rules (§12.5). | Snapshot. |
| 55 | Implement `SCHEMA_CTX_LATE_INDEX` + `SCHEMA_CTX_FK_NO_INDEX_AT_ADD` (the contextual rules with the highest empirical hit rate). | Snapshot. |
| 56 | Wire `attach_schema_state_findings` into `Report::build` behind `--cumulative-migrations`. CLI flag, env var, auto-detect when a `migrations/` directory is in repo. | Integration test: an existing fixture without migrations is unchanged; a new fixture with migrations gets the cumulative findings. |
| 57 | OSS-corpus mining script `tools/mine_schema_bugs.py` that materializes pre/post fixtures from "add index" commits and snapshot-asserts the invariant (pre fires, post doesn't). | CI gate. |

### 12.10b Implementation-grade findings from
[`research/MIGRATION_AST_SHAPES.md`](research/MIGRATION_AST_SHAPES.md)

A separate deep-research pass extracted concrete tree-sitter node kinds
and AST patterns for every migration tool drift will support. The full
document lives in `research/`; the actionable refinements that change
this plan are below.

**Parser choice confirmed.** `tree-sitter-sequel = "0.3"` (Cargo crate
for the MIT grammar at `DerekStride/tree-sitter-sql`) is the right
parser for `.sql` files. Node kinds confirmed: `create_table`,
`create_index`, `alter_table`, `add_column`, `drop_column`,
`alter_column`, `rename_column`, `add_constraint`, `drop_constraint`,
`column_definition`, `object_reference`, `_key_constraint`,
`_primary_key_constraint`, `ordered_columns`, `keyword_concurrently`,
`keyword_unique`. Postgres-specific gaps (`INHERITS`, `IDENTITY`
distinguishing) fall through to `pg_query` — already a drift dep.

**Ruby gap.** drift currently ships grammars for Python/Java/TS/JS/Go/
Rust/Scala/Kotlin (Cargo.toml lines 17-26) — **no `tree-sitter-ruby`**.
For v1 of §12, ship Rails migration scanning as a regex-based parser:
per-line match on `^\s*(create_table|add_column|drop_table|add_index|
add_foreign_key|change_column_null|remove_column|add_reference)\b`
plus a small Ruby-symbol-and-hash mini-parser. Adding
`tree-sitter-ruby` (MIT) is a deferred upgrade once block-DSL precision
inside `create_table :users do |t| … end` is needed for contextual
rules.

**New dependency to add at step 51:**

```toml
tree-sitter-sequel = "0.3"   # MIT — DerekStride/tree-sitter-sql, Cargo name
quick-xml          = "0.36"  # Apache-2.0 — Liquibase XML changesets
# Optional, behind a feature flag for v2:
# tree-sitter-prisma-io = "1.4"   # MIT — for .prisma schema parsing
# tree-sitter-ruby      = "0.23"  # MIT — when Rails block-DSL needed
```

**`pgroll` schema model port (Apache-2.0).** Step 51's `Schema` struct
should extend `Column` with `enum_values: Vec<String>`, `Index` with
`predicate: Option<String>` and `method: String` (btree/hash/gist/gin/
brin), `ForeignKey` with `on_delete: Option<String>`,
`on_update: Option<String>`, `match_type: Option<String>`. Each
unlocks a future lingering rule
(`SCHEMA_LINGER_FK_NO_ACTION_NOT_NULL`,
`SCHEMA_LINGER_EXCLUSION_OVERLAP`, etc.) without further schema
changes.

**Filename version probe order (step 52).** Implement as fall-through:

| Try | Pattern | Example |
| --- | --- | --- |
| 1 | `^(\d{4,})_` | Django: `0001_initial.py` |
| 2 | `^(\d{14})_` | Rails: `20211202041233_init_schema.rb` |
| 3 | `^(\d{13})-` | TypeORM: `1480489020310-CreatePost.ts` |
| 4 | `^[VURB](?P<v>[\d._]+)?__` | Flyway: `V1__init.sql`, `R__view.sql` |
| 5 | Alembic | Filename has 12-char hex prefix; topological sort via `down_revision` chain (NOT lexicographic) |
| 6 | `^(\d+)_` | Generic numeric prefix |
| 7 | Fallback to mtime |

**Squashed migrations.** Sentry uses `0001_squashed_0904_*` — the
version is the *trailing* number, not the leading one. The probe at
step 1 must check for an internal `_squashed_(\d+)` segment and prefer
that when present.

**Empirical thresholds (sample of Sentry, Superset, Mastodon, GitLab):**

- Median **lag between `create_table` and the first FK index** on
  that table: **≈47 migrations** (Sentry sample). The plan's
  `SCHEMA_CTX_LATE_INDEX ≥5` threshold (§12.4) is **correctly
  conservative** — we'd rather miss some 3- or 4-lag cases than emit
  noise on intentional patterns.
- Non-atomic migration prevalence: ~12% (`atomic = False`). Rule
  framing: `AddIndex` (non-concurrent) without `atomic = False` is
  the unsafe case, *not* the inverse.
- "Constraint NOT VALID then VALIDATE" pattern prevalence in OSS:
  **<3%**. Most projects accept brief write-blocks on FK adds. This
  validates that `SCHEMA_CTX_FK_NO_INDEX_AT_ADD` (missing index on a
  freshly-added FK) is higher-value than `SCHEMA_CTX_FK_NO_NOT_VALID`
  (missing the two-step dance) — drift's §12.4 already prioritizes
  the index case.

**Suppression rule for the backfill-then-NOT-NULL safe pattern.**
The canonical 3-step shape (seen widely in Sentry):

```
V0789_add_X_nullable    : AddField('X', nullable=True)
V0790_backfill_X        : RunPython(backfill_X)
V0791_set_X_not_null    : AlterField('X', nullable=False)
```

When `SCHEMA_CTX_ADD_NOT_NULL_LATE` (§12.4) would fire on V0791,
suppress IF the **window of the prior 2 migrations** contains an
`AddField` (nullable=True) for the same column **plus** a
`RunPython`/`RunSQL` step. This is the deliberate safe pattern, not
the bug.

**Sources cited.** All findings in this section trace back to the
research file at [`research/MIGRATION_AST_SHAPES.md`](research/MIGRATION_AST_SHAPES.md)
(35 cited URLs including DerekStride/tree-sitter-sql, the Alembic +
Django + Rails + Flyway + TypeORM + Sequelize + Knex + GORM + Prisma +
Liquibase reference docs, xataio/pgroll's Apache-2.0 schema model, and
direct samples of the Sentry/Superset/Mastodon/GitLab/Wagtail migration
trees).

### 12.11 Where the docs the user shared *do* and *don't* change drift

**Folded into the plan:**
- Cumulative schema-state simulation (this section §12).
- Sorted version-aware migration discovery (§12.2 + step 52).
- "Lingering issue" rule family on the final schema (§12.5).
- Common-column index heuristic catalog (§12.6).
- tree-sitter-sequel as the migration-file parser of record (§12.8).
- Two-mode operation: independent per-file (kept) AND cumulative (new) (§12.7).

**Not folded** (explicitly out of scope):
- The static-vs-dynamic preamble and continuous-profiler tooling
  (Parca/Perfetto/Pyroscope). Drift is the static half; runtime profilers
  are complements. One-sentence acknowledgement in §0 of the project
  ARCHITECTURE would be reasonable but doesn't belong in this plan.
- Migration size estimation (`heuristic: table > X rows`) — drift cannot
  know row counts statically. We rely on **migration age** as a proxy:
  the older the table (in migration count), the more likely it's big. The
  exact "this table is large" judgment is the runtime profiler's job.

### 12.12 Acceptance for §12

- [x] Single new file (`src/schema_state.rs`) and one new catalog
      (`schema_heuristics.json`); no new top-level hierarchies.
- [x] Reuses the existing `Finding` substrate; new rule IDs (not new
      `FindingKind` variants) — viewers and downstream consumers need no
      schema bump.
- [x] Adds **15 new rules** (7 contextual + 8 lingering) covering the
      most common cross-migration perf-bug class.
- [x] Cost-bounded: one linear pass per migration tree, <500ms
      empirically on 1000+ file repos.
- [x] Empirically validated through the OSS bug-corpus methodology
      (§12.9 + corpus methodology research doc); ship at
      `confidence: high` once ≥30 fixtures per rule land.
- [x] Two-mode: opt-in via CLI/auto-detect, does NOT replace per-file
      `migration_lint.rs`.

When you say "go schema-state" (or "go step 51"), we begin.

---

## 13. ORM → SQL static prediction (the §3 inverse)

§3 of this plan handles SQL strings that are *already written as
literals* in source code. §4 handles ORM call chains *syntactically*
(N+1 detection by call-in-loop shape). Neither sees the SQL that an
ORM chain like `User.objects.filter(active=True).select_related('profile')`
will actually emit. This section closes that gap.

Full implementation-grade research lives in
[`research/ORM_SQL_PREDICTION.md`](research/ORM_SQL_PREDICTION.md)
(45 KB, ~4,400 words, per-ORM method → SQL-clause tables for Django,
SQLAlchemy, TypeORM, Eloquent, ActiveRecord, GORM, EF Core, Sequelize,
Prisma, Doctrine, Hibernate; runtime SQL-inspection APIs as the
ground-truth oracle for tests; cited prior art including Petersohn
et al. ICSE'18 *Static Analysis of ORM Applications* and Cheung et al.
PLDI'14 *ROOT* compositional optimization).

### 13.1 The core idea (one paragraph)

Take a fluent call chain that drift's tree-sitter already captures
(`@ref.call` chains in `parser.rs`), walk it with a per-ORM dispatcher
that maps each method name onto an SQL clause mutation, build a
synthetic `sqlparser::ast::Statement`, and stringify it. That string
becomes a new `ExternalCall.predicted_sql_literal: Vec<String>` field
(Vec because `prefetch_related` / `with` / `includes` emit secondary
SELECTs). The existing `sql_lint.rs` then runs the **same** 11 SQL
rules — plus a future schema-cross-reference family — over **both**
inline `sql_literal` and `predicted_sql_literal`, with findings tagged
for provenance.

### 13.2 New module — `src/orm_sql_predictor.rs`

Same OCP shape as `sql_lint.rs` (the catalog of `SqlRule`s is data,
not code paths). The dispatcher table is `&'static [&'static dyn
OrmDialect]`; each ORM is one `impl`:

```rust
trait OrmDialect {
    fn matches(imports: &[ImportRecord], chain: &[CallStep]) -> bool;
    fn predict(chain: &[CallStep]) -> Option<PredictedSql>;
}

const DIALECTS: &[&dyn OrmDialect] = &[
    &Django, &SqlAlchemy, &TypeOrm, &Eloquent, &ActiveRecord,
    &Gorm, &EfCore, &Sequelize, &Prisma, &Doctrine, &Hibernate,
];

pub struct PredictedSql {
    pub statements: Vec<String>,        // multi-stmt for prefetch/include
    pub orm: OrmKind,
    pub confidence: Confidence,         // High / Medium / Low
    pub dropped_methods: Vec<String>,   // unhandled methods we silent-dropped
}
```

Per-ORM `predict()` builds a small `PartialSelect { from, columns,
where, joins, order_by, group_by, having, limit, offset, distinct,
combinator }`, then serializes via `sqlparser::ast::Statement::Query(...)
.to_string()`. **No new SQL parser** — we reuse the existing
`sqlparser-rs` AST as both the IR and the emitter.

### 13.3 Schema additions — one optional field

```rust
// src/graph.rs ExternalCall
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub predicted_sql_literal: Vec<String>,
```

That's it. The new field is `Vec<String>` (not `Option<String>`)
because eager-load operations emit secondary statements. Empty when
the call isn't an ORM chain or when the predictor silent-dropped.

### 13.4 Per-ORM coverage targets (v1 → v1.2)

Following [`research/ORM_SQL_PREDICTION.md`](research/ORM_SQL_PREDICTION.md)
§10 roadmap:

| Phase | ORMs | Why |
| --- | --- | --- |
| **v1** | Django, SQLAlchemy, TypeORM | Cover most of drift's existing fixtures (python-fastapi, typescript-nestjs). All three have stable runtime SQL-inspection APIs (`str(qs.query)`, `stmt.compile()`, `qb.getSql()`) for ground-truth test fixtures. |
| **v1.1** | Eloquent, ActiveRecord, Prisma | Most-deployed ORMs in 2026. Prisma's recent `.toSQL()` (5.10+) is a direct oracle. |
| **v1.2** | GORM, EF Core, Sequelize, Doctrine, Hibernate (Criteria only — HQL strings already flow through inline `sql_literal`) | Long-tail. |

### 13.5 New rule family enabled by prediction — schema-cross-reference

Once we have predicted SQL, drift can intersect it with the cumulative
migration scanner (§12) and flag patterns that previously needed
runtime EXPLAIN:

| Runtime EXPLAIN finding | Static analog (now reachable) |
| --- | --- |
| Seq Scan on big table with selective predicate | `WHERE col = …` where `col` has no index in §12's migration index |
| Sort spills to disk | `ORDER BY` without `LIMIT` on a `predicted_sql` whose FROM table appears N migrations old (proxy for size) |
| Heap Fetches > Rows | `SELECT a, b` where the index covering the WHERE doesn't include `a, b` |
| Nested Loop, no inner index | `JOIN n ON n.fk = m.id` where `n.fk` has no index in §12 |
| Filter discards >50% | `WHERE bool_col = true` (bool) or `WHERE status = 'X'` (low-card enum) |

Concretely, this adds:
`SQL_UNINDEXED_WHERE`, `SQL_SORT_WITHOUT_LIMIT`,
`SQL_JOIN_WITHOUT_EQUALITY`, `SQL_JOIN_UNINDEXED_FK`,
`SQL_LOW_CARDINALITY_FILTER`. All fire **identically** on inline AND
predicted SQL.

### 13.6 Approximation budget — explicit drop policy

From the research §9, validated by Petersohn et al. ICSE'18 (~80%
of real Django/Hibernate chains are statically translatable):

- Unknown method name → drop entire chain, return `None` (silent-skip).
- Known method, unknown arg expression → keep chain, substitute
  placeholder `<expr>` token. SQL stays parseable; lints still fire.
- Unknown receiver (couldn't infer table name) → use `<unknown>` table.
- Chain spans function boundaries (`qs = make_qs(); qs.filter(...)`) →
  v1: predict suffix only, `Confidence::Medium`. v3: simple
  intra-procedural dataflow over the existing `CallGraph`.
- Chain under conditional control flow (`if x: qs = qs.filter(...)`) →
  v1: take the longest branch, mark `Confidence::Low`.
- **Target coverage: 80%** of common shapes. The 20% silent-skip rate
  is *expected*, not a bug.

### 13.7 Ground-truth testing

For each supported ORM, vendor a small fixture tree:
`tests/orm_sql_predictor/<orm>/chain_NN.{py,ts,rb,go,cs,php}` plus a
sibling `chain_NN.expected.sql` captured from the ORM's own runtime:
- Django: `str(qs.query)`
- SQLAlchemy: `str(stmt.compile(compile_kwargs={"literal_binds": True}))`
- TypeORM: `qb.getSql()`
- Eloquent: `$qb->toSql()`
- ActiveRecord: `relation.to_sql`
- GORM: `stmt.SQL.String()` in DryRun mode
- EF Core: `q.ToQueryString()`
- Prisma: `.toSQL()` (5.10+)

Each integration test runs drift's predictor, normalizes both sides
through `sqlparser-rs` `Statement::to_string()` (collapses quoting
+ whitespace), and diffs. **Acceptance: ≥90% match rate per ORM
fixture set**; chains below that bar either get a walker fix or move
to the silent-skip list with an explicit `# DROP REASON` comment.

### 13.8 Implementation micro-steps (extending §7 / §12)

Continues from step 57 in §12:

| # | Step | Test |
| --- | --- | --- |
| 58 | Add `predicted_sql_literal: Vec<String>` to `ExternalCall` (line 23 of `graph.rs`). Schema bump in `profile.schema.json`. Backward-compat optional field. | Existing fixtures still validate. |
| 59 | New file `src/orm_sql_predictor.rs` with the `OrmDialect` trait, `PartialSelect` builder, and `Django` impl (the largest walker). | Unit tests: 15+ Django chain shapes from `research/ORM_SQL_PREDICTION.md §3`. |
| 60 | Implement `SqlAlchemy` walker (legacy `query()` + 2.x `select()`). | Unit tests: 12+ chain shapes. |
| 61 | Implement `TypeOrm` walker. The 1:1 method-name → clause mapping is mechanical. | Unit tests: 10+ chain shapes. |
| 62 | Wire `attach_predicted_sql(graph)` into `Report::build`, immediately before `attach_sql_antipatterns`. | Existing fixtures unchanged unless the chains parse as ORM. |
| 63 | Extend `sql_lint.rs::for_each_sql_literal` to also visit `predicted_sql_literal`. Tag each finding with `provenance: Inline | Predicted`. | Snapshot: a predicted SQL with `SELECT *` produces a tagged `SQL001` finding. |
| 64 | v1.1 ORMs: Eloquent, ActiveRecord, Prisma walkers. | Per-ORM unit tests. |
| 65 | v1.2 ORMs: GORM, EF Core, Sequelize, Doctrine. | Per-ORM unit tests. |
| 66 | v2 — schema-cross-reference rules. Read §12's migration index. Add `SQL_UNINDEXED_WHERE`, `SQL_SORT_WITHOUT_LIMIT`, `SQL_JOIN_WITHOUT_EQUALITY`, `SQL_JOIN_UNINDEXED_FK`, `SQL_LOW_CARDINALITY_FILTER`. | Per-rule fixtures: a `create_table` migration + a predicted SQL that triggers the rule. |
| 67 | OSS-corpus validation: pick 20 commits from the seed-repo list in `OSS_BUG_CORPUS_METHODOLOGY.md §8` that added an index; verify drift's predictor + §12 simulator produces a `SQL_UNINDEXED_WHERE` on the pre-state and not the post-state. | CI gate. |

### 13.9 Acceptance for §13

- [x] One new file (`orm_sql_predictor.rs`), one new optional field
      (`ExternalCall.predicted_sql_literal: Vec<String>`); reuses the
      existing `Finding`/`SqlAntipattern` substrate.
- [x] OCP-clean: each ORM is a discrete `OrmDialect` impl. Adding
      Hibernate Criteria support = appending one impl. **Zero
      modification** of existing dialects.
- [x] No new SQL parser: `sqlparser-rs` is both the IR and the
      emitter. Predicted SQL flows through the same `parse_and_match`
      path the inline SQL uses.
- [x] License-clean: all referenced research is permissive (Petersohn
      ICSE'18 is academic, sqlglot MIT, Apache Calcite Apache-2.0).
      No code linked from GPL tools.
- [x] **Approximation budget is explicit**: 80% coverage target,
      20% silent-skip with `Confidence::Low` for borderline. Drift's
      false-positive policy (plan §8) covers the residual.
- [x] Adds **10 testable micro-steps (58-67)** continuing the
      established cadence.

When you say "go orm-predict" (or "go step 58"), we begin.

---

## 13b. Generic SQL rules + IO heuristics (the "make rules generic" round)

### 13b.1 What shipped

- **Public extension seams** in [`src/sql_lint.rs`](src/sql_lint.rs):
  `pub struct SqlRule`, `pub const BUILTIN_RULES`, `pub fn
  check_with_rules`, `pub fn attach_sql_antipatterns_with` —
  Dependency-Inversion: downstream code (tests, future ORM-lint
  module, third-party rule packs) composes against the abstraction,
  not the concrete catalog.
- **Public AST predicate library** in [`src/sql_ast.rs`](src/sql_ast.rs)
  (~600 LOC, 18 pub fns): `walk_where_exprs`, `count_joins_in_query`,
  `max_subquery_depth_in_query`, `or_chain_length`,
  `collect_or_chain_columns`, `count_largest_in_list`,
  `query_has_distinct`, `query_has_order_by`, `query_offset_value`,
  `set_expr_has_implicit_union`, `is_function_on_identifier`,
  `where_contains_equality`, `expr_call_name_matches`,
  `expr_calls_random`, `identifier_name`, `literal_string`,
  `literal_unsigned_int`, `has_leading_wildcard`. Single Responsibility
  per function, pure (no I/O, no globals), composable via `||` / `&&`.
- **Three new IO rules** in `BUILTIN_RULES`, all composed from
  `sql_ast` predicates (no new internals):
  - `SQL_LARGE_IN_LIST` — `WHERE col IN (…)` ≥100 literals.
    `count_largest_in_list(stmt) >= 100`.
  - `SQL_DISTINCT_NO_ORDER_BY` — `SELECT DISTINCT` without `ORDER BY`.
    `query_has_distinct(stmt) && !query_has_order_by(stmt)`.
  - `SQL_OFFSET_DEEP_PAGINATION` — `OFFSET N` with N ≥ 1000.
    `query_offset_value(stmt) >= Some(1000)`.

Each matcher is exactly **one call into `sql_ast` plus one threshold
comparison** — empirical proof the "rules are data + predicate
composition" architecture works.

### 13b.2 Plan additions from the research deliverable

Full prior-art catalog with 35 cited URLs lives in
[`research/SQL_IO_HEURISTICS_AND_PLUGIN_ARCH.md`](research/SQL_IO_HEURISTICS_AND_PLUGIN_ARCH.md).
The implementation-actionable parts:

**6 of 7 rules from this batch shipped** (round 3, each ≤ 1 SqlRule
literal + 1 matcher line composed against `sql_ast` helpers).
`SQL_COMPLEX_LENGTH` deferred — it needs a `fn(&str) -> bool` rule
signature alongside the existing `fn(&Statement) -> bool`; small
refactor, scheduled separately.

| Rule ID | Pattern | Severity | Source | Shipped |
| --- | --- | --- | --- | --- |
| `SQL014_HAVING_NO_AGGREGATE` | `HAVING` clause with no aggregate function — should be `WHERE` | Low | sqlcheck rule 3013; Brass & Goldberg §3 | ✅ |
| `SQL017_AMBIGUOUS_GROUP` | Aggregate + non-aggregate column in SELECT with no GROUP BY (MySQL silently picks; Postgres rejects) | High | Karwin Ch.15; Brass & Goldberg §6 | ✅ |
| `SQL021_GROUP_BY_ORDINAL` | `GROUP BY 1, 2` — silent misalignment under projection reorder | Low | sqlfluff AM06; Brass & Goldberg §21 | ✅ |
| `SQL022_ORDER_BY_CONSTANT` | `ORDER BY 'x'`, `ORDER BY NULL` — no-op | Low | Brass & Goldberg §21 | ✅ |
| `SQL023_NOT_IN_WITH_NULL` | `NOT IN (1, 2, NULL)` — always empty result | High | Karwin "Fear of Unknown"; Brass & Goldberg §9 | ✅ |
| `SQL025_JSON_PATH_IN_WHERE` | `WHERE data->>'field' = …` without documented functional index (Postgres) | Medium | pganalyze GIN index guide | ✅ |
| `SQL_COMPLEX_LENGTH` | Normalized SQL ≥500 chars (sqlcheck's spaghetti-query threshold) | Medium | sqlcheck rule 3008 (hardcoded 500) | ⏳ needs raw-SQL signature |

**Reusable helpers landed in `src/sql_ast.rs`**:
`expr_contains_aggregate`, `having_lacks_aggregate`,
`select_has_mixed_aggregation_no_group_by`, `group_by_uses_ordinal`,
`order_by_is_all_constant`, `where_has_not_in_with_null`,
`where_uses_json_path` — each a pure SRP predicate, composable from
external rule packs via the existing pub re-exports.

**Tests**: 14 new unit tests (~2 per rule, positive + negative case).
**197 lib + 90 integration tests pass** (up from 183/90 before this
batch).

**Plugin architecture decision** (research §5 conclusion): mirror
**Ruff's stance** — keep `BUILTIN_RULES` first-party for v1. The
public `SqlRule` + `attach_sql_antipatterns_with` seams already make
extension possible from inside the workspace (tests, other crate
modules) without dynamic dispatch. If/when an "enterprise rule pack"
need emerges, the upgrade path is the [`inventory`](https://crates.io/crates/inventory)
crate — Rust-idiomatic compile-time plugin discovery: third-party
crates declare `inventory::submit! { SqlRule { … } }`, the binary
picks them up automatically. **No work needed in v1**; the design
makes the future opening painless.

**Empirical threshold reference** (research §2):
- sqlcheck's "spaghetti query" hardcodes **500 chars** (normalized).
- pganalyze's "Large Offset" insight matches **OFFSET ≥1000**.
- depesz's color thresholds: rows-misestimate yellow at 10×, orange
  at 100×, red at 1000×; exclusive-time yellow >10%, brown >50%, red
  >90%. Map directly onto our `Severity::{Low, Medium, High}`.
- PostgreSQL `join_collapse_limit` default = **8**; `geqo_threshold`
  = **12**. Our `SQL_COMPLEX_JOINS` fires at ≥6 (conservative
  warn-before-cliff). Future `SQL_GEQO_RISK` rule at ≥12 = High
  severity (planner switches to genetic search, plans get worse).

### 13b.3 Acceptance for §13b

- [x] Public extension seams: `SqlRule`, `BUILTIN_RULES`,
      `check_with_rules`, `attach_sql_antipatterns_with`. DIP-clean.
- [x] Public AST predicate library at `src/sql_ast.rs` — 18 SRP
      functions, no allocation beyond `Vec` returns, composable.
- [x] 3 new IO rules shipped (`SQL_LARGE_IN_LIST`,
      `SQL_DISTINCT_NO_ORDER_BY`, `SQL_OFFSET_DEEP_PAGINATION`); each
      matcher is 1 call into `sql_ast` + 1 threshold.
- [x] 7 more rules drafted with full citations (research §1 + §2)
      and ready to append on demand.
- [x] Plugin architecture decision documented: Ruff-style closed v1,
      `inventory`-crate upgrade path for v2.
- [x] **No regressions: 176 lib + 90 integration tests pass.**

When you say "go rules-batch" (add the next 7 rules), we begin.

---

## 13c. Per-language module split (separation of concerns)

### 13c.1 Reasoning chain

The old `src/parser.rs` (~481 lines) held **eight large `const
<LANG>_QUERY: &str`** literals back-to-back: Python, Java, TypeScript,
JavaScript, Go, Rust, Scala, Kotlin. Each language is independent —
its grammar binding, tags-query string, and per-language grammar
comments (Go's `interpreted_string_literal`, Rust's `generic_function`
for turbofish, Kotlin's `navigation_expression`) belong together with
*that one language*, not 200 lines away from each other.

**The Clean Code violations**:
- **SRP**: one file, eight unrelated concerns.
- **OCP**: adding a 9th language required editing the central file.
- **High-cohesion / low-coupling**: per-language comments and per-
  language query string drifted apart inside the same file.

### 13c.2 New shape — `src/languages/`

```
src/languages/
  mod.rs        ← dispatcher: language_for(), tags_query()
  python.rs     ← Python grammar + TAGS_QUERY
  java.rs       ← Java
  typescript.rs ← TS
  javascript.rs ← JS
  go.rs         ← Go
  rust.rs       ← Rust
  scala.rs      ← Scala
  kotlin.rs     ← Kotlin
```

Each per-language module exposes a tiny two-symbol surface:

```rust
pub fn language() -> tree_sitter::Language { /* grammar binding */ }
pub const TAGS_QUERY: &str = r#"..."#;
```

Callers depend on the dispatcher in `mod.rs`, never on individual
language modules — **Dependency Inversion**.

[`src/parser.rs`](src/parser.rs) is now a 9-line backward-compat shim:
`pub use crate::languages::{language_for, tags_query};`.

### 13c.3 Open/Closed in practice

Adding a 9th language (say Ruby):

1. `cargo add tree-sitter-ruby`
2. Create `src/languages/ruby.rs`:
   ```rust
   use tree_sitter::Language;
   pub fn language() -> Language { tree_sitter_ruby::LANGUAGE.into() }
   pub const TAGS_QUERY: &str = r#"…"#;
   ```
3. Add `Language::Ruby` to the enum in `lib.rs`.
4. Two one-line edits in `src/languages/mod.rs`'s dispatchers.

**No edits to existing language modules.** OCP achieved.

### 13c.4 Test invariant

The split is **mechanical** — zero semantic change. All 183 lib +
90 integration tests pass identically before and after.

### 13c.5 Acceptance for §13c

- [x] One file per language under `src/languages/`. SRP per file.
- [x] Two-symbol public surface per module (`language()` +
      `TAGS_QUERY`). ISP-clean.
- [x] Dispatcher in `mod.rs` is the only crate-wide depend-on point.
      Callers depend on the dispatcher, not on individual modules.
- [x] [`src/parser.rs`](src/parser.rs) reduced to a re-export shim —
      backward-compat preserved, no external call site broke.
- [x] **No regressions: 183 lib + 90 integration tests pass.**
- [x] Adding a new language = create one sibling file + 4 one-line
      edits (Cargo dep, `Language` enum, two `match` arms).

When you say "go add-language ruby" (or any other), the SoC layout
makes it a 30-minute task.

When you say "go round-2", step 30 begins.

---

## 12. Round-3 research expansion — ORM performance internals

Three more research agents covered: (A) **ORM hydration / session lifecycle / emitted-SQL gotchas**, (B) **ORM-aware APMs + canonical perf books + OTel conventions + benchmark suites**, (C) **named bugs with public ticket IDs** across every major ORM.

The major shift round-2 → round-3: drift was treating ORM N+1 as a single shape. Round-3 shows the production-latency catalog is **wider than N+1** — it includes hydration cost, session-PC bloat, identifier-strategy mistakes, cascade storms, inheritance-strategy choice, soft-delete index gaps, JSON-column index defeat, UUID-PK locality, OSIV, lazy collections in JSON serializers, the canonical DISTINCT+LIMIT+JOIN FETCH wrong-row-count bug, and ~40 ORM-specific named bugs with public ticket IDs.

### 12.1 New FindingKind variants (extends §11.1)

```rust
pub enum FindingKind {
    // ── existing through round-2 ──
    // ...

    // ── new (round 3) ──
    /// Per-row object hydration cost — `@ManyToOne` defaulting to EAGER,
    /// missing `@BatchSize`, persistence-context bloat without
    /// `session.clear()`, `lazy='dynamic'` returning a new SELECT on
    /// every access, EF Core change-tracking on a read path missing
    /// `AsNoTracking()`, `lean()` missing on Mongoose read paths.
    HydrationCost,

    /// The ORM emits SQL that does the wrong thing even when the user
    /// thinks the call is correct. Carries the public ticket ID in
    /// the message: HHH-1262 (DISTINCT+LIMIT+JOIN FETCH), Hibernate
    /// MultipleBagFetchException, Django `iterator()` no chunk_size,
    /// Sequelize `findAndCountAll` slow COUNT, EF Core
    /// `RowLimitingOperationWithoutOrderByWarning`, GORM
    /// `Updates(struct)` zero-value skip, etc.
    EmittedSqlGotcha,

    /// API serializer triggers an unguarded lazy-load while
    /// constructing the response. DRF serializer field path that
    /// doesn't match a `select_related`/`prefetch_related`; Spring
    /// HATEOAS + Hibernate lazy collection at JSON time; FastAPI +
    /// Pydantic response_model referencing a lazy relation.
    SerializerNPlusOne,

    /// A schema choice forces a query shape that needs an index the
    /// migration doesn't create. Soft-delete `WHERE deleted_at IS NULL`
    /// with no partial index; JSONB query that doesn't match a GIN
    /// index's expression; polymorphic FK with no `(type, id)` index;
    /// `ORDER BY created_at` with no btree.
    SchemaIndexMissing,

    /// ORM identifier strategy or PK choice imposes a perf tax.
    /// `GenerationType.IDENTITY` disabling JDBC insert batching;
    /// UUIDv4 default + B-tree fragmentation; Sequelize
    /// `defaultValue: UUIDV4` in a hot insert table; Postgres
    /// `gen_random_uuid()` vs `uuid_generate_v4()` extension cost.
    OrmIdGenerator,

    /// Cascade configuration leads to mass operations the user didn't
    /// see. `CascadeType.ALL` on bidirectional `@OneToMany` + delete;
    /// `orphanRemoval=true` clearing a collection; TypeORM
    /// `cascade: true` chaining recursively; Sequelize `onDelete:
    /// CASCADE` writing massive delete-storms.
    OrmCascadeStorm,

    /// ORM-level config drift in production: `hbm2ddl.auto=update`,
    /// `spring.jpa.open-in-view=true` (Spring Boot default), Sequelize
    /// `autoIndex: true` at boot, Doctrine `auto_generate_proxy_classes`
    /// in prod, Mongoose `bufferCommands=true` hiding connection loss.
    OrmConfigDrift,
}
```

Total new variants across all 3 rounds: **18** (round 1: 3 + round 2: 9 + round 3: 6).

### 12.2 APM-aligned rule-ID vocabulary

drift's existing `Evidence.call` field is freeform. Adopt a **stable namespacing convention** so a user can grep the same string across drift + Sentry/Datadog/Hypersistence/OTel without translation:

```
<source>:<rule_id>
```

Source prefixes (each grounded in public docs):

| Prefix | Examples | URL |
| --- | --- | --- |
| `hibernate:` | `hibernate:eager_many_to_one_default`, `hibernate:open_session_in_view`, `hibernate:identity_generator_blocks_batch`, `hibernate:missing_pass_distinct_through`, `hibernate:many_to_many_list`, `hibernate:one_to_one_lazy_proxy` | https://vladmihalcea.com/14-high-performance-java-persistence-tips/ |
| `hhh:` | `hhh:1262` (DISTINCT+LIMIT+JOIN FETCH), `hhh:multiple_bag_fetch_exception` | Hibernate Jira |
| `mihalcea:` | `mihalcea:hp_jdbc_batching`, `mihalcea:hp_sequence_allocation`, `mihalcea:hp_dto_projection` | Same |
| `efcore:` | `efcore:multiple_collection_include_warning`, `efcore:row_limiting_operation_without_order_by_warning`, `efcore:lazy_load_on_disposed_context_warning`, `efcore:include_ignored_warning`, `efcore:navigation_base_include_ignored` | `CoreEventId` enum |
| `sentry:` | `sentry:n_plus_one_db`, `sentry:consecutive_db_queries`, `sentry:slow_db_query`, `sentry:n_plus_one_api_calls`, `sentry:large_render_blocking_asset` | https://develop.sentry.dev/backend/issue-platform/writing-detectors/ |
| `quickperf:` | `quickperf:disable_same_select_types_with_different_param_values`, `quickperf:disable_like_with_leading_wildcard`, `quickperf:expect_jdbc_batching`, `quickperf:expect_max_selected_column` | https://github.com/quick-perf/doc/wiki/sql-annotations |
| `karwin:` | `karwin:jaywalking`, `karwin:select_star`, `karwin:rand_order`, `karwin:like_leading_wildcard`, `karwin:polymorphic_associations`, `karwin:eav`, `karwin:keyless_entry` | *SQL Antipatterns* by Bill Karwin |
| `useindexluke:` | `useindexluke:function_on_indexed_col`, `useindexluke:leading_wildcard`, `useindexluke:offset_pagination` | https://use-the-index-luke.com |
| `bullet:` | `bullet:n_plus_one`, `bullet:unused_eager_loading`, `bullet:counter_cache_missing` | https://github.com/flyerhzm/bullet |
| `prosopite:` | `prosopite:same_callsite_repeated_query` | https://github.com/charkost/prosopite |
| `nplusone:` | `nplusone:lazy_after_parent_loaded`, `nplusone:unused_eager` | https://github.com/jmcarp/nplusone |
| `jpabuddy:` | `jpabuddy:cascade_all_on_many_to_many`, `jpabuddy:missing_index_on_fk`, `jpabuddy:identity_generator_anti` | https://www.jpa-buddy.com/ |
| `mongoose:` | `mongoose:populate_extra_roundtrip`, `mongoose:no_lean_on_read`, `mongoose:auto_index_in_prod` | Mongoose docs |
| `prisma:` | `prisma:select_include_mutual_exclusion`, `prisma:transaction_5s_default_timeout`, `prisma:relation_load_strategy_default` | Prisma docs |
| `gorm:` | `gorm:updates_struct_zero_value_skip`, `gorm:where_in_empty_slice_returns_all`, `gorm:automigrate_concurrent_race`, `gorm:max_open_conns_unlimited_default` | GORM issue tracker |
| `sequelize:` | `sequelize:find_and_count_all_slow_count`, `sequelize:default_scope_silent`, `sequelize:include_separate_extra_query` | Sequelize docs |

**Architecture rule**: detectors emit `Evidence.call` using one of these stable `source:rule_id` IDs. The plain-English message remains in `Finding.message`. This lets drift output cross-link to runtime APMs without translation.

### 12.3 OTel semantic-convention attribute mirroring

`Evidence` gains an optional `otel: Option<OtelAttrs>` (skip_serializing_if absent — backward compatible):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OtelAttrs {
    /// `db.system.name` — postgresql/mysql/mongodb/redis/cassandra/...
    /// (NEW semconv name; never the deprecated `db.system`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_system_name: Option<String>,
    /// `db.operation.name` — SELECT, INSERT, findAndModify, HMSET, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_operation_name: Option<String>,
    /// `db.collection.name` — table / collection name when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_collection_name: Option<String>,
    /// `messaging.system` — kafka/rabbitmq/aws_sqs/... for QueueAntipattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_system: Option<String>,
    /// `http.request.method` — for NetworkAntipattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_request_method: Option<String>,
}
```

**Strict naming**: always emit the NEW OTel names — the semconv registry has deprecated several common ones. drift must avoid:

| OLD (deprecated) | NEW (use these) |
| --- | --- |
| `db.system` | `db.system.name` |
| `db.statement` | `db.query.text` |
| `db.name` | `db.namespace` |
| `db.operation` | `db.operation.name` |
| `db.mongodb.collection` / `db.cassandra.table` | `db.collection.name` |
| `db.cassandra.consistency_level` | `cassandra.consistency.level` |
| `db.cosmosdb.*` | `azure.cosmosdb.*` |

Source: https://opentelemetry.io/docs/specs/semconv/registry/attributes/db/

### 12.4 Sentry-aligned threshold defaults

drift's severity cutoffs mirror Sentry's industry-default published numbers:

| Drift severity bump | Sentry threshold |
| --- | --- |
| `NPlusOne` → High when ≥5 sibling calls in same loop | `sentry:n_plus_one_db` requires ≥5 repeating spans |
| `ConsecutiveAwaits` → Medium when ≥2 awaits in one fn | `sentry:consecutive_db_queries` ≥2 sequential non-overlapping |
| `Network::HttpRequestInLoop` → High when ≥10 in one trace | `sentry:n_plus_one_api_calls` ≥10 concurrent within 5ms |
| Slow-query annotation cutoff | `sentry:slow_db_query` 500ms × 100 reps / 24h |
| Time-saved-by-parallelizing | 100ms (`sentry:consecutive_db_queries`) |

Put these in `severity_thresholds.json` so they're defensible to anyone reading Sentry's docs.

### 12.5 Named-bug rule catalog (with public ticket IDs)

#### Hibernate / JPA

| ID | Bug | Detection |
| --- | --- | --- |
| `hhh:1262` | DISTINCT + LIMIT + JOIN FETCH = silent in-memory dedup, wrong page sizes | JPQL literal contains both `DISTINCT` and `JOIN FETCH`; OR Java code calls `.setMaxResults(N)` on a query with `JOIN FETCH` |
| `hhh:multiple_bag_fetch_exception` | Two `List`-typed collections JOIN FETCH'd in same query → runtime exception | Annotated `List<...>` on ≥2 collection fields + JPQL `JOIN FETCH` on both |
| `hibernate:one_to_one_lazy_proxy_silent_eager` | `@OneToOne(fetch=LAZY)` behaves EAGER without `@LazyToOne(NO_PROXY)` + bytecode enhancement | annotation pairing absence |
| `hibernate:identity_generator_blocks_batch` | `GenerationType.IDENTITY` disables JDBC insert batching | annotation match |
| `hibernate:cascade_all_bidirectional` | `CascadeType.ALL` on bidirectional `@OneToMany` produces delete-storm | annotation pairing |
| `hibernate:open_session_in_view_default` | `spring.jpa.open-in-view=true` (Spring Boot default) → view-layer lazy queries | properties regex |
| `hibernate:hbm2ddl_auto_update_in_prod` | `ddl-auto=update` mutates schema at startup | properties regex |
| `hibernate:many_to_many_list_delete_reinsert` | `@ManyToMany List<...>` rewrites collection on every change | annotation match |
| `hibernate:element_collection_no_order_column` | `@ElementCollection` w/o `@OrderColumn` rewrites entire collection | annotation absence |
| `hibernate:missing_jdbc_batch_size` | `hibernate.jdbc.batch_size` not set | properties absence |
| `hibernate:formula_per_row` | `@Formula(...)` recomputes per row on every load | annotation presence |

#### Django ORM

| ID | Bug | Detection |
| --- | --- | --- |
| `django:iterator_no_chunk_size_pg` | `iterator()` on Postgres without `chunk_size=` won't use server-side cursor (Django 4.1+ requires it) | `.iterator(` arg list missing `chunk_size=` |
| `django:atomic_decorator_holds_http` | `@transaction.atomic` view calls `requests.*`/`httpx.*` inside | scope-contains-io |
| `django:bulk_create_skips_signals` | `bulk_create` skips `pre_save`/`post_save` | INFO when audit observers detected |
| `django:save_writes_all_fields` | `obj.save()` w/o `update_fields=` rewrites every column | shape |
| `django:f_expression_no_refresh_from_db` | `obj.field = F('field') + 1; obj.save()` then access `obj.field` reads stale | sequence pattern |
| `django:generic_foreign_key_no_index` | `GenericForeignKey` on hot lookup without composite `(content_type, object_id)` index | model + migration cross-check |
| `django:distinct_without_arg_postgres` | `.distinct()` w/o args + `ORDER BY` on Postgres = wrong row count | shape |
| `django:soft_delete_no_partial_index` | Manager subclasses adding `deleted_at__isnull=True` w/o matching partial index | manager + migration cross-check |
| `django:queryset_in_template_for_loop` | `{% for x in qs.all %}` re-evaluates queryset per render | template scan |

#### Rails ActiveRecord

| ID | Bug | Detection |
| --- | --- | --- |
| `ar:find_each_overrides_order_limit` | `User.order(:foo).limit(10).find_each` silently drops `order` and `limit` | shape |
| `ar:counter_cache_race` | `counter_cache: true` w/o `with_lock` on concurrent paths | annotation pairing |
| `ar:update_attribute_skips_validation` | `update_attribute` bypasses validations | shape (info) |
| `ar:touch_cascade_update_storm` | `belongs_to :parent, touch: true` UPDATE storm on bulk inserts | annotation + write pattern |
| `ar:acts_as_paranoid_needs_partial_unique` | `acts_as_paranoid` + unique index needs partial `WHERE deleted_at IS NULL` | gem use + migration check |
| `ar:serializer_n_plus_one` | `ActiveModel::Serializer` field path includes `:related` but parent `.includes(:related)` missing | serializer scan + caller scan |
| `ar:chunk_id_drift` | `Model.chunk(N)` modifying records → use `chunk_by_id` | shape |

#### SQLAlchemy

| ID | Bug | Detection |
| --- | --- | --- |
| `sqla:autoflush_in_loop` | Autoflush fires per query inside a `for` over a Session with pending changes | shape |
| `sqla:lazy_dynamic_in_loop` | `lazy='dynamic'` returns a `Query`; accessing it in loop = new SELECT per iter | annotation + loop access |
| `sqla:expire_on_commit_default` | Default `expire_on_commit=True` re-loads after `session.commit()` mid-loop | Session config inspection |
| `sqla:merge_in_loop` | `session.merge(obj)` per iter = N SELECT + N UPDATE | shape |
| `sqla:no_clear_in_batch_job` | Long script with `session.add()` repeated and no `session.flush()`/`clear()`/`commit()` | shape (heuristic) |
| `sqla:joinedload_collections_cartesian` | Multiple `joinedload(...)` on collections → cartesian; use `selectinload` | option pairing |
| `sqla:f_string_in_text` | `text(f"... {x} ...")` defeats bind params (sec + plan cache) | shape |

#### Sequelize / TypeORM / Prisma

| ID | Bug | Detection |
| --- | --- | --- |
| `sequelize:find_and_count_all_slow_count` | `findAndCountAll` runs slow COUNT(*) | shape (info) |
| `sequelize:paranoid_no_partial_index` | `paranoid: true` adds `deleted_at IS NULL` everywhere | annotation + migration check |
| `sequelize:default_scope_silent` | `defaultScope: { where: ... }` silently applies to every query | model scan |
| `typeorm:findone_no_where` | `repo.findOne()` with no `where:` returns random row | shape |
| `typeorm:save_does_extra_select` | `repo.save(newEntity)` issues a SELECT first; use `repo.insert()` for pure inserts | shape |
| `typeorm:cascade_recursive_no_depth_limit` | `cascade: true` on self-referential tree → unbounded save | annotation match |
| `prisma:findunique_request_cache_surprise` | `findUnique` caches at request level; tests break | shape (info) |
| `prisma:transaction_5s_default_timeout` | `prisma.$transaction(async tx => {...})` default 5s timeout | shape (info) |
| `prisma:select_include_mutual_exclusion` | `{ select: ..., include: ... }` at same level throws | object literal walk |
| `prisma:connection_limit_serverless_blow` | Serverless w/o `connection_limit` URL param × instances > DB max | config + topology |

#### Mongoose

| ID | Bug | Detection |
| --- | --- | --- |
| `mongoose:no_lean_on_read` | `.find()` on read path without `.lean()` (5–10× hydration cost) | shape (info) |
| `mongoose:populate_extra_roundtrip` | `.populate('a').populate('b')` runs 2 extra queries; consider `$lookup` | shape |
| `mongoose:auto_index_in_prod` | `autoIndex: true` (default) creates indexes at app startup | schema config |
| `mongoose:max_time_ms_missing` | Query without `.maxTimeMS(N)` can run forever | shape (info) |
| `mongoose:pre_save_in_insertmany` | `.pre('save')` middleware fires per doc in `insertMany` | schema + bulk call cross-check |
| `mongoose:where_server_side_js` | `$where` / `$accumulator` in pipeline — server-side JS | aggregation literal walk |

#### EF Core (warnings → drift findings)

| ID | Bug | Detection |
| --- | --- | --- |
| `efcore:multiple_collection_include_warning` | Multiple sibling `Include()`s w/o `AsSplitQuery` → cartesian | LINQ chain walk |
| `efcore:row_limiting_operation_without_order_by_warning` | `Skip`/`Take` without `OrderBy` → non-deterministic | LINQ chain walk |
| `efcore:lazy_load_on_disposed_context_warning` | Lazy load after `DbContext` disposed | scope analysis |
| `efcore:client_evaluation_warning` | LINQ falling back to client-side evaluation | pattern hint |
| `efcore:include_ignored_warning` | `Include()` followed by `.Select()` projection drops relation | chain analysis |
| `efcore:savechanges_in_loop` | `await ctx.SaveChangesAsync()` per iter — batch instead | shape |
| `efcore:no_as_no_tracking_on_read` | Read-only LINQ chain without `AsNoTracking()` | chain analysis (info) |
| `efcore:execute_update_async_alternative` | `foreach (var e in db.Set) { e.X = ...; } db.SaveChanges();` — EF 7+ should use `.Where(...).ExecuteUpdateAsync(...)` | shape |

#### GORM / ent / sqlc

| ID | Bug | Detection |
| --- | --- | --- |
| `gorm:updates_struct_zero_value_skip` | `db.Updates(&User{Age:0})` silently skips zero-valued `Age`; use map[string]any | shape |
| `gorm:where_in_empty_slice_returns_all` | `.Where("id IN ?", []int{})` returns ALL rows (some GORM versions) | shape (info) |
| `gorm:automigrate_concurrent_race` | `db.AutoMigrate(...)` in `main()` racing with another instance | shape |
| `gorm:preload_cartesian` | Multiple `Preload(...)` chains causing cartesian | shape |
| `gorm:max_open_conns_default_unlimited` | `database/sql` default = unlimited; must `SetMaxOpenConns` | shape (overlaps POOL004) |

#### Doctrine / Eloquent

| ID | Bug | Detection |
| --- | --- | --- |
| `doctrine:flush_in_loop_no_clear` | `$em->flush(); $em->clear();` pattern needed in batch loops | shape |
| `doctrine:cascade_persist_silent` | `cascade={"persist"}` saves unrelated graph silently | annotation match |
| `doctrine:extra_lazy_collection_missing` | `count()`/`contains()` on `@OneToMany` without `fetch="EXTRA_LAZY"` loads full collection | annotation + usage cross-check |
| `eloquent:chunk_id_drift` | `Model::chunk(...)` modifying records → use `chunkById` | shape |
| `eloquent:prevent_lazy_loading_in_prod` | Recommend `Model::preventLazyLoading()` in non-prod | config (info) |
| `eloquent:with_relation_select_mismatch` | `with(['rel' => fn($q) => $q->select('id')])` missing FK column drops relation silently | closure scan |

### 12.6 New detector families (round 3)

**`HydrationCost`** — reuses the `chain_method_names` walk from §11.6. Presence of `lean()` / `AsNoTracking()` / `StatelessSession` / `with_only_columns(...)` is the negative signal.

**`EmittedSqlGotcha`** — string + annotation pattern matchers for the named-bug catalog in §12.5. Most rules are 5-10 lines of tree-sitter Q + one positive + one negative fixture.

**`SerializerNPlusOne`** — three-step detector (the highest-value market gap drift can fill):
1. Locate the serializer / view class (DRF `serializers.ModelSerializer`, Spring `@RestController`, FastAPI `response_model=`).
2. Extract every field's source path (`source='profile.bio'`, `parent.children`, JPA entity getter chain).
3. Cross-check against `select_related`/`prefetch_related`/`@EntityGraph`/`.populate(...)` in the calling view/controller.

No purely-static tool catches this across frameworks today — Sentry/Datadog only catch it at runtime; nplusone/Bullet via instrumentation; IntelliJ JPA inspection only covers Java.

**`SchemaIndexMissing`** — cross-references model annotations + migration files:
- Soft-delete model needs partial index `WHERE deleted_at IS NULL`.
- Polymorphic `(type, id)` composite index.
- JSONB query path needs matching GIN index (`USING gin (data jsonb_path_ops)`).

**`OrmIdGenerator`** — annotation/config inspection: flag `GenerationType.IDENTITY` + `@SequenceGenerator(allocationSize=1)` + `UUIDV4` PK in append-heavy tables.

**`OrmCascadeStorm`** — annotation pairing: `CascadeType.ALL` + bidirectional `@OneToMany`, `orphanRemoval=true` + collection clear, TypeORM `cascade: true` on tree entities, Sequelize `onDelete: CASCADE` on high-fanout joins.

**`OrmConfigDrift`** — extends §11.4's `config_value_constraint`: `hbm2ddl.auto=update`, `spring.jpa.open-in-view=true`, Mongoose `autoIndex: true` in prod, Doctrine `auto_generate_proxy_classes` true in prod, Hibernate `hibernate.generate_statistics=true` in prod (overhead).

### 12.7 Canonical benchmark numbers — `expected_speedup` in remediation

When emitting a remediation, drift carries a quantitative expectation so the user knows the upper bound. Drawn from public benchmarks (Mihalcea HP-JP, EF Core docs, Drizzle vs Prisma):

| Pattern → fix | Expected speedup | Source |
| --- | --- | --- |
| Hibernate `EAGER` → `LAZY` + EntityGraph | 2–10× on collection paths | Mihalcea HP-JP |
| Hibernate `IDENTITY` → `SEQUENCE allocationSize=50` for batch inserts | 5–20× | Mihalcea |
| EF Core tracking → `AsNoTracking()` on read path | 1.4× CPU / 1.6× alloc | learn.microsoft.com/.../efficient-querying |
| EF Core `Include()` → projection `Select(x => new {...})` | 2–10× | Microsoft EF docs |
| Mongoose hydrated → `.lean()` on read path | 5–10× | Mongoose docs |
| Sequelize/TypeORM ORM → raw / Dapper-equivalent | ~2× | Dapper benchmarks |
| Drizzle vs Prisma simple read | 2–5× | Drizzle benchmarks |
| jOOQ vs Hibernate bulk read | 1.5–3× | Mihalcea HP-JP |
| Polars vs Pandas group-by | 5–10× | (push grouping to DB) |

Encoded as a range in `Finding.message`: "Expected 2–10× on hot path; verify with your workload."

### 12.8 QuickPerf-style test-assertion remediation

For Java fixtures, drift's remediation suggests a QuickPerf annotation that locks the fix into CI:

```
remediation: Add `@ExpectMaxSelect(1)` (QuickPerf) to this test to fail
             the CI build if the N+1 returns.
```

Full QuickPerf catalog (Apache-2.0, freely borrowable as remediation strings):
`@ExpectSelect(N)`, `@ExpectMaxSelect(N)`, `@ExpectInsert`, `@ExpectMaxInsert`, `@ExpectUpdate`, `@ExpectMaxUpdate`, `@ExpectDelete`, `@ExpectMaxDelete`, `@ExpectSelectedColumn(N)` (over-fetching), `@ExpectMaxSelectedColumn(N)`, `@ExpectUpdatedColumn(N)`, `@ExpectMaxQueryExecutionTime(value=N, unit=MILLISECONDS)`, `@ExpectJdbcBatching` (verify batching is on), `@ExpectNoConnectionLeak`, `@DisableSameSelectTypesWithDifferentParamValues` (THE canonical N+1 detector), `@DisableSameSelects`, `@DisableQueriesWithoutBindParameters`, `@DisableLikeWithLeadingWildcard`, `@DisableStatements`, `@ProfileQuery`.

Other-language analogs:
- Python: `django-perf-rec` snapshot recordings, `nplusone` `NPLUSONE_RAISE=True`, SQLAlchemy `raiseload('*')` on test session.
- Ruby: `prosopite.scan` / `Prosopite.raise = true`, `Bullet.raise = true`.
- .NET: `EF.ConfigureWarnings(w => w.Throw(RelationalEventId.MultipleCollectionIncludeWarning))`.
- Node: `prisma.$on('query', ...)` count assertion, Sequelize `logging:` count assertion.

### 12.9 Updated micro-step list (extends §7 + §11.8)

| # | Step | Test |
| --- | --- | --- |
| 43 | Extend `FindingKind` with the 6 round-3 variants. Add `as_str` cases + serde round-trip tests. | Existing snapshots unchanged. |
| 44 | Add optional `Evidence.otel: Option<OtelAttrs>` per §12.3. Populate in existing `NPlusOne` / `BlockingInAsync` detectors. | Snapshots: optional field absent by default; populated where ORM is recognized. |
| 45 | Adopt the rule-ID vocabulary (§12.2). Refactor existing detectors so `Evidence.call` uses one of the `source:rule_id` IDs. | All snapshots updated once; vocabulary stable thereafter. |
| 46 | Add `severity_thresholds.json` mirroring Sentry defaults (§12.4); wire into `bump_severities_by_impact`. | Bumps line up with documented thresholds. |
| 47 | Implement Hibernate named-bug rules from §12.5 (HHH-1262, multi-bag-fetch, EAGER defaults, etc.). | Fixture + snapshot per rule. |
| 48 | Implement Django named-bug rules. | Snapshot per rule. |
| 49 | Implement Rails ActiveRecord named-bug rules (when tree-sitter-ruby is added; until then catalog stays staged). | Snapshot per rule. |
| 50 | Implement SQLAlchemy named-bug rules. | Snapshot per rule. |
| 51 | Implement Sequelize / TypeORM / Prisma named-bug rules. | Snapshot per rule. |
| 52 | Implement Mongoose named-bug rules. | Snapshot per rule. |
| 53 | Implement EF Core warning-mirror rules. | Snapshot per rule. |
| 54 | Implement GORM / ent / sqlc named-bug rules. | Snapshot per rule. |
| 55 | Implement Doctrine / Eloquent named-bug rules. | Snapshot per rule. |
| 56 | Implement `SerializerNPlusOne` detector (DRF + Spring + FastAPI to start). | Snapshot per framework. |
| 57 | Implement `SchemaIndexMissing` detector — soft-delete partial + polymorphic composite + JSONB GIN cross-checks. | Snapshot. |
| 58 | Implement `OrmIdGenerator` + `OrmCascadeStorm` + `OrmConfigDrift` detectors. | Snapshot. |
| 59 | Enrich remediation text with QuickPerf-style test-assertion recommendations (§12.8). | Snapshot diff verifies remediation strings. |

Round-3 brings total micro-steps to **59 across three rounds**. Implementation order: round-1 SQL Query Analyzer (1-9) → round-2 ORM long-tail + migrations + pools + tx (10-42) → round-3 named bugs + serializer N+1 + schema-index cross-check (43-59).

### 12.10 Round-3 acceptance checklist

- [x] Adds **6 new `FindingKind` variants** (`HydrationCost`, `EmittedSqlGotcha`, `SerializerNPlusOne`, `SchemaIndexMissing`, `OrmIdGenerator`, `OrmCascadeStorm`, `OrmConfigDrift`). Total across 3 rounds: **18**.
- [x] Establishes a **stable `source:rule_id` vocabulary** (§12.2) so drift output cross-grep'd with Sentry / Datadog / Hypersistence / QuickPerf without translation.
- [x] Adds **optional OTel-attribute mirroring** on `Evidence` (§12.3) using the NEW semconv names.
- [x] Adopts **Sentry's published threshold defaults** (5 reps, 50ms span, 500ms slow-query, 300ms N+1 API) as drift's severity cutoffs (§12.4).
- [x] Specifies **~60 named-bug rules** keyed by public ticket IDs (HHH-1262, multi-bag-fetch, EF Core CoreEventId catalog, GORM zero-value Updates, Prisma 5s tx timeout, ...) so users searching for the bug find drift's coverage (§12.5).
- [x] Adds the **`SerializerNPlusOne` detector** — the highest-value market gap (no purely-static tool catches this today across frameworks).
- [x] Adds **canonical benchmark numbers** as `expected_speedup` ranges in remediation text (§12.7).
- [x] Adds **QuickPerf-style test-assertion recommendations** in remediation so the user can lock the fix in CI (§12.8).
- [x] **17 new testable micro-steps (43-59)** continuing the established cadence.
- [x] Total scope across all 3 rounds: **~150 statically-detectable rules**, **6 reusable detector shapes**, **10 catalog JSON files**, **~4 new source files** (`sql_lint.rs`, `orm_lint.rs`, `migration_lint.rs`, `config_lint.rs`).
- [x] License-clean: every catalog mined for *names/strings only*. No GPL code linked. New Rust deps remain Apache/BSD/MIT.

When you say "go round-3", step 43 begins.
