I have ample research now. Let me compile the deliverable as the final response.

# Drift Static Profiler — Deep Research: Static ORM→SQL & Live-DB IO-Perf Discovery

Two related questions about how `drift-static-profiler` can move from "this is a DB call in a loop" to "this is the exact SQL it emits, here is its plan, here is the rewrite." Citations are inline; URL list at the bottom.

---

## Question 1 — Static ORM → SQL translation, per ORM

The general taxonomy across ORMs is:

1. **Reified IR** — the ORM builds an AST/IR in memory that is *already* the SQL graph (SQLAlchemy Core, Drizzle, sqlc). Drift can evaluate or render that IR without a DB.
2. **Opaque builder chain** — the ORM defers SQL synthesis to a runtime visitor that needs schema + dialect + bound parameters (Django QuerySet, ActiveRecord, Eloquent, Sequelize). Drift can only *symbolically* infer the SQL.
3. **String-with-DSL** — the ORM accepts a literal in a sublanguage (JPQL/HQL, DQL, raw SQL, Drizzle template `sql\`\``). Drift can capture these via the existing SQL-sink machinery in §3 of the plan and parse them with `sqlparser-rs` / `pg_query`.
4. **Schema-first** — the SQL contract lives in a separate file (Prisma `.prisma`, sqlc `.sql`, Diesel `schema.rs`). Drift can read that as a side-channel ground truth.

Per-ORM detail follows.

### 1.1 Django ORM (Python)

**Status: NO mature static SQL translator exists.** All known tools are runtime.

- `QuerySet.query` attribute — runtime. Renders SQL by walking the compiler against the connection's vendor backend. `str(qs.query)` works without execution but the QuerySet object must still be alive in memory.
- `django-debug-toolbar` — runtime, captures via DB cursor wrapper.
- `django-extensions shell_plus --print-sql` — runtime; truncates at 1000 chars by default (`SHELL_PLUS_PRINT_SQL_TRUNCATE`).
- `django-debug-toolbar`'s `debugsqlshell` — runtime.
- `django-querycount`, `nplusone`, `django-sql-sniffer` — all runtime.
- `django-lint` (Chris Lamb) — code-smell-level static, not SQL-aware.
- `pylint-django` / `mypy-django` (django-stubs) — type-aware, but do not synthesize SQL.

**Why it's hard:** Django's compiler is at `django.db.models.sql.compiler.SQLCompiler.as_sql()`, and it pulls dialect, table aliases, and `Meta.db_table` from the live `apps` registry. To do this statically, drift would need to:
1. Parse `models.py` and build a model registry (field types, FKs, `Meta.db_table`).
2. Symbolically execute the QuerySet chain (`.filter()`, `.values()`, `.select_related()`, `.annotate()`) into a `Query` IR.
3. Hand it to a dialect compiler (Postgres/MySQL/SQLite) that drift ships.

This is roughly what `django-stubs`' plugin does *for typing* but stops short of SQL emission. The closest experiment is `django-sql-explorer` (UI-only) and the abandoned `django-explorer-sql`.

**Drift gap to fill:** This is a clear opportunity. A `model_registry.py` pass (resolve Field types, FK targets, table names) + a closed-world QuerySet evaluator that emits parameterized SQL for the top-N built-in QuerySet operations would deliver Django→SQL statically. Even a 60%-coverage version (filter/exclude/annotate/values/select_related/order_by/limit) would already let drift run `EXPLAIN` against a dev DB.

### 1.2 SQLAlchemy (Python)

**Status: PARTIALLY static — the easiest ORM to translate.**

SQLAlchemy Core builds a reified `ClauseElement` tree. To get SQL without a connection:

```python
from sqlalchemy.dialects import postgresql
str(stmt.compile(
    dialect=postgresql.dialect(),
    compile_kwargs={"literal_binds": True}))
```

This is documented in the [SQL Expressions FAQ](https://docs.sqlalchemy.org/en/21/faq/sqlexpressions.html). The catch is `literal_binds` only supports primitive types (int/str/bool/date), so subqueries with binary blobs or JSONB will fall back to `?` placeholders.

**Static catch:** This still requires *running* the Python module to construct the ClauseElement. A pure-AST drift pass would need to:
1. Recognize `select()`, `update()`, `Table(...)`, `Column(...)`, `relationship(...)` from `import` graph.
2. Build a parallel `ClauseElement` purely from AST shapes.
3. Pass to `sqlalchemy.dialects.<x>.dialect().statement_compiler` — but the compiler reaches into Python objects.

A pragmatic hybrid (used by tools like SQLModel migrations): drift could *optionally* import the user's module in a sandboxed Python subprocess, walk the metadata, and `.compile()` each statement. Not pure static, but cheap.

Existing tools:
- `sqlcommenter` (Google) — runtime, augments SQL with comments. Not a static analyzer.
- `phpstan-dba` analog does not exist for Python.
- `sqlalchemy-stubs` / `sqlalchemy.ext.mypy_plugin` — type-aware only.

**Drift gap:** ship the "subprocess introspection" mode for SQLAlchemy as the first real ORM-translator. License (MIT) is clean.

### 1.3 Hibernate / JPA (Java)

**Status: STRING-CAPTURE works trivially; Criteria API requires runtime.**

JPQL/HQL strings in `@Query` or `entityManager.createQuery("...")` are literals — drift's existing string-sink pass already gets them. They're not SQL, they're JPQL; drift would need either to ship a JPQL parser or translate JPQL→SQL via a runtime hook.

For Criteria API, the canonical tool is **Hypersistence Utils' `SQLExtractor`** (since 2.9.11). Usage:
```java
String sql = SQLExtractor.from(criteriaQuery);
```
This is runtime — it walks Hibernate's internal `SqmQuery` tree. License is Apache-2.0 (Vlad Mihalcea's project).

Static analyzers that exist in the JPA space:
- **JPA Buddy** (IntelliJ plugin, commercial) — inspections for `FetchType.EAGER`, missing `@EntityGraph`, etc. Not free / not embeddable.
- **JArchitect / Sonargraph** — architecture lint, not SQL.
- **SonarQube `java:S2479`** style rules touch JPA naming but not SQL shape.
- **InspectionGadgets** (IntelliJ open-core) has `JpaModelErrorsInspection` and `JpaQlInspection` — checks JPQL syntax. Apache-2.0.

**Drift gap:** Hibernate developers benefit most from `FetchType.EAGER` detection (clear lint) + `@OneToMany` without `@BatchSize` (perf) + missing `@EntityGraph` on `findAll` calls in service methods. These are AST patterns. SQL synthesis for Criteria is impractical statically.

### 1.4 TypeORM (TypeScript)

**Status: Pure runtime.** Query builder (`createQueryBuilder().where().leftJoinAndSelect()`) compiles via `QueryBuilder.getSql()` which still requires the live connection (for dialect + metadata).

No static analyzer worth citing. TypeORM is the consensus loser of the 2026 ORM comparisons.

**Drift gap:** AST pattern for `findAll(...)` inside a loop is straightforward and high-value. Translating `createQueryBuilder` chains statically is low-ROI (the ecosystem is moving to Drizzle/Prisma).

### 1.5 Prisma (TypeScript, schema-first)

**Status: SCHEMA is statically parseable; queries are not.**

The `.prisma` file is the contract. Two parsers exist:
- `psl` (Rust, Prisma official): `prisma::prisma_engines::psl` — usable directly from drift if drift adds it as an optional Cargo dep. License: Apache-2.0. [Crate](https://prisma.github.io/prisma-engines/doc/psl/).
- `loancrate/prisma-schema-parser` (TS).
- `MrLeebo/prisma-ast` (TS).

But the runtime client calls (`prisma.user.findMany({ where, include: { posts: true } })`) are compiled by the Rust query engine — and per the November 2025 Prisma 7 release, the engine is now pure TS. Either way, statically rendering SQL needs to re-implement the Prisma query compiler.

`Prisma Optimize` (commercial) does this on captured traces, not statically. There is no open-source static Prisma→SQL translator.

**Drift gap:** Parse `.prisma`, build a model registry, then AST-detect anti-patterns at the *call site* (e.g., `findMany` inside a `for`, `findUnique` without `where`, missing `select`, deep `include` nesting). Don't try to synthesize SQL.

### 1.6 Drizzle ORM (TypeScript)

**Status: BEST of the TypeScript ORMs for static translation.**

Drizzle exposes `.toSQL()` synchronously on any query builder. The returned `{sql, params}` is the literal SQL string + bound params, *with no connection* — Drizzle's dialect is purely client-side. See [docs/goodies](https://orm.drizzle.team/docs/goodies).

```ts
const q = db.select().from(users).where(eq(users.id, 1));
q.toSQL() // -> { sql: 'select ... from "users" where "users"."id" = $1', params: [1] }
```

Static catch: `toSQL()` needs to run, but since Drizzle is pure-TS with no runtime engine, drift can spin up a Node subprocess that imports the user's schema module and invokes `toSQL()` on each query expression it finds via AST. Very high yield.

Limitation: type-level inference of *result shape* is open (drizzle issue [#2597](https://github.com/drizzle-team/drizzle-orm/issues/2597)) but doesn't affect SQL synthesis.

**Drift gap:** A `drizzle-static-translate` module that AST-walks `.ts`, finds Drizzle expressions, evaluates `.toSQL()` in a worker, and feeds the literal SQL into drift's `sqlparser-rs` linting + EXPLAIN pipeline. This is the *single highest-leverage* ORM target.

### 1.7 Sequelize (TypeScript / Node)

**Status: Runtime-only.** `sequelize.query(sql)` is captured; the model API (`Model.findAll({include, where})`) needs the dialect/connection.

Sequelize logs the generated SQL via `logging: console.log`. To get SQL statically you'd reimplement `QueryGenerator`. The community has not produced a tool.

**Drift gap:** AST patterns for `include: [...]` depth, `where: {[Op.or]: [...]}` size, `findAll` in loops. Treat like TypeORM.

### 1.8 GORM (Go)

**Status: Mostly runtime; `db.Debug()` prints SQL.**

- No static N+1 detector for GORM in the major linters (staticcheck, golangci-lint, gopls). [DoltHub Blog](https://www.dolthub.com/blog/2024-07-24-static-analysis/) confirms gap.
- `gox` (mentasystems) — strict Go static analyzer with no specific GORM rules.
- Custom `go/analysis` analyzers are the recommended path; [countingup](https://engineering.countingup.com/custom-go-vet/) documents the approach.

**Drift gap:** GORM is a great target for AST-based anti-pattern detection because the chain is fluent and the receiver types are inferrable. Detect: `.Find(&users)` inside a `for`, missing `.Preload`, missing `.Select` on wide tables, `.Where` predicate that re-fetches same row in a loop. Translating to SQL statically is feasible (GORM compiles in Go and uses Go reflection to map struct→table, so drift could mirror that with struct-tag parsing).

### 1.9 ActiveRecord (Ruby/Rails)

**Status: Static AST patterns exist; SQL synthesis is runtime.**

- **Bullet** — runtime callback that watches association loads. Suggests `includes`/`eager_load`. Both false positives and false negatives in dynamic codebases.
- **Prosopite** (charkost/prosopite, MIT) — runtime ActiveSupport instrumentation, fingerprints repeat queries with same callstack. Catches N+1 *after creation* and outside association chains. Zero false-positive claim. Does not suggest fixes (Bullet does).
- **`rails-bestpractices` / `rubocop-rails`** — static, AST level, but no SQL synthesis.

ActiveRecord can render SQL statically via `Model.all.to_sql` *in a Rails console* — still loads schema.rb.

**Drift gap:** AST pass on `.rb` files for `Model.where(...).each`, `User.find_by(...)` inside `.map`, and chains that miss `includes`. Combine with `db/schema.rb` parse to know table sizes.

### 1.10 Doctrine / Eloquent (PHP)

**Status: BEST PHP coverage of any ecosystem, thanks to `phpstan-dba`.**

- **`phpstan-dba`** (staabm) — Apache-2.0, PHPStan extension. Per their docs, it does **SQL static analysis and type inference** for Doctrine DBAL, PDO, and mysqli on MySQL/MariaDB and PostgreSQL. Catches placeholder mismatches, syntax errors, query plan analysis. Notably: "Queries are analyzed statically and do not require a running database server, using the Doctrine DQL parser and entities metadata."
- **`phpstan/phpstan-doctrine`** — analyzes entities, repositories, query builders, and DQL. Reflection-based.
- **`larastan/larastan`** — Eloquent inference (Laravel).
- **`rector` Symfony/Doctrine rules** — refactoring, not lint.

**Drift gap:** PHP is the one ecosystem where the static-translation problem is *largely solved* by phpstan-dba. Drift's value-add in PHP is to (a) re-emit phpstan-dba-style findings in drift's unified Finding shape, or (b) skip PHP entirely.

### 1.11 EF Core (C#) — for completeness

`IQueryable` builds an expression tree that EF Core's `RelationalQueryTranslationPostprocessor` lowers to SQL. The `.ToQueryString()` extension method (EF Core 5+) renders SQL *without* a DB roundtrip, but does need EF Core's `DbContext` instantiated. The closest static analyzer is **EFCore.Analyzers** (Roslyn) — checks for client-evaluation, missing `.AsNoTracking()`. License: Apache-2.0.

### 1.12 Summary table for Question 1

| ORM | Reified IR available? | Static SQL feasibility | Existing tool | License | Drift opportunity |
|---|---|---|---|---|---|
| Django | No (needs apps registry) | Hard — subprocess only | `qs.query` (runtime), nplusone (runtime) | MIT/BSD | Reimplement closed-world compiler |
| SQLAlchemy | Yes (ClauseElement) | Medium — subprocess | `stmt.compile(dialect=...)` | MIT | Optional subprocess introspection |
| Hibernate JPQL | Literal string | Easy | drift already captures | n/a | Add JPQL parser |
| Hibernate Criteria | Runtime | Hard | `SQLExtractor` (Hypersistence Utils) | Apache-2.0 | Skip; do EAGER lint |
| TypeORM | Runtime | Hard | none | n/a | AST anti-patterns |
| Prisma | Schema-first, runtime queries | Medium | `psl` parser | Apache-2.0 | Parse schema + AST anti-patterns |
| **Drizzle** | **Yes (.toSQL())** | **Easy** | `.toSQL()` | Apache-2.0 | **Subprocess `toSQL()` is highest-leverage TS target** |
| Sequelize | Runtime | Hard | none | n/a | AST anti-patterns |
| GORM | Runtime | Medium (Go reflection mirrors) | `db.Debug()` | n/a | Custom go/analysis pass |
| ActiveRecord | Runtime (`.to_sql`) | Hard | Bullet (runtime), Prosopite (runtime) | MIT | AST + schema.rb |
| Doctrine | Runtime + reflection | Solved | **phpstan-dba** | Apache-2.0 | Wrap or skip |
| Eloquent | Runtime | Partial | Larastan | MIT | Wrap or skip |
| EF Core | Yes (Expression tree) | Medium | `.ToQueryString()` (runtime) | MIT | Roslyn analyzers exist |

The clear conclusion: **Drizzle (`toSQL()`) and SQLAlchemy (`compile(dialect)`) are the only two ORMs where high-fidelity static SQL translation is cheap to ship**. Django, ActiveRecord, GORM, Sequelize are AST-anti-pattern targets only. PHP is solved by phpstan-dba. JPA Criteria is impractical; JPQL strings are captured trivially.

---

## Question 2 — Live-DB IO-perf discovery: signatures, thresholds, rewriters

### 2.1 Empirical signatures of an "over-complicated" query

| Signature | Threshold (community-cited) | Source |
|---|---|---|
| Total plan node count | > 30 nodes | pganalyze plan-comparison heuristics |
| Join nodes | ≥ 5 joins per statement | sqlcheck "many table joins" rule |
| Subquery nesting depth | ≥ 3 levels | sqlcheck "deep nesting" |
| Same subquery repeated | ≥ 2 textually-identical subselects | sqlglot `eliminate_subqueries` (rewritable to CTE) |
| `SELECT *` from wide table | > 10 cols, only N used | sqlfluff `AM04` (ambiguous columns), sqlcheck "select star" |
| Number of columns in projection | > 20 | sqlcheck "too many columns" |
| CTE that is not referenced > 1× | always inlineable (PG 12+) | sqlglot `eliminate_ctes` |
| `Materialize` node when inner is cheap | depesz exclusive-time > 10% | depesz coloring |

### 2.2 Empirical signatures of a "bad" IO-heavy query

Distilled from pgMustard, pganalyze Query Advisor, depesz, and Datadog DBM:

| Signature | Postgres EXPLAIN ANALYZE check | Severity threshold |
|---|---|---|
| Sequential scan on large table | `Seq Scan` and `Rows Removed by Filter > 1000 * Rows Returned` | pganalyze "Slow Scan" |
| Row misestimate | `actual_rows / planned_rows > 10` or inverse | pgMustard flags ≥ 10×; depesz colors 10×/100×/1000× |
| Sort spilling | `Sort Method: external merge` | pganalyze "Disk Sort" |
| Hash join with multiple batches | `Batches > 1` | pgMustard mentions; bump `work_mem` |
| Lossy bitmap heap scan | `Heap Blocks: exact=N lossy=M (M > 0)` | depesz/pgMustard |
| Index Scan with high Heap Fetches | `Heap Fetches > 0.1 * Rows Returned` | pgMustard Heap-Fetches tip; needs `VACUUM` for index-only |
| Cold cache (high shared read) | `Buffers: shared hit=A read=B` with `B/(A+B) > 10%` | pgMustard Buffers-Shared-Read tip; cache hit ratio < 90% per Redgate |
| JIT overhead dominates | `JIT Time > 30% of total Execution Time` | community/pganalyze |
| Nested Loop disaster | outer rows × no inner index | pganalyze Query Advisor primary signal |

### 2.3 Cost/latency thresholds the community uses

- **pgMustard** scores each tip 0–5 stars based on time-saving potential; default highlight cutoff is **0.95 stars** (raised from 1.0). Row-estimate tips fire at **≥ 10× misestimate**. They specifically exclude `BitmapAnd`/`BitmapOr`/`ModifyTable` row-tips to cut false positives.
- **depesz** exclusive-time colors:
  - White ≤ 10 % of total time
  - Yellow 10–50 %
  - Brown 50–90 %
  - Red > 90 %
  Row-estimate colors at 10×/100×/1000×.
- **pganalyze Query Advisor** categories: Disk Sort, Mis-Estimate, Slow Scan, Stale Stats, Nested Loop, Wrong Index for Sort.
- **Sentry**:
  - Slow query detector fires when the same query exceeds **500 ms** and is observed **100 times** consistently.
  - N+1 detector requires sequential non-overlapping DB spans with similar descriptions; thresholds are configurable per project.
  - Consecutive DB queries detector counts back-to-back DB spans inside a transaction.
- **Datadog DBM**: collects top **200** normalized queries per **10 s** interval; sample monitors suggest `Duration:>30s` as long-running. No documented "rows examined" default — DBM is sampling-based, not threshold-based.
- **MySQL `sys.statements_with_full_table_scans`** ranks digests by percentage of time *no_index_used*; threshold is implicit (any non-zero is flagged).

### 2.4 Rewriting heuristics (statically applicable to SQL text)

| Antipattern | Rewrite | Why | Tool that does this automatically |
|---|---|---|---|
| `IN (subquery)` | `EXISTS (correlated subquery)` | Postgres planner can pick **Semi Join** instead of SubPlan; early-terminates on first match; no DISTINCT needed (Cybertec, Percona). | sqlglot's `unnest_subqueries` |
| `NOT IN (subquery)` | `NOT EXISTS (correlated subquery)` | NULL-safe: `NOT IN` returns UNKNOWN if any inner row is NULL. PG cannot auto-rewrite because **semantics differ**. (Cybertec) | sqlglot `unnest_subqueries` partially |
| `OR a=1 OR a=2 OR a=3` | `a IN (1,2,3)` | Planner can convert to ScalarArrayOp / hash-lookup. | sqlglot `simplify` |
| Implicit join `FROM a, b WHERE a.k=b.k` | `a JOIN b ON a.k=b.k` | Standardizes plan; sqlfluff `AM07` flags. | sqlfluff (lint), sqlglot |
| Correlated subquery on every row | LATERAL / lateral derived table | One scan instead of N×inner. | sqlglot, Calcite |
| Duplicate subqueries in one statement | hoist to CTE | Deduplication; PG 12+ inlines correctly. | sqlglot `eliminate_subqueries` |
| Redundant single-use CTE | inline as derived table | Pre-PG-12 materialization barrier. | sqlglot `eliminate_ctes`, `merge_subqueries` |
| `LIKE 'prefix%'` | functional / btree index works; OK | n/a | sqlcheck flags `LIKE '%x'` |
| `LIKE '%infix%'` | trigram (`pg_trgm`), full-text, or reverse-prefix index | btree useless. | suggest, not auto-rewrite |
| `ORDER BY RANDOM() LIMIT N` | `TABLESAMPLE BERNOULLI (p)` / `tsm_system_rows` / pre-pick IDs | Up to **20,000–40,000× faster** vs `ORDER BY RANDOM()` per [Render](https://render.com/blog/postgresql-random-samples-big-tables), [EnterpriseDB](https://www.enterprisedb.com/blog/tablesample-and-other-methods-getting-random-tuples). Caveat: TABLESAMPLE doesn't combine with `WHERE`. | manual / suggest |
| `WHERE func(col) = x` | functional index `CREATE INDEX ON t(func(col))` | sargability. | HypoPG + suggestion |
| `WHERE col::text = '1'` | drop cast or store as text | breaks index. | sqlcheck |
| `SELECT count(*)` on large table | `pg_class.reltuples` approximate / materialized counter | seq-scan avoidance. | docs only |

### 2.5 Open-source query rewriters / linters

- **sqlglot** (MIT, Python) — full optimizer pipeline: `qualify → pushdown_projections → normalize → unnest_subqueries → pushdown_predicates → optimize_joins → eliminate_subqueries → merge_subqueries → eliminate_joins → eliminate_ctes → quote_identifiers → annotate_types → canonicalize → simplify`. Drift could shell out to Python for now, or wait for a Rust port. Source: [sqlglot/optimizer/optimizer.py](https://github.com/tobymao/sqlglot/blob/main/sqlglot/optimizer/optimizer.py).
- **Apache Calcite** (Apache-2.0, Java) — ~100 transformation rules including `FilterPushDown`, `ProjectPushDown`, `JoinCommute`, `SubQueryRemoveRule`, `AggregateExpandDistinctAggregatesRule`, `MaterializedViewRule`. Heavyweight; not practical from Rust directly. Source: [calcite.apache.org](https://calcite.apache.org/javadocAggregate/org/apache/calcite/rel/rules/package-summary.html).
- **DataFusion `datafusion-optimizer`** (Apache-2.0, Rust) — `OptimizerRule`s for predicate pushdown, common-subexpr elimination, projection pushdown, simplify-expressions, push-down-filter, eliminate-cross-join, eliminate-outer-join. *Usable directly from drift.* Source: [docs.rs/datafusion-optimizer](https://docs.rs/datafusion-optimizer).
- **`sqlparser-rs`** (Apache-2.0, Rust, `apache/datafusion-sqlparser-rs`) — has `Visitor` trait (feature-flagged) for AST walking + AST→SQL regeneration. Drift already lists this as a dep in §1.1 of QUERY_ORM_ANALYZER_PLAN.md.
- **`pg_query.rs`** (BSD-3) — real Postgres parser + `normalize()` + `fingerprint()`. Already a candidate dep.
- **sqlfluff** (MIT, Python) — `ST01` (do not use `SELECT *`), `ST02` (CASE returning booleans → `COALESCE`), `ST03` (unused CTE), `ST05` (subqueries → CTEs), `AM04` (ambiguous columns), `AM07` (implicit cross join). Mineable for rule ideas.
- **sqlcheck** (Apache-2.0, C++, jarulraj) — 21 anti-patterns (selection star, nullable columns, indexed-search inefficiency, generic primary key, etc.). The full list is enumerated in their VLDB 2020 paper "SQLCheck: Automated Detection and Diagnosis of SQL Anti-Patterns" (DOI [10.1145/3318464.3389754](https://dl.acm.org/doi/abs/10.1145/3318464.3389754)).
- **squawk** (GPL-3, Rust) — migration safety. Drift cannot link this (GPL contagion) but can mine rule names freely.
- **pg_hint_plan** (BSD, PG extension) — supplies the *syntax* drift remediation can recommend (`/*+ IndexScan(t idx) */`).

### 2.6 Tools for running EXPLAIN safely on dev DBs

- **pgMustard** (commercial; free CLI) — submits a plan, returns 0–5 star tips. Static rules + threshold.
- **explain.dalibo.com** (free, web, AGPL on PEV2) — visual.
- **explain.depesz.com** (free, web) — color heatmap, exclusive vs inclusive time.
- **pganalyze Query Advisor** (commercial) — continuous EXPLAIN ingestion.
- **pgBadger** (PostgreSQL Licence) — log post-processor, top-N normalized.
- **HypoPG** (PostgreSQL Licence) — Postgres extension: `hypopg_create_index('CREATE INDEX ON t(c)')` returns a virtual index visible to the planner. Pair with EXPLAIN to test "would this index help?" without writing it.
- **Dexter** (MIT, Andrew Kane) — reads slow-query log, uses HypoPG to test candidate B-tree indexes, recommends winners.
- **pg_qualstats** (PostgreSQL Licence) — per-predicate selectivity stats; `pg_qualstats_example_query()` gives one example per qual digest. Combined with HypoPG = the Percona automatic-index-recommendation recipe.
- **pg_stat_statements** — query digests + cumulative timings; baseline for "what's the top-N expensive query?"
- **auto_explain** — runtime hook, captures plans of slow queries.
- **MySQL Workbench Visual Explain** — UI.
- **MySQL `sys.statements_with_full_table_scans`** — view that flags digests doing seq scans.
- **Percona pt-query-digest** (GPL) — log post-processor.

### 2.7 Rust crates relevant to drift

| Crate | License | Use in drift |
|---|---|---|
| `sqlparser` (Apache datafusion-sqlparser-rs) | Apache-2.0 | Multi-dialect parse + Visitor; AST→SQL regen. Already planned. |
| `pg_query` (pganalyze) | BSD-3 | Real Postgres parse tree, normalize, fingerprint. Already planned. |
| `datafusion-sql` + `datafusion-optimizer` | Apache-2.0 | Logical-plan optimizer with ~30 rewrite rules (predicate pushdown, eliminate cross join, simplify expressions). **Embeddable.** |
| `datafusion-expr` | Apache-2.0 | LogicalPlan enum for matching plan shapes. |
| `tokio-postgres` / `sqlx` | MIT/Apache-2.0 | EXPLAIN execution against dev DBs. |

There is no Rust port of Calcite's rules. The realistic path: drift uses `sqlparser-rs` as the AST, ports a small subset of sqlglot's rewrites manually (`IN`→`EXISTS`, OR-fold, subquery→CTE), and uses DataFusion's optimizer for the more general rewrites when feasible.

### 2.8 Static-detection vs runtime-EXPLAIN translatability

| Rule | Static (text-only) | Requires EXPLAIN | Requires schema |
|---|---|---|---|
| `SELECT *` | yes | no | no |
| `NOT IN` with nullable column | partial | no | yes (need to know nullability) |
| `IN (subquery)` candidate | yes | no | no |
| Seq scan on large table | no | yes | no |
| Row misestimate ≥ 10× | no | yes (`EXPLAIN ANALYZE`) | no |
| Sort spilling to disk | no | yes (`EXPLAIN ANALYZE`) | no |
| Missing index | partial (WHERE col not in any index) | yes (HypoPG to validate) | yes |
| Functional predicate breaks index | yes | no | yes (index list) |
| `ORDER BY RANDOM()` | yes | no | no |
| `LIKE '%infix%'` | yes | no | no |
| `OR a=1 OR a=2 OR a=3` | yes | no | no |
| Duplicate subqueries | yes | no | no |
| Plan node count > 30 | no | yes | no |
| Heap Fetches needs vacuum | no | yes | no |

Roughly half the catalogue is pure-static. The other half needs the dev-DB EXPLAIN integration — and even there, HypoPG + Dexter give drift a way to recommend indexes *without writing them*.

---

## Recommended next steps for drift

Concrete, ranked by ROI:

1. **Drizzle subprocess `.toSQL()`** — single biggest win for TS. ~1 week.
2. **Static SQL-text linter** — port sqlcheck's 21 rules + sqlfluff `ST01/02/03/05` + the rewriter table 2.4 above to Rust via `sqlparser-rs` Visitor. ~2 weeks. All pure-static.
3. **Postgres EXPLAIN integration** — already in plan §5. Add HypoPG-backed "would this index help" mode. ~1 week given Postgres connection scaffolding.
4. **SQLAlchemy subprocess `compile(dialect)`** — second-biggest Python win. ~1 week.
5. **Django closed-world QuerySet evaluator** — biggest gap but biggest investment. ~4-8 weeks; only justified if Django users dominate drift's target audience.
6. **`phpstan-dba` interop** — read its JSON output, re-emit as drift Findings. ~3 days. Free PHP coverage.
7. **AST anti-pattern catalogs** for GORM, ActiveRecord, TypeORM, Sequelize. ~1 week each. Don't try to synthesize SQL; flag shape.

---

## Sources

- [Django QuerySet → SQL forum thread](https://forum.djangoproject.com/t/how-to-extract-usable-sql-from-a-queryset/4057)
- [django-extensions shell_plus --print-sql lesson](https://www.vintasoftware.com/lessons-learned/django-extensions-shell-plus-command-allows-you-to-print-sql-queries)
- [django-sql-sniffer](https://github.com/gruuya/django-sql-sniffer)
- [django-debug-toolbar commands](https://django-debug-toolbar.readthedocs.io/en/latest/commands.html)
- [django-lint](https://chris-lamb.co.uk/projects/django-lint)
- [SQLAlchemy SQL Expressions FAQ](https://docs.sqlalchemy.org/en/21/faq/sqlexpressions.html)
- [SQLAlchemy compile() discussion #10997](https://github.com/sqlalchemy/sqlalchemy/discussions/10997)
- [Generate SQL from SQLAlchemy objects (ZappyTalks)](https://zappytalks.medium.com/generate-the-sql-query-from-sqlalchemy-objects-c5100aca809f)
- [Hypersistence Utils SQLExtractor (Vlad Mihalcea)](https://vladmihalcea.com/get-sql-from-jpql-or-criteria/)
- [JPA Criteria API (Baeldung)](https://www.baeldung.com/hibernate-criteria-queries)
- [Hibernate eager vs lazy (Baeldung)](https://www.baeldung.com/hibernate-lazy-eager-loading)
- [Prisma psl crate](https://prisma.github.io/prisma-engines/doc/psl/)
- [loancrate/prisma-schema-parser](https://github.com/loancrate/prisma-schema-parser)
- [MrLeebo/prisma-ast](https://github.com/MrLeebo/prisma-ast)
- [Prisma Optimize](https://www.prisma.io/optimize)
- [Drizzle toSQL goodies](https://orm.drizzle.team/docs/goodies)
- [Drizzle issue #2597 (toSQL inference)](https://github.com/drizzle-team/drizzle-orm/issues/2597)
- [Prisma vs Drizzle vs TypeORM 2026 (Encore)](https://encore.dev/articles/prisma-vs-drizzle-vs-typeorm)
- [Sequelize logging tutorial](https://futurestud.io/tutorials/sequelize-show-or-log-generated-sql-query-statements)
- [GORM performance docs](https://gorm.io/docs/performance.html)
- [GORM N+1 article (Stackademic)](https://blog.stackademic.com/the-n-1-query-problem-in-gorm-how-to-avoid-silent-performance-killers-856e028d4b15)
- [Custom go-vet for GORM (countingup)](https://engineering.countingup.com/custom-go-vet/)
- [Staticcheck](https://staticcheck.dev/)
- [Bullet vs Prosopite (Factorial)](https://labs.factorialhr.com/posts/bullet-or-prosopite-for-nplus1)
- [Prosopite README](https://github.com/charkost/prosopite/blob/main/README.md)
- [phpstan-dba](https://github.com/staabm/phpstan-dba)
- [phpstan-doctrine](https://github.com/phpstan/phpstan-doctrine)
- [pgMustard scoring API](https://www.pgmustard.com/docs/scoring-api)
- [pgMustard changelog (thresholds)](https://www.pgmustard.com/changelog/)
- [pgMustard row count estimates article](https://medium.com/pgmustard/row-count-estimates-in-postgres-8540087a826e)
- [pgMustard Buffers Shared Read](https://www.pgmustard.com/docs/explain/buffers-shared-read)
- [pgMustard Buffers Shared Hit](https://www.pgmustard.com/docs/explain/buffers-shared-hit)
- [depesz help (color thresholds)](https://explain.depesz.com/help)
- [depesz fixed exclusive times 2021](https://www.depesz.com/2021/08/03/new-changes-on-explain-depesz-com-fixed-calculations-of-exclusive-times/)
- [pganalyze Query Advisor GA](https://pganalyze.com/blog/query-advisor-ga)
- [pganalyze EXPLAIN docs](https://pganalyze.com/docs/explain)
- [pganalyze nested loops + buffers](https://pganalyze.com/blog/5mins-explain-analyze-buffers-nested-loops)
- [Sentry slow DB queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/slow-db-queries/)
- [Sentry N+1 queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/n-one-queries/)
- [Sentry consecutive DB queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/consecutive-db-queries/)
- [Datadog DBM data collected](https://docs.datadoghq.com/database_monitoring/data_collected/)
- [Datadog DBM optimize PG](https://www.datadoghq.com/blog/optimize-postgresql-performance-with-datadog/)
- [MySQL sys.statements_with_full_table_scans](https://dev.mysql.com/doc/refman/8.4/en/sys-statements-with-full-table-scans.html)
- [sqlglot eliminate_subqueries](https://github.com/tobymao/sqlglot/blob/main/sqlglot/optimizer/eliminate_subqueries.py)
- [sqlglot merge_subqueries](https://sqlglot.com/sqlglot/optimizer/merge_subqueries.html)
- [sqlglot optimizer pipeline](https://sqlglot.com/sqlglot/optimizer/optimizer.html)
- [Apache Calcite rules javadoc](https://calcite.apache.org/javadocAggregate/org/apache/calcite/rel/rules/package-summary.html)
- [Apache Calcite materialized views](https://calcite.apache.org/docs/materialized_views.html)
- [DataFusion query optimizer](https://datafusion.apache.org/library-user-guide/query-optimizer.html)
- [datafusion-optimizer crate](https://lib.rs/crates/datafusion-optimizer)
- [datafusion-sql crate](https://crates.io/crates/datafusion-sql)
- [sqlparser-rs (datafusion fork)](https://github.com/apache/datafusion-sqlparser-rs)
- [sqlfluff rules reference](https://docs.sqlfluff.com/en/stable/reference/rules.html)
- [sqlcheck repo](https://github.com/jarulraj/sqlcheck)
- [sqlcheck VLDB 2020 paper](https://dl.acm.org/doi/10.14778/3476311.3476343)
- [Cybertec: Subqueries and performance in Postgres](https://www.cybertec-postgresql.com/en/subqueries-and-performance-in-postgresql/)
- [Yugabyte: correlated subqueries with semi-joins](https://www.yugabyte.com/blog/correlated-subqueries-with-semi-joins/)
- [Percona: IN vs EXISTS vs ANY](https://www.percona.com/blog/sql-optimizations-in-postgresql-in-vs-exists-vs-any-all-vs-join/)
- [Percona: pg_qualstats + hypopg](https://www.percona.com/blog/automatic-index-recommendations-in-postgresql-using-pg_qualstats-and-hypopg/)
- [ankane/dexter](https://github.com/ankane/dexter)
- [Introducing Dexter](https://ankane.org/introducing-dexter)
- [Headway: automating Postgres indexing](https://www.headway.io/blog/automating-postgres-indexing-with-dexter-and-hypopg)
- [Render: random samples from big tables](https://render.com/blog/postgresql-random-samples-big-tables)
- [EnterpriseDB: TABLESAMPLE](https://www.enterprisedb.com/blog/tablesample-and-other-methods-getting-random-tuples)
- [Redgate: cache hit ratio](https://www.red-gate.com/hub/product-learning/redgate-monitor/understanding-postgresqls-cache-hit-ratio)
- [pgedge: cold page reads](https://www.pgedge.com/blog/optimising-cold-page-reads-in-postgresql)
- [Meta: static analysis of SQL queries](https://engineering.fb.com/2022/11/30/data-infrastructure/static-analysis-sql-queries/)