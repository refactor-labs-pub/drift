I have all the material needed. Now let me compose the comprehensive deliverable.

---

# Drift Static Profiler: New SQL Lint Rules + Plugin Architecture Research

## Part 1 — Concrete IO-cost rules to ship next

Below are 12 new rules that fit cleanly behind the existing `SqlRule` descriptor in `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/sql_lint.rs`. Each one is purely AST-driven (no DB, no schema), each cites prior art, and each is expressed in terms of the `sqlparser-rs` types already imported in that file.

### SQL012 — `SELECT` with no `WHERE` and no `LIMIT` (unbounded scan)

**Severity: High · Effort: Trivial**

The single highest-yield IO rule. A plain `SELECT col FROM t` (no `WHERE`, no `LIMIT`) is the canonical full-table scan. sqlcheck flags this under [rule 3001 (SELECT *)](https://github.com/jarulraj/sqlcheck/blob/master/docs/query/3001.md), Sentry tags it as a "Slow DB Query" when it crosses [500 ms 100 times in 24 h](https://docs.sentry.io/product/issues/issue-details/performance-issues/slow-db-queries/), and pganalyze's [Slow Scan insight](https://pganalyze.com/docs/explain/insights) is the runtime equivalent.

AST pattern:
```rust
fn matches_unbounded_scan(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    if q.limit_clause.is_some() { return false }            // bounded → safe
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    s.selection.is_none() && !s.from.is_empty()
        && !is_aggregate_only(&s.projection)                // COUNT(*)/SUM() = scalar
        && s.group_by_is_empty()
}
```

`is_aggregate_only` returns true if every `SelectItem` is `Expr::Function` with an aggregate name (`COUNT`,`SUM`,`AVG`,`MIN`,`MAX`) — those queries are O(N) by construction and the user already accepted that cost.

### SQL013 — Deep `OFFSET` literal (deep-pagination smell)

**Severity: Medium · Effort: Medium**

Postgres has to read-and-discard every row before the offset; classic O(offset) walk. [Vlad Mihalcea's keyset pagination article](https://vladmihalcea.com/sql-seek-keyset-pagination/) is the canonical write-up; pganalyze tags this at runtime under the [Large Offset insight](https://pganalyze.com/docs/explain/insights). Threshold: 1000 mirrors pganalyze's published tip catalog.

AST: `q.limit_clause` is `Some(LimitClause::LimitOffset { offset: Some(off), .. })`; extract numeric literal from `off.value`; fire if `>= 1000`.

### SQL014 — `HAVING` with no aggregate (should be `WHERE`)

**Severity: Low · Effort: Trivial**

`HAVING` runs *after* the group; predicates that don't reference an aggregate should run in `WHERE` so the planner can push them under the GROUP BY. This is sqlcheck rule 3013 (HAVING Clause Usage) and [Brass & Goldberg's paper](https://users.informatik.uni-halle.de/~brass/sem04_2/semerr1.pdf) makes the same observation ("In the HAVING-clause … conditions that are possible under WHERE are better written there").

AST:
```rust
let SetExpr::Select(s) = q.body.as_ref() else { return false };
s.having.as_ref()
    .map(|h| !expr_contains_aggregate(h))
    .unwrap_or(false)
```

`expr_contains_aggregate` walks the expression, checking for `Expr::Function` whose name (case-insensitive) is in `{COUNT, SUM, AVG, MIN, MAX, ARRAY_AGG, STRING_AGG, JSON_AGG, BOOL_AND, BOOL_OR, EVERY, STDDEV*, VAR*}` or `f.over.is_some()` (window aggregate counts).

### SQL016 — Unnecessary `DISTINCT` (no JOIN, no UNION, no set op)

**Severity: Low · Effort: Trivial**

`DISTINCT` triggers a sort/hash dedup pass. Without a JOIN or a set op, a single-table `SELECT DISTINCT col FROM t` is almost always wrong — the user probably wanted a `GROUP BY`. sqlcheck flags this as rule 3011 ("Eliminate Unnecessary DISTINCT Conditions"); Karwin discusses it in the [Implicit Columns / Ambiguous Groups](https://medium.com/pragmatic-programmers/chapter-15-ambiguous-groups-5dc6b667d8dc) chapters.

AST: `s.distinct.is_some() && s.from.iter().all(|t| t.joins.is_empty()) && s.from.len() == 1 && !is_set_operation(q.body.as_ref())`.

### SQL017 — Aggregate + non-aggregate without `GROUP BY` (ambiguous group)

**Severity: High · Effort: Small** (it's an outright bug in standard SQL — MySQL silently picks a row)

Karwin Chapter 15 ("Ambiguous Groups") is the definitive treatment. Postgres rejects it; MySQL in non-strict mode picks an arbitrary row per group. Brass & Goldberg cover it as Error 6.

AST: in a `Select`, count `n_agg = projection.iter().filter(is_aggregate_item).count()`, `n_plain = projection.iter().filter(is_plain_identifier).count()`. Fire if `s.group_by_is_empty() && n_agg >= 1 && n_plain >= 1`.

### SQL018 — `COUNT(*)` subquery used purely for existence

**Severity: Medium · Effort: Small**

`WHERE (SELECT COUNT(*) FROM t WHERE …) > 0` forces the inner query to count every matching row even though one row suffices. Rewrite as `EXISTS`. Karwin's "Spaghetti Query" chapter and Brass & Goldberg Error 14 both cover it.

AST: walk WHERE; match `Expr::BinaryOp { left: Expr::Subquery(q), op: Gt|GtEq|NotEq, right: Expr::Value(0|1) }` where `q.body` is a `Select` whose only projection is a `COUNT(*)` function call.

### SQL019 — Correlated subquery in `SELECT` projection

**Severity: High · Effort: Medium**

A correlated subquery in a SELECT list is executed per outer row — classic N+1-inside-one-query. Karwin discusses it under "Spaghetti Query"; sqlcheck rule 3014 ("Nested sub queries") is the loose equivalent. pganalyze's [Nested Loop insight](https://pganalyze.com/docs/explain/insights) is the runtime version, and [pgMustard's Nested Loop tip](https://www.pgmustard.com/docs/explain/nested-loop) explains the cost model.

AST: for each `SelectItem`, if it contains `Expr::Subquery(q)`, scan `q` for any identifier whose qualifier matches a table alias in the *outer* `from`. If found → correlated. (Heuristic only: bare identifiers without a qualifier are common in correlated subqueries; ship a stricter v1 that only fires when the inner WHERE references an aliased qualifier present in the outer FROM.)

### SQL020 — Implicit type-coercion comparison

**Severity: Medium · Effort: Small**

`WHERE varchar_col = 1` forces the planner to cast every row's string to int (or vice versa); index is unusable. Brass & Goldberg Error 24 ("Comparison between attributes with incompatible types"); Karwin's "Fear of the Unknown" chapter touches it too.

AST: `Expr::BinaryOp { left, right, op: Eq|NotEq|Gt|… }` where `left` is `Identifier`/`CompoundIdentifier` and `right` is `Expr::Value(Number(_))`, **and** the column name has a string-suggesting suffix (`_id` when stored as varchar, `_code`, `_uuid`, `name`, `email`). Pure-syntactic so it's noisy — keep severity Medium and confidence ~0.6 (lower than the 0.9 default in `to_finding`).

Better signal: any `Expr::Cast` or `Expr::Convert` wrapping a column on either side of a comparison — same anti-index defeat as SQL009.

### SQL021 — `GROUP BY` ordinal positions

**Severity: Low · Effort: Trivial**

`GROUP BY 1, 2` is deprecated in standard SQL and silently misaligns when the SELECT list reorders. [sqlfluff AM06 ("ambiguous.column_references")](https://docs.sqlfluff.com/en/stable/reference/rules.html) enforces consistent ordinal vs. named usage. Brass & Goldberg Error 21.

AST: `s.group_by` matches `GroupByExpr::Expressions(exprs, _)` and any expr is `Expr::Value(Number(_))`.

### SQL022 — `ORDER BY` constant expression

**Severity: Low · Effort: Trivial**

`ORDER BY 'foo'`, `ORDER BY 1=1`, `ORDER BY NULL` are no-ops the planner has to recognize and discard. Brass & Goldberg Error 21 covers it; sqlfluff [AM09](https://docs.sqlfluff.com/en/stable/reference/rules.html) is adjacent.

AST: walk `OrderByKind::Expressions(items)`; if every `item.expr` is `Expr::Value(_)` (literal) or constant `Expr::BinaryOp` whose both sides are literals → fire.

### SQL023 — `NOT IN` (literal list) including `NULL`

**Severity: High · Effort: Trivial**

`WHERE x NOT IN (1, 2, NULL)` always returns empty. Brass & Goldberg Error 9 ("NULL in NOT IN"); Karwin's "Fear of the Unknown" chapter is the definitive reference.

AST: `Expr::InList { list, negated: true, .. }` where any `list[i]` is `Expr::Value(Value::Null)`.

### SQL024 — Self-join without distinct alias or join key

**Severity: Medium · Effort: Small**

`FROM users JOIN users ON …` without aliasing is a parse error in most dialects, but `FROM users a JOIN users b` without a meaningful `ON` (only `a.id = b.id`, which is identity) generally indicates either a typo or a hierarchical query missing a parent-key. Brass & Goldberg Error 27.

AST: collect `(table_name, alias)` pairs across `s.from` and joins; if any base table appears ≥2 times, walk each `JoinConstraint::On` to verify the equality involves two *different* columns (not `a.id = b.id`). Heuristic — confidence 0.55.

---

## Part 2 — Spaghetti-Query / complexity thresholds from literature

### sqlcheck "Spaghetti Query" — character count

Rule [3008](https://github.com/jarulraj/sqlcheck/blob/master/docs/query/3008.md) fires on raw character count; the implementation hardcodes **500 characters** in the C++ source (`min_statement_length`). Threshold matches what Stéphane Derosiaux uses in ["The Rise of Spaghetti SQL"](https://sderosiaux.medium.com/the-rise-of-spaghetti-sql-ab13bd5d0bb0). Suggested drift rule:

### SQL_COMPLEX_LENGTH — statement >500 normalized chars

**Severity: Medium · Effort: Medium**

Implementation: reuse the `fingerprint` whitespace-normalizer; fire if normalized length ≥ 500. Cheap to compute, very high recall for "this query needs to be refactored into CTEs."

### Cyclomatic complexity for SQL — Subali & Rochimah 2018

[Subali & Rochimah, "A new model for measuring the complexity of SQL commands"](https://www.semanticscholar.org/paper/A-new-model-for-measuring-the-complexity-of-SQL-Subali-Rochimah/b52df94315738c142a2e80b0130125c9f5a181c2) gives a SQL-adapted cyclomatic number: 1 per `WHERE/AND/OR`, +1 per `JOIN`, +1 per subquery, +1 per `UNION`, +1 per `CASE WHEN`. Threshold for "complex" follows McCabe's original ≥10. We already cover joins (SQL_COMPLEX_JOINS), subquery depth, and OR-chain — adding a unified score gives a single overall "this is too much" signal.

### Halstead-for-SQL

[Jain & Vashistha (SQLShare)](https://uwescience.github.io/sqlshare/pdfs/Jain-Vashistha.pdf) adapt Halstead operators (`SELECT, FROM, WHERE, JOIN, GROUP, HAVING, ORDER, LIMIT, UNION, DISTINCT, NOT, IN, EXISTS, AND, OR, …`) and operands (column refs, literals, function names). The [Wikipedia Halstead page](https://en.wikipedia.org/wiki/Halstead_complexity_measures) gives the formulas: Volume `V = N · log2(n)` where `N = total tokens`, `n = unique tokens`. Adoption is low; cite as a roadmap rule, don't ship as v1 — too much false-positive risk vs. statement length.

### Postgres `join_collapse_limit` (default 8)

[PostgreSQL docs](https://www.postgresql.org/docs/current/runtime-config-query.html) state: *"By default, this variable is set the same as `from_collapse_limit`, which is appropriate for most uses."* and *"Setting this value to `geqo_threshold` or more may trigger use of the GEQO planner, resulting in non-optimal plans."* `geqo_threshold` default is 12. So the cost-cliff is at 8 (planner switches to merge-into-upper-query heuristics) and again at 12 (planner switches to genetic search). Our existing SQL_COMPLEX_JOINS fires at ≥6, which is a conservative warn-before-cliff; consider a **SQL_GEQO_RISK** rule firing at ≥12 with `Severity::High` to catch queries that are *definitely* going to get a worse plan than written.

### Apache Calcite "rule application depth"

Calcite uses depth-3 as a soft cutoff inside `RelOptUtil` (when pushing past `Project` rules: *"if the visiting depth exceeds 3, the process returns because … new created sub-trees have layers bigger than that"* — [datacadamia](https://datacadamia.com/db/calcite/planner)). Not a query-complexity metric per se — it's a *planner* recursion limit — so it's a weak proxy. Useful only as inspiration for our existing subquery-depth rule.

---

## Part 3 — Generic IO patterns (no schema, AST-only)

### Pattern: WHERE on an unindexable expression

Already covered by SQL009 (function-wrapping). Extending it:

- **JSON path access in WHERE**: `WHERE data->>'email' = 'a@b'` without a documented functional index. Postgres docs and [pganalyze's GIN-index guide](https://pganalyze.com/blog/gin-index) note that `jsonb_ops` GIN doesn't accelerate `->>` text equality; `jsonb_path_ops` only accelerates `@>`. We can't see the schema but we can flag the AST shape and tell the user "if there isn't a functional index on `(data->>'email')`, you're sequentially scanning."

  AST: `Expr::JsonAccess { .. }` (or `Expr::BinaryOp { op: BinaryOperator::LongArrow|Arrow, ... }`) on either side of a WHERE comparison. Fire as **SQL025**, Severity Medium, message references `CREATE INDEX ... USING GIN ((data->>'email'))`.

### Pattern: `WHERE col IS NULL` on a column that's likely high-cardinality NOT-NULL

Pure-AST heuristic won't catch this without a schema; skip.

### Pattern: `LIKE 'foo'` without wildcards (use `=`)

Tiny win, but the planner can sometimes pick a different plan. Mention but don't ship — too cosmetic.

### Pattern: `IN (SELECT …)` that could be a JOIN

When `WHERE x IN (SELECT y FROM t)` and the outer query doesn't need any other column from `t`, Postgres usually picks a semi-join. But some legacy MySQL versions execute the inner per-row. Skip — modern planners handle this.

---

## Part 4 — What Sentry / Datadog / pganalyze call "over-complicated"

**Sentry's published thresholds** ([Slow DB Queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/slow-db-queries/), [N+1 Queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/n-one-queries/), [Consecutive DB Queries](https://docs.sentry.io/product/issues/issue-details/performance-issues/consecutive-db-queries/)):
- Slow DB query: ≥500 ms duration, seen 100 times in 24 h.
- Consecutive DB queries: total duration of each parallelizable span >30 ms, and time-saved-ratio >0.1.
- N+1: ≥1 source span + a sequence of similar repeated db spans (same fingerprint).

These are runtime thresholds — drift doesn't have runtime, but the *fingerprints* Sentry parameterizes are pure-SQL (whitespace + literal replacement), which is exactly what our `fingerprint()` in sql_lint.rs already does. So drift's SQL anti-pattern matches map 1:1 onto Sentry "slow query" fingerprints — useful framing for the docs.

**Datadog** publishes no specific complexity threshold in their [DBM docs](https://docs.datadoghq.com/database_monitoring/query_metrics/); they leave it to user-configured anomaly detection.

**pganalyze** publishes 8 EXPLAIN insights ([page](https://pganalyze.com/docs/explain/insights)): Disk Sort, Hash Batches, Inefficient Index, Large Offset, Lossy Bitmaps, Mis-Estimate, Slow Scan, Stale Stats. Of these, only **Large Offset** is purely-SQL-shape; the rest need runtime EXPLAIN. That's our SQL013 above — drift can preempt pganalyze's #1 most-common insight purely statically.

**depesz color thresholds** ([help page](https://explain.depesz.com/help)): rows-X yellow at 10×, orange at 100×, red at 1000×; exclusive-time yellow >10%, brown >50%, red >90%. These map cleanly onto drift's Severity enum if we ever ingest live EXPLAIN; for now they shape the *Severity* assignment policy: Low for cosmetic, Medium for "could be 10× slower", High for "could be 1000× slower or wrong."

**pgMustard's** rules-of-thumb that we can lift today: [Rows Removed by Filter](https://www.pgmustard.com/docs/explain/rows-removed-by-filter) (>5:1 ratio = missing index — runtime, skip), [Nested Loop](https://www.pgmustard.com/docs/explain/nested-loop) (problematic when inner loop is a Seq Scan — runtime). Statically we mirror these via SQL019 (correlated-subquery) and SQL_COMPLEX_JOINS.

---

## Part 5 — How other linters expose AST visitors to plugins

The architectural question is: **do we want drift's rule registry to be open to third-party crates, or open only to internal commits?** Here's how the field does it.

### Clippy (Rust) — internal-only, no stable plugin API

[Clippy docs / Trail of Bits writeup](https://blog.trailofbits.com/2021/11/09/write-rust-lints-without-forking-clippy/): Clippy uses `rustc_private` and links against unstable `rustc_driver` + `rustc_lint`. Rules implement `EarlyLintPass` or `LateLintPass`. The `clippy_utils` crate is shared internally but explicitly unstable. A third-party would have to ship its own driver binary and a fixed rustc nightly. **Verdict: clippy is a closed registry; "plugins" are forks.**

### Ruff (Python, in Rust) — explicitly closed, plugins out-of-scope-for-now

[Ruff FAQ](https://docs.astral.sh/ruff/faq/): *"Ruff's primary limitation vis-à-vis Flake8 is that it does not support custom lint rules"* and *"Ruff does not yet support third-party plugins, though a plugin system is within-scope for the project."* Every flake8 plugin was re-implemented as a first-party rule. Rule codes use prefix-conventions (`F` Pyflakes, `E` pycodestyle, `B` flake8-bugbear, etc.).

**This is the model drift currently follows** — `BUILTIN_RULES` as a const array, one source of truth, no dynamic dispatch. It scales to thousands of rules and stays fast.

### ESLint (JS) — open plugin registry, RuleListener interface

[ESLint Custom Rules docs](https://eslint.org/docs/latest/extend/custom-rules): a rule is a JS object with a `create(context)` method that returns a `RuleListener` — an object whose keys are AST node-type names (e.g. `CallExpression`, `VariableDeclarator:exit`) and values are visitor functions. ESTraverse drives the walk; events fire on enter and on `:exit`. The `context` arg gives the rule access to `context.report({ node, message, fix })`. Plugins ship as npm packages exporting `{ rules: { "my-rule": myRule } }`. Public API is in `lib/api.js`: `Linter`, `RuleTester`, `SourceCode`.

### Staticcheck (Go) — open plugin registry via `go/analysis`

[Staticcheck](https://staticcheck.dev/): every check is an `analysis.Analyzer` struct ([package](https://pkg.go.dev/golang.org/x/tools/go/analysis)):

```go
type Analyzer struct {
    Name     string
    Doc      string
    Run      func(*Pass) (interface{}, error)
    Requires []*Analyzer        // dependencies
    Flags    flag.FlagSet
    ResultType reflect.Type
}
```

Third parties drop their Analyzer into any `go/analysis`-aware driver (go vet, golangci-lint, nogo). The `Pass` carries the AST and a `Report(diag)` method.

### PMD (Java) — open registry, two paths: XPath or Java visitor

[PMD docs](https://docs.pmd-code.org/latest/pmd_userdocs_extending_writing_java_rules.html): rules implement `Rule` (usually subclassing `AbstractJavaRule` which overrides `visit(ASTSomeNode)` methods — classic GoF visitor). Plugins are JARs added to PMD's classpath. The XPath path lets rule authors write rules declaratively in XML: `//IfStatement[condition[...]]`. Both ship in a rule-set XML file.

### Recommendation for drift

Mirror **Ruff's stance** for v1: keep the registry first-party, append to `BUILTIN_RULES`. The current `SqlRule` struct already has exactly the right shape — `id`, `severity`, `effort`, `message`, `remediation`, `matches: fn(&Statement) -> bool`. This is the `analysis.Analyzer` pattern with a stricter signature.

If/when you want to open it (say, for an "enterprise rule pack" or domain-specific lints — Django ORM idioms, SQLAlchemy idioms — that don't belong in the core), the cleanest seam is:

1. **Make `SqlRule` `pub`** in a new sub-module `drift_static_profiler::sql_rules`. The function-pointer field `matches: fn(&Statement) -> bool` is already `Send + Sync + 'static` and works in a `Vec<&'static SqlRule>` lazily concatenated from multiple `inventory::iter`-style registries.
2. **Expose a `register_rule(&'static SqlRule)`** entrypoint that pushes into a `LazyLock<Mutex<Vec<&'static SqlRule>>>`. The dispatcher iterates `BUILTIN_RULES.iter().chain(EXTERNAL.lock().iter().copied())`.
3. **Use the [`inventory`](https://crates.io/crates/inventory) crate** for compile-time plugin discovery — a third-party crate declares `inventory::submit! { SqlRule { … } }`, the binary picks it up automatically. This is the Rust-idiomatic version of Go's "blank import for side-effect registration" and ESLint's `plugins:` config.
4. **Keep `sqlparser::ast::*` re-exported** from your public surface — that's the AST contract. Third-party rules pattern-match on `&Statement`. This is exactly what ESLint does with ESTree node types.

The downside ESLint and PMD users complain about — plugin version skew when the AST changes — is bounded for us because `sqlparser-rs` versions are pinned in `Cargo.toml` and the AST is a public crate (unlike rustc's HIR). So drift's plugin story can be *much* cleaner than Clippy's.

---

## Implementer's afternoon checklist

To ship 10 new rules in ~half a day, append the following IDs to `BUILTIN_RULES` and add a matcher fn each. Order by ROI:

1. **SQL012** unbounded SELECT — biggest IO win, trivial AST (Severity High)
2. **SQL013** OFFSET ≥1000 — easy literal extraction (Medium)
3. **SQL014** HAVING without aggregate — needs `expr_contains_aggregate` helper (Low)
4. **SQL017** ambiguous group — needs `is_aggregate_item` + `is_plain_identifier` (High)
5. **SQL023** NOT IN list with NULL literal — single AST shape (High)
6. **SQL016** unnecessary DISTINCT — `s.distinct.is_some()` + no-join check (Low)
7. **SQL021** GROUP BY ordinal — `Expr::Value(Number)` in GROUP BY (Low)
8. **SQL022** ORDER BY constant — all-literal expressions in OrderBy (Low)
9. **SQL_COMPLEX_LENGTH** ≥500 chars normalized — reuse `fingerprint()` (Medium)
10. **SQL025** JSON path in WHERE — `Expr::JsonAccess` walk (Medium)

Each rule = one `SqlRule { … }` literal + one `fn matches_xxx(&Statement) -> bool`. Re-use the existing `walk_where_exprs` visitor for predicate-shape rules. Tests follow the same `ids_for(...)` pattern already in the file (lines 863-1185).

Stretch goal (not afternoon, but next-day): SQL018, SQL019, SQL020, SQL024 — each needs one new helper (correlated-ref detection, cast-walking, self-join key analysis).

---

## Citations

- sqlcheck repo: https://github.com/jarulraj/sqlcheck — Apache 2.0; 28 rules across 4 categories.
- sqlcheck rule 3001 (SELECT *): https://github.com/jarulraj/sqlcheck/blob/master/docs/query/3001.md
- sqlcheck rule 3008 (spaghetti query, 500-char threshold): https://github.com/jarulraj/sqlcheck/blob/master/docs/query/3008.md
- sqlfluff rules reference: https://docs.sqlfluff.com/en/stable/reference/rules.html
- Brass & Goldberg, "Semantic Errors in SQL Queries: A Quite Complete List" (Halle): https://users.informatik.uni-halle.de/~brass/sem04_2/semerr1.pdf
- Brass & Goldberg, "Detecting Logical Errors in SQL Queries": https://dbs.informatik.uni-halle.de/sqllint/brass_gdb04.pdf
- Karwin, "SQL Antipatterns" (Pragmatic): https://pragprog.com/titles/bksqla/sql-antipatterns/
- Karwin Ch.15 Ambiguous Groups: https://medium.com/pragmatic-programmers/chapter-15-ambiguous-groups-5dc6b667d8dc
- Vlad Mihalcea, SQL Seek Method / Keyset Pagination: https://vladmihalcea.com/sql-seek-keyset-pagination/
- pganalyze EXPLAIN Insights: https://pganalyze.com/docs/explain/insights
- pganalyze GIN index guide: https://pganalyze.com/blog/gin-index
- pgMustard Nested Loop: https://www.pgmustard.com/docs/explain/nested-loop
- pgMustard Rows Removed by Filter: https://www.pgmustard.com/docs/explain/rows-removed-by-filter
- depesz EXPLAIN color thresholds: https://explain.depesz.com/help
- Sentry Slow DB Queries: https://docs.sentry.io/product/issues/issue-details/performance-issues/slow-db-queries/
- Sentry N+1 Queries: https://docs.sentry.io/product/issues/issue-details/performance-issues/n-one-queries/
- Sentry Consecutive DB Queries: https://docs.sentry.io/product/issues/issue-details/performance-issues/consecutive-db-queries/
- Datadog DBM Query Metrics: https://docs.datadoghq.com/database_monitoring/query_metrics/
- PostgreSQL `join_collapse_limit`: https://www.postgresql.org/docs/current/runtime-config-query.html
- PostgreSQL GIN indexes: https://www.postgresql.org/docs/current/gin.html
- Subali & Rochimah, "A new model for measuring the complexity of SQL commands": https://www.semanticscholar.org/paper/A-new-model-for-measuring-the-complexity-of-SQL-Subali-Rochimah/b52df94315738c142a2e80b0130125c9f5a181c2
- Sharma et al., "Smelly Relations" SANER 2018: https://faculty.cc.gatech.edu/~jarulraj/courses/8803-f18/papers/smelly_relations.pdf
- Jain & Vashistha, Measuring Query Complexity in SQLShare (Halstead-for-SQL): https://uwescience.github.io/sqlshare/pdfs/Jain-Vashistha.pdf
- Halstead complexity (Wikipedia): https://en.wikipedia.org/wiki/Halstead_complexity_measures
- Apache Calcite RelOptPlanner: https://calcite.apache.org/javadocAggregate/org/apache/calcite/plan/RelOptPlanner.html
- Apache Calcite planner overview: https://datacadamia.com/db/calcite/planner
- Clippy adding lints: https://doc.rust-lang.org/nightly/clippy/development/adding_lints.html
- Trail of Bits — Write Rust lints without forking Clippy: https://blog.trailofbits.com/2021/11/09/write-rust-lints-without-forking-clippy/
- Ruff FAQ (no plugin API): https://docs.astral.sh/ruff/faq/
- ESLint Custom Rules: https://eslint.org/docs/latest/extend/custom-rules
- Staticcheck: https://staticcheck.dev/ and go/analysis API: https://pkg.go.dev/golang.org/x/tools/go/analysis
- PMD writing Java rules: https://docs.pmd-code.org/latest/pmd_userdocs_extending_writing_java_rules.html
- Derosiaux, "The Rise of Spaghetti SQL": https://sderosiaux.medium.com/the-rise-of-spaghetti-sql-ab13bd5d0bb0