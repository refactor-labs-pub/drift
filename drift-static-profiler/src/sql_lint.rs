//! SQL Query Analyzer — Phase 1 of the plan
//! ([QUERY_ORM_ANALYZER_PLAN.md](../QUERY_ORM_ANALYZER_PLAN.md) §3).
//!
//! Architecture (SOLID-shaped, Open/Closed first):
//!
//!   1. Each rule is a `SqlRule` *descriptor* — a plain struct that
//!      bundles its metadata (id, severity, effort, message,
//!      remediation) with a single pure function `matches: fn(&Statement)
//!      -> bool` that recognizes the AST pattern.
//!   2. The full catalog lives in one place: the `BUILTIN_RULES` const array.
//!      Adding a new rule = appending one struct literal. **No existing
//!      code is modified** — Open/Closed Principle.
//!   3. The dispatcher (`check_statement`) iterates the catalog and
//!      filters. It has *no* knowledge of which rules exist.
//!   4. The matchers (`matches_select_star`, …) are tiny SRP-shaped
//!      predicates. They neither carry copy nor allocate.
//!   5. Per-call-site SQL is parsed ONCE per unique fingerprint
//!      (whitespace-normalized hash) — `attach_sql_antipatterns` is
//!      O(unique-SQL + call-sites), not O(all-call-sites).
//!
//! v1 ships four rules — the load-bearing high-precision shapes:
//!   - `SQL001` SELECT *        — list columns explicitly
//!   - `SQL002` DELETE w/o WHERE — accidental table wipe
//!   - `SQL003` UPDATE w/o WHERE — accidental full-table mutation
//!   - `SQL004` INSERT no cols   — schema-drift hazard
//!
//! False-positive policy (plan §8): if `sqlparser-rs` can't parse the
//! input (dialect-specific syntax, dynamic fragments, fstring stubs),
//! we silently skip — the generic `n_plus_one` / `db-in-loop` finding
//! from `insights.rs` still fires from the category-level detector, so
//! no regression in coverage, just no rule-level specificity.

use crate::insights::{Effort, Evidence, Finding, FindingKind, Severity};
// Generic SQL AST predicates — extracted into a sibling module so any
// future rule engine (ORM lint, migration lint, ORM→SQL predictor)
// composes against the same vocabulary. See `src/sql_ast.rs`.
use crate::sql_ast::{
    collect_or_chain_columns, count_joins_in_query, count_largest_in_list, expr_calls_random,
    group_by_uses_ordinal, has_leading_wildcard, having_lacks_aggregate,
    is_function_on_identifier, literal_string, max_subquery_depth_in_query,
    order_by_is_all_constant, or_chain_length, query_has_distinct, query_has_order_by,
    query_offset_value, select_has_mixed_aggregation_no_group_by, set_expr_has_implicit_union,
    walk_where_exprs, where_contains_equality, where_has_not_in_with_null, where_uses_json_path,
};
use crate::tree::CallTreeNode;
use sqlparser::ast::{BinaryOperator, Expr, OrderByKind, SelectItem, SetExpr, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParser;
use std::collections::{HashMap, HashSet};

// ────────────────────────────────────────────────────────────────────
// Rule descriptor — the OCP seam
// ────────────────────────────────────────────────────────────────────

/// Self-contained description of a SQL anti-pattern rule. All fields
/// are `&'static` so the catalog can live in a `const` array — no
/// heap, no lock, no init cost.
///
/// **Public** so downstream crates / tests can compose extra rules
/// alongside [`BUILTIN_RULES`] via [`attach_sql_antipatterns_with`].
/// Build `matches` predicates with the AST visitors in
/// [`crate::sql_ast`] — they carry the SRP-shaped vocabulary every
/// rule shares.
///
/// Adding a new rule to drift's built-in catalog means appending one
/// literal to [`BUILTIN_RULES`]. **Nothing else changes** (OCP).
#[derive(Debug, Clone, Copy)]
pub struct SqlRule {
    /// Stable identifier surfaced in evidence chip + remediation links.
    pub id: &'static str,
    pub severity: Severity,
    pub effort: Effort,
    /// One-line user-facing description of what's wrong.
    pub message: &'static str,
    /// Concrete fix instruction.
    pub remediation: &'static str,
    /// Pure predicate over the parsed AST. Returns `true` iff this rule
    /// applies to this statement.
    pub matches: fn(&Statement) -> bool,
}

impl SqlRule {
    /// Derive the `FindingKind` from the rule-id prefix. Single source
    /// of truth — the naming convention is already enforced by
    /// `rule_catalog_invariants`, so this is removing redundancy, not
    /// adding magic.
    ///
    /// Mapping:
    ///   - `SQL*`  → [`FindingKind::SqlAntipattern`] (DML query-shape lint)
    ///   - `MIG_*` → [`FindingKind::MigrationSafety`] (DDL schema-change hazard)
    ///
    /// Why split: query-shape and schema-change are different concerns —
    /// different audience (app dev vs ops), different severity calibration,
    /// different remediation playbook. Lumping them under one
    /// `findings_by_kind` bucket made the summary opaque ("316 sql
    /// findings" — which were query problems vs migration hazards?).
    pub fn kind(&self) -> FindingKind {
        if self.id.starts_with("MIG_") {
            FindingKind::MigrationSafety
        } else {
            FindingKind::SqlAntipattern
        }
    }

    /// Materialize a `Finding` anchored at a call-site line.
    fn to_finding(&self, line: usize, sql_snippet: &str) -> Finding {
        Finding {
            kind: self.kind(),
            severity: self.severity,
            effort: self.effort,
            // SQL parser saw it — high confidence in the AST shape.
            confidence: 0.90,
            line,
            message: self.message.to_string(),
            evidence: vec![
                // First evidence row carries the rule id chip per plan
                // §2.2; viewer renders it as a badge next to severity.
                Evidence {
                    call: self.id.to_string(),
                    line,
                    category: None,
                },
                // Second row carries the SQL snippet for triage. We
                // truncate aggressively so the JSON stays small.
                Evidence {
                    call: truncate_sql(sql_snippet, 120),
                    line,
                    category: None,
                },
            ],
            remediation: Some(self.remediation.to_string()),
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// The catalog — open for extension. New rules append below.
// ────────────────────────────────────────────────────────────────────

/// drift's built-in SQL anti-pattern catalog. Downstream code that
/// wants to add rules calls [`attach_sql_antipatterns_with`] passing a
/// slice that concatenates this with custom rules.
pub const BUILTIN_RULES: &[SqlRule] = &[
    SqlRule {
        id: "SQL001",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`SELECT *` returns every column — bandwidth waste and a schema-drift hazard (new columns silently appear in results)",
        remediation: "List columns explicitly. If you genuinely need every column, use a deliberate marker comment so reviewers can verify.",
        matches: matches_select_star,
    },
    SqlRule {
        id: "SQL002",
        severity: Severity::High,
        effort: Effort::Trivial,
        message: "`DELETE` has no `WHERE` clause — this will erase every row in the target table(s)",
        remediation: "Add a `WHERE` clause. If the intent really is to clear the table, use `TRUNCATE` so the planner and replication know it's intentional.",
        matches: matches_delete_no_where,
    },
    SqlRule {
        id: "SQL003",
        severity: Severity::High,
        effort: Effort::Trivial,
        message: "`UPDATE` has no `WHERE` clause — every row in the table will be rewritten",
        remediation: "Add a `WHERE` clause. Full-table updates are almost always a typo — if intentional, use a comment and run inside a transaction so it can be rolled back.",
        matches: matches_update_no_where,
    },
    SqlRule {
        id: "SQL004",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`INSERT` has no column list — positional binding will silently misalign when the table schema changes",
        remediation: "Name the columns explicitly: `INSERT INTO t (col1, col2) VALUES (...)`. Schema-drift won't break callers.",
        matches: matches_insert_no_columns,
    },
    SqlRule {
        id: "SQL005",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "`LIKE` pattern has a leading wildcard (`%…` or `_…`) — the planner cannot use a B-tree index on the column",
        remediation: "Use a trigram (`pg_trgm`), full-text (`tsvector`/`FULLTEXT`), or reverse-string index. For prefix search, drop the leading wildcard.",
        matches: matches_like_leading_wildcard,
    },
    SqlRule {
        id: "SQL006",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "`ORDER BY RANDOM()`/`RAND()`/`NEWID()` sorts the entire result set just to pick a random row — O(N) work for an O(1) need",
        remediation: "Use `TABLESAMPLE` (Postgres/SQL Server), pick ids in app code, or pre-compute a random column to sort on.",
        matches: matches_order_by_random,
    },
    SqlRule {
        id: "SQL007",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "Long `OR` chain on the same column — the planner often can't use an index efficiently. Use `IN (…)` to give the optimizer a fighting chance.",
        remediation: "Rewrite `WHERE c = a OR c = b OR c = c` as `WHERE c IN (a, b, c)`. The semantics are identical and the plan is far more likely to use an index.",
        matches: matches_or_chain_on_col,
    },
    SqlRule {
        id: "SQL009",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "Function wrapping a column in `WHERE` (e.g. `LOWER(email) = …`, `DATE(created_at) = …`) defeats a plain B-tree index on that column",
        remediation: "Add a functional index (`CREATE INDEX ... ON t(LOWER(email))`), normalize at write time, or store a derived column. See also Postgres' generated columns.",
        matches: matches_function_in_where,
    },
    SqlRule {
        id: "SQL010",
        severity: Severity::High,
        effort: Effort::Small,
        message: "Multiple tables in `FROM` with no `JOIN` and no join predicate in `WHERE` — likely an accidental Cartesian product",
        remediation: "Use explicit `JOIN ... ON ...` syntax. If you really intend a cross join, write `CROSS JOIN` so the intent is unambiguous.",
        matches: matches_cartesian_join,
    },
    SqlRule {
        id: "SQL011",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "`NOT IN (SELECT …)` returns an empty set the moment the inner query yields any NULL — almost always a bug, and `NOT EXISTS` is faster anyway",
        remediation: "Rewrite as `WHERE NOT EXISTS (SELECT 1 FROM ... WHERE ...)` — handles NULLs sanely and the planner usually prefers an anti-join.",
        matches: matches_not_in_with_subquery,
    },
    SqlRule {
        id: "SQL015",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`UNION` (without `ALL`) does an implicit `DISTINCT` — a global sort just to dedup. If you know rows are disjoint, use `UNION ALL`.",
        remediation: "When the two SELECTs cannot produce overlapping rows (different time windows, different status partitions, etc.), use `UNION ALL` and skip the sort.",
        matches: matches_union_distinct_implicit,
    },
    // ── Complexity rules — flag "over-complicated" queries that work
    //    correctly but are smells that compound at scale. Different
    //    framing from SQL00x: those are bugs; these are warnings.
    SqlRule {
        id: "SQL_COMPLEX_JOINS",
        severity: Severity::Medium,
        effort: Effort::Medium,
        message: "Query joins ≥6 tables — high planner cost (factorial join-order search), high optimizer-bias risk, and a smell that the query is doing too much",
        remediation: "Split into a CTE per logical step, or denormalize a read model. PostgreSQL's `join_collapse_limit` (default 8) starts to hurt around 8 tables.",
        matches: matches_join_count_over_5,
    },
    SqlRule {
        id: "SQL_COMPLEX_SUBQUERY_DEPTH",
        severity: Severity::Medium,
        effort: Effort::Medium,
        message: "Subquery depth ≥4 — deeply nested SELECTs are hard to reason about and rarely the cheapest plan",
        remediation: "Refactor with `WITH` (CTE) expressions or temporary tables. Postgres ≥12 inlines CTEs by default, so readability comes free.",
        matches: matches_subquery_depth_over_3,
    },
    SqlRule {
        id: "SQL_COMPLEX_OR_CHAIN",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`OR` chain with ≥7 branches in `WHERE` — predicate explosion. Even with indexes, the planner may give up and seq-scan.",
        remediation: "Replace with `IN (…)` for equality predicates, or move the disjunction into a UNION of selective queries.",
        matches: matches_long_or_chain,
    },
    // ── IO / shape rules — composed from sql_ast predicates.
    //    These exist purely to prove the "rules are data + predicate
    //    composition" architecture: each matcher is ONE call into
    //    sql_ast plus one threshold comparison.
    SqlRule {
        id: "SQL_LARGE_IN_LIST",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "`WHERE col IN (…)` with ≥100 literals — the planner builds a hash set per row, parser memory cost is O(N), and any version-controlled query that big is a signal that the data model is wrong",
        remediation: "Use a temp table + JOIN, or `VALUES`-table + JOIN, or hash a `bytea` for set-membership. PG14+ also has `= ANY('{…}'::int[])` which is more efficient than IN for very large lists.",
        matches: matches_large_in_list,
    },
    SqlRule {
        id: "SQL_DISTINCT_NO_ORDER_BY",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`SELECT DISTINCT …` without `ORDER BY` — DISTINCT sorts the whole result silently and consumers often assume an ordering that isn't guaranteed",
        remediation: "Add an explicit `ORDER BY` (often the same columns as DISTINCT). Better: replace with `GROUP BY` if you're collapsing rows by key, since GROUP BY is intent-revealing.",
        matches: matches_distinct_no_order_by,
    },
    SqlRule {
        id: "SQL_OFFSET_DEEP_PAGINATION",
        severity: Severity::Medium,
        effort: Effort::Medium,
        message: "`OFFSET N` with N ≥ 1000 — every page reads-and-discards N rows. Cost grows linearly with page number",
        remediation: "Switch to keyset (a.k.a. cursor) pagination: `WHERE (created_at, id) < (?, ?) ORDER BY created_at DESC, id DESC LIMIT k`. Constant-cost paging at any depth.",
        matches: matches_offset_deep_pagination,
    },
    SqlRule {
        id: "SQL_WIN_NO_PARTITION",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "Ranking/positional window function (`ROW_NUMBER`/`RANK`/`LAG`/…) with `ORDER BY` but no `PARTITION BY` — produces a global rank across every row in the table",
        remediation: "Add `PARTITION BY tenant_id` (or the relevant grouping key) so the rank scopes correctly. Aggregate windows (e.g. `SUM() OVER (ORDER BY …)` for running totals) legitimately omit `PARTITION BY` — this rule only fires on ranking/positional functions.",
        matches: matches_window_without_partition,
    },
    SqlRule {
        id: "SQL_SUB_SCALAR_IN_PROJECTION",
        severity: Severity::High,
        effort: Effort::Medium,
        message: "Scalar subquery in `SELECT` projection — runs once per outer row, the SQL-side equivalent of an ORM N+1",
        remediation: "Rewrite as a `LEFT JOIN (SELECT …, GROUP BY parent_id)` or a CTE. PostgreSQL's planner cannot always de-correlate the scalar form; on multi-thousand-row results this is one of the highest-impact rewrites available.",
        matches: matches_scalar_subquery_in_projection,
    },
    // ── Round-3 batch from plan §13b research catalog. Each matcher
    //    is ONE call into a sql_ast predicate.
    SqlRule {
        id: "SQL014_HAVING_NO_AGGREGATE",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`HAVING` clause has no aggregate function — the predicate should be in `WHERE` so the planner can push it under the GROUP BY",
        remediation: "Move the predicate from `HAVING` to `WHERE`. `HAVING` runs after grouping; `WHERE` runs before, letting the planner skip rows earlier.",
        matches: matches_having_no_aggregate,
    },
    SqlRule {
        id: "SQL017_AMBIGUOUS_GROUP",
        severity: Severity::High,
        effort: Effort::Small,
        message: "SELECT mixes aggregate + non-aggregate columns with no `GROUP BY` — Postgres rejects; MySQL silently picks an arbitrary row per implicit group",
        remediation: "Add `GROUP BY <non-aggregate columns>`, or wrap the non-aggregate columns in `MAX()`/`MIN()`/`ANY_VALUE()` if you genuinely don't care which row's value you get.",
        matches: matches_ambiguous_group,
    },
    SqlRule {
        id: "SQL021_GROUP_BY_ORDINAL",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`GROUP BY 1, 2` uses ordinal positions — silently misaligns when the SELECT projection is reordered",
        remediation: "Replace with named columns: `GROUP BY col1, col2`. Self-documenting and refactor-safe.",
        matches: matches_group_by_ordinal,
    },
    SqlRule {
        id: "SQL022_ORDER_BY_CONSTANT",
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "`ORDER BY` only references constants (`'x'`, `1=1`, `NULL`) — the clause is a no-op the planner still has to recognize and discard",
        remediation: "Remove the `ORDER BY` if you don't need ordering, or replace the constant expression with the column you actually want to sort by.",
        matches: matches_order_by_constant,
    },
    SqlRule {
        id: "SQL023_NOT_IN_WITH_NULL",
        severity: Severity::High,
        effort: Effort::Trivial,
        message: "`NOT IN (…, NULL, …)` always returns empty — the entire predicate becomes UNKNOWN as soon as one list element is NULL",
        remediation: "Replace with `NOT EXISTS` or filter NULLs out of the list: `WHERE x NOT IN (SELECT y FROM t WHERE y IS NOT NULL)`.",
        matches: matches_not_in_with_null,
    },
    SqlRule {
        id: "SQL025_JSON_PATH_IN_WHERE",
        severity: Severity::Medium,
        effort: Effort::Small,
        message: "`WHERE data->>'field' = …` uses a JSON-path operator on a column — without a functional / GIN index on the path the planner has to scan every row",
        remediation: "Add a functional index: `CREATE INDEX ON t ((data->>'field'))`. Or for many keys: `CREATE INDEX ON t USING GIN (data jsonb_path_ops)`.",
        matches: matches_json_path_in_where,
    },
    // ── Migration-safety rules (MIG*) ──────────────────────────────
    // Each rule is a one-statement DDL pattern that's known to cause
    // production incidents during deploys (lock waits, table rewrites,
    // app-breakage across rolling restarts). Catalog sources: squawk
    // (GPL — rule names re-used, code clean-room), strong_migrations
    // (MIT — port-friendly), Vlad Mihalcea on JPA migration safety.
    // The matchers below are single-pass over one Statement; no walking,
    // no aggregation, sub-microsecond each.
    SqlRule {
        id: "MIG_CREATE_INDEX_NOT_CONCURRENT",
        severity: Severity::High,
        effort: Effort::Trivial,
        message: "`CREATE INDEX` without `CONCURRENTLY` holds an ACCESS EXCLUSIVE-equivalent SHARE lock on the table — blocks all writes for the duration of the index build",
        remediation: "Use `CREATE INDEX CONCURRENTLY` (Postgres). Note: cannot run inside a transaction; if this file wraps statements in `BEGIN`, split the index creation into its own migration.",
        matches: matches_create_index_not_concurrent,
    },
    SqlRule {
        id: "MIG_DROP_TABLE",
        severity: Severity::High,
        effort: Effort::Medium,
        message: "`DROP TABLE` is irreversible and breaks any deployed code reading the table — orphans data and crashes rolling-restart pods that haven't updated yet",
        remediation: "Stage in two deploys: (1) stop writing to the table, deploy. (2) confirm zero traffic on the table for one retention window, then drop. Consider renaming to `*_deprecated` first so a quick `RENAME` can roll back.",
        matches: matches_drop_table,
    },
    SqlRule {
        id: "MIG_DROP_COLUMN",
        severity: Severity::High,
        effort: Effort::Medium,
        message: "`DROP COLUMN` breaks any deployed code that still references the column — fatal during rolling restarts",
        remediation: "Stage in two deploys: (1) stop reading/writing the column in app code, deploy, verify. (2) drop in a follow-up migration. Use the expand-and-contract pattern (Strong Migrations).",
        matches: matches_drop_column,
    },
    SqlRule {
        id: "MIG_ALTER_COLUMN_TYPE",
        severity: Severity::High,
        effort: Effort::Medium,
        message: "`ALTER COLUMN … TYPE` rewrites the entire table and holds an ACCESS EXCLUSIVE lock — multi-minute outage on large tables",
        remediation: "Use add-new-column + dual-write + backfill + swap (Strong Migrations). For numeric widening on Postgres 9.2+ (e.g. int → bigint of the same family), the rewrite is sometimes avoided — but verify against your version.",
        matches: matches_alter_column_type,
    },
    SqlRule {
        id: "MIG_ADD_FK_NOT_VALID",
        severity: Severity::High,
        effort: Effort::Small,
        message: "`ADD CONSTRAINT … FOREIGN KEY` without `NOT VALID` immediately validates every existing row under an ACCESS EXCLUSIVE lock — minutes-to-hours outage on large tables",
        remediation: "Two-step it: `ALTER TABLE … ADD CONSTRAINT … FOREIGN KEY … NOT VALID;` (fast, takes SHARE ROW EXCLUSIVE), then `ALTER TABLE … VALIDATE CONSTRAINT …;` (no lock).",
        matches: matches_add_fk_not_valid,
    },
    SqlRule {
        id: "MIG_ADD_COLUMN_NOT_NULL_NO_DEFAULT",
        severity: Severity::High,
        effort: Effort::Small,
        message: "`ADD COLUMN … NOT NULL` with no DEFAULT fails immediately on any non-empty table and (on Postgres <11 with a non-volatile DEFAULT) rewrites the table while holding ACCESS EXCLUSIVE",
        remediation: "Three-step: (1) `ADD COLUMN x …` (nullable, no default). (2) Backfill in batches. (3) `ALTER COLUMN x SET NOT NULL`. On Postgres 11+, `ADD COLUMN x … NOT NULL DEFAULT v` is safe (no rewrite) — but the DEFAULT must be a constant.",
        matches: matches_add_column_not_null_no_default,
    },
];

// ────────────────────────────────────────────────────────────────────
// Pattern matchers — each is a pure SRP predicate.
// ────────────────────────────────────────────────────────────────────

/// `SELECT * FROM …` (bare wildcard projection).
///
/// Does NOT fire on `SELECT COUNT(*)` (function call, not Wildcard) or
/// `SELECT t.*` (QualifiedWildcard — narrower, almost always intentional).
fn matches_select_star(stmt: &Statement) -> bool {
    let Statement::Query(query) = stmt else { return false };
    let SetExpr::Select(select) = query.body.as_ref() else { return false };
    select
        .projection
        .iter()
        .any(|item| matches!(item, SelectItem::Wildcard(_)))
}

/// `DELETE FROM t` with no `WHERE`. Wipes the table.
fn matches_delete_no_where(stmt: &Statement) -> bool {
    let Statement::Delete(del) = stmt else { return false };
    del.selection.is_none()
}

/// `UPDATE t SET …` with no `WHERE`. Rewrites every row.
fn matches_update_no_where(stmt: &Statement) -> bool {
    let Statement::Update(upd) = stmt else { return false };
    upd.selection.is_none()
}

/// `INSERT INTO t VALUES (…)` with no column list AND no SELECT source
/// AND no MySQL `SET col = val` assignments.
///
/// `INSERT … SELECT …` carries its own column mapping (the SELECT list)
/// and `INSERT … SET …` (MySQL) names each column inline — neither is
/// the anti-pattern. The anti-pattern is *positional VALUES* binding.
fn matches_insert_no_columns(stmt: &Statement) -> bool {
    let Statement::Insert(ins) = stmt else { return false };
    ins.columns.is_empty()
        && ins.assignments.is_empty()
        && ins.source.is_some()
        && is_values_sourced(ins)
}

fn is_values_sourced(ins: &sqlparser::ast::Insert) -> bool {
    let Some(src) = ins.source.as_deref() else { return false };
    matches!(src.body.as_ref(), SetExpr::Values(_))
}

// ── Matchers for SQL005 onward ──────────────────────────────────────

/// `WHERE col LIKE '%foo'` or `'…%foo…'` — leading wildcard kills
/// B-tree index usage. `'foo%'` (trailing only) is fine.
fn matches_like_leading_wildcard(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        match e {
            Expr::Like { pattern, .. } | Expr::ILike { pattern, .. } => {
                if let Some(text) = literal_string(pattern) {
                    if has_leading_wildcard(text) {
                        found = true;
                    }
                }
            }
            _ => {}
        }
    });
    found
}

/// `ORDER BY RANDOM() / RAND() / NEWID()` — sorts the whole result.
fn matches_order_by_random(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let Some(order_by) = &q.order_by else { return false };
    let OrderByKind::Expressions(exprs) = &order_by.kind else { return false };
    exprs
        .iter()
        .any(|item| expr_calls_random(&item.expr))
}

/// `WHERE a = 1 OR a = 2 OR a = 3` (≥3 ORs against the same column).
/// Recommends `IN (…)`. Distinct from SQL_COMPLEX_OR_CHAIN, which fires
/// on chain length alone regardless of column.
fn matches_or_chain_on_col(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        let cols = collect_or_chain_columns(e);
        // ≥3 same-column equalities chained by OR
        let mut counts: HashMap<String, usize> = HashMap::new();
        for c in cols {
            *counts.entry(c).or_default() += 1;
        }
        if counts.values().any(|&n| n >= 3) {
            found = true;
        }
    });
    found
}

/// `WHERE LOWER(col) = …`, `DATE(col) = …`, `COALESCE(col, 0) > …`.
fn matches_function_in_where(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        if let Expr::BinaryOp { left, right, op } = e {
            if matches!(
                op,
                BinaryOperator::Eq
                    | BinaryOperator::NotEq
                    | BinaryOperator::Gt
                    | BinaryOperator::Lt
                    | BinaryOperator::GtEq
                    | BinaryOperator::LtEq
            ) {
                if is_function_on_identifier(left) || is_function_on_identifier(right) {
                    found = true;
                }
            }
        }
    });
    found
}

/// Cartesian product: ≥2 tables in FROM, no JOINs declared, AND no
/// equality predicate in WHERE relating them. Heuristic — we don't
/// resolve column ownership, so we look for the absence of *any* `=`
/// in the WHERE; if WHERE has an equality, we assume it joins.
fn matches_cartesian_join(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(select) = q.body.as_ref() else { return false };
    if select.from.len() < 2 {
        return false;
    }
    let any_joins = select.from.iter().any(|t| !t.joins.is_empty());
    if any_joins {
        return false;
    }
    // Multiple top-level FROM items with no JOIN AST = comma-join. If
    // the WHERE has no equality predicate at all, treat as cartesian.
    let where_has_eq = select
        .selection
        .as_ref()
        .map(where_contains_equality)
        .unwrap_or(false);
    !where_has_eq
}

/// `WHERE x NOT IN (SELECT …)` — NULL-trap.
fn matches_not_in_with_subquery(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        if let Expr::InSubquery { negated, .. } = e {
            if *negated {
                found = true;
            }
        }
    });
    found
}

/// `SELECT … UNION SELECT …` (no explicit ALL) — implicit DISTINCT.
fn matches_union_distinct_implicit(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    set_expr_has_implicit_union(q.body.as_ref())
}

// (set_expr_has_implicit_union extracted to crate::sql_ast)

// ── Complexity matchers ─────────────────────────────────────────────

/// Total joined tables in the statement ≥ 6.
fn matches_join_count_over_5(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    count_joins_in_query(q) >= 6
}

/// Max subquery nesting depth ≥ 4 (counting outer SELECT as depth 1).
fn matches_subquery_depth_over_3(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    max_subquery_depth_in_query(q, 1) >= 4
}

/// Any single OR chain anywhere in WHERE has ≥7 branches.
fn matches_long_or_chain(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        if or_chain_length(e) >= 7 {
            found = true;
        }
    });
    found
}

// ── IO / shape matchers — each is one compose call into sql_ast.
//    The whole point of extracting sql_ast was to make these 1-3
//    lines of code. If a new matcher grows past 5 lines, the missing
//    abstraction belongs in sql_ast, not here.

/// `WHERE col IN (a, b, c, …)` with at least 100 literals anywhere
/// in the statement.
fn matches_large_in_list(stmt: &Statement) -> bool {
    count_largest_in_list(stmt) >= 100
}

/// `SELECT DISTINCT …` with no `ORDER BY` on the top-level query.
fn matches_distinct_no_order_by(stmt: &Statement) -> bool {
    query_has_distinct(stmt) && !query_has_order_by(stmt)
}

/// `OFFSET N` with N ≥ 1000 — paging beyond this depth on a real
/// table is a linear-scan tax.
fn matches_offset_deep_pagination(stmt: &Statement) -> bool {
    matches!(query_offset_value(stmt), Some(n) if n >= 1000)
}

/// `ROW_NUMBER()/RANK()/LAG()/LEAD()/… OVER (ORDER BY x)` with **no**
/// `PARTITION BY`. On a multi-tenant or scoped table this produces a
/// global rank — almost always a correctness bug AND a sort over the
/// whole table.
///
/// Only fires for ranking/positional window functions; aggregate
/// windows (`SUM() OVER (ORDER BY …)`) legitimately omit
/// `PARTITION BY` for running totals.
fn matches_window_without_partition(stmt: &Statement) -> bool {
    use sqlparser::ast::WindowType;
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    s.projection.iter().any(|item| {
        let expr = match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => e,
            _ => return false,
        };
        let Expr::Function(f) = expr else { return false };
        let Some(WindowType::WindowSpec(spec)) = &f.over else { return false };
        let name = f
            .name
            .0
            .last()
            .map(crate::sql_ast::part_name)
            .unwrap_or("")
            .to_ascii_uppercase();
        let is_rank_or_pos = matches!(
            name.as_str(),
            "ROW_NUMBER"
                | "RANK"
                | "DENSE_RANK"
                | "PERCENT_RANK"
                | "CUME_DIST"
                | "NTILE"
                | "LAG"
                | "LEAD"
                | "FIRST_VALUE"
                | "LAST_VALUE"
                | "NTH_VALUE"
        );
        is_rank_or_pos && spec.partition_by.is_empty() && !spec.order_by.is_empty()
    })
}

/// `SELECT a, b, (SELECT MAX(c) FROM t WHERE t.parent_id = outer.id) AS x
/// FROM outer` — scalar subquery in the projection. The inner runs
/// once per outer row. ORM `withCount` / `annotate(Subquery())` emit
/// this shape. The SQL-side equivalent of the ORM N+1 detector.
fn matches_scalar_subquery_in_projection(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    s.projection.iter().any(|item| match item {
        SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
            matches!(e, Expr::Subquery(_))
        }
        _ => false,
    })
}

// ── Round-3 matchers — each is ONE call into a sql_ast predicate. ──

fn matches_having_no_aggregate(stmt: &Statement) -> bool {
    having_lacks_aggregate(stmt)
}

fn matches_ambiguous_group(stmt: &Statement) -> bool {
    select_has_mixed_aggregation_no_group_by(stmt)
}

fn matches_group_by_ordinal(stmt: &Statement) -> bool {
    group_by_uses_ordinal(stmt)
}

fn matches_order_by_constant(stmt: &Statement) -> bool {
    order_by_is_all_constant(stmt)
}

fn matches_not_in_with_null(stmt: &Statement) -> bool {
    where_has_not_in_with_null(stmt)
}

fn matches_json_path_in_where(stmt: &Statement) -> bool {
    where_uses_json_path(stmt)
}

// ── Migration-safety matchers (MIG_*) ────────────────────────────────
//
// Each matcher is a single AST shape check over one Statement. The
// `use` import bundle stays local to keep the rest of the file's import
// surface lean (these enums are only referenced in this block).

use sqlparser::ast::{
    AlterColumnOperation, AlterTableOperation, ColumnOption, ObjectType, TableConstraint,
};

/// `CREATE INDEX … ON …` with `concurrently == false`.
///
/// In Postgres this acquires an ACCESS-EXCLUSIVE-equivalent share lock
/// on the table for the duration of the build — production outages on
/// large tables. The fix is one keyword (`CONCURRENTLY`) with the small
/// catch that it can't run inside a transaction.
fn matches_create_index_not_concurrent(stmt: &Statement) -> bool {
    matches!(stmt, Statement::CreateIndex(idx) if !idx.concurrently)
}

/// `DROP TABLE …` — any object type filter to Table specifically so
/// `DROP INDEX` / `DROP VIEW` don't fire this rule.
fn matches_drop_table(stmt: &Statement) -> bool {
    matches!(stmt, Statement::Drop { object_type: ObjectType::Table, .. })
}

/// `ALTER TABLE … DROP COLUMN …` — any DropColumn op in the operations
/// list. Fires once per statement regardless of how many columns are
/// dropped (one statement = one finding, sized correctly).
fn matches_drop_column(stmt: &Statement) -> bool {
    let Statement::AlterTable(t) = stmt else { return false };
    t.operations
        .iter()
        .any(|op| matches!(op, AlterTableOperation::DropColumn { .. }))
}

/// `ALTER TABLE … ALTER COLUMN … TYPE …` — only the data-type change is
/// the rewrite hazard. SET NOT NULL / SET DEFAULT are different concerns
/// (covered by their own would-be rules; out of scope for v1).
fn matches_alter_column_type(stmt: &Statement) -> bool {
    let Statement::AlterTable(t) = stmt else { return false };
    t.operations.iter().any(|op| {
        matches!(
            op,
            AlterTableOperation::AlterColumn {
                op: AlterColumnOperation::SetDataType { .. },
                ..
            }
        )
    })
}

/// `ALTER TABLE … ADD CONSTRAINT … FOREIGN KEY …` *without* `NOT VALID`.
/// Without `NOT VALID`, the constraint is validated against every existing
/// row immediately under an ACCESS EXCLUSIVE lock.
fn matches_add_fk_not_valid(stmt: &Statement) -> bool {
    let Statement::AlterTable(t) = stmt else { return false };
    t.operations.iter().any(|op| {
        matches!(
            op,
            AlterTableOperation::AddConstraint {
                constraint: TableConstraint::ForeignKey(_),
                not_valid: false,
            }
        )
    })
}

/// `ALTER TABLE … ADD COLUMN … NOT NULL` with NO `DEFAULT` clause.
/// The column-options vec must contain a `NotNull` option and must NOT
/// contain a `Default(_)` option for this to fire. This is the precise
/// hazard pattern — `ADD COLUMN x INT NOT NULL DEFAULT 0` is safe on
/// Postgres 11+, so we deliberately don't fire on it.
fn matches_add_column_not_null_no_default(stmt: &Statement) -> bool {
    let Statement::AlterTable(t) = stmt else { return false };
    t.operations.iter().any(|op| {
        let AlterTableOperation::AddColumn { column_def, .. } = op else { return false };
        let opts: Vec<&ColumnOption> = column_def.options.iter().map(|o| &o.option).collect();
        let has_not_null = opts.iter().any(|o| matches!(o, ColumnOption::NotNull));
        let has_default = opts.iter().any(|o| matches!(o, ColumnOption::Default(_)));
        has_not_null && !has_default
    })
}

// All generic AST predicates moved to crate::sql_ast — see imports above.
// New rules compose those predicates rather than redefining walkers here.

// ────────────────────────────────────────────────────────────────────
// Dispatcher — has no knowledge of which rules exist. Closed for
// modification: new rules go in BUILTIN_RULES, not here.
// ────────────────────────────────────────────────────────────────────

/// Apply an arbitrary rule slice to one parsed statement — the
/// **Dependency-Inversion seam**. Downstream code (tests, future
/// modules, third-party rule packs) calls this with whatever
/// `&'static [SqlRule]` it composed; the dispatcher stays oblivious
/// to which rules exist.
pub fn check_with_rules<'a>(
    stmt: &Statement,
    rules: &'a [SqlRule],
) -> Vec<&'a SqlRule> {
    rules.iter().filter(|r| (r.matches)(stmt)).collect()
}

// ────────────────────────────────────────────────────────────────────
// Public entrypoint — called from Report::build
// ────────────────────────────────────────────────────────────────────

/// Run drift's built-in SQL anti-pattern catalog over every captured
/// `ExternalCall.sql_literal` and attach findings. Equivalent to
/// `attach_sql_antipatterns_with(entries, BUILTIN_RULES)`.
pub fn attach_sql_antipatterns(entries: &mut [CallTreeNode]) {
    attach_sql_antipatterns_with(entries, BUILTIN_RULES);
}

/// Generic variant of [`attach_sql_antipatterns`] — runs a
/// caller-supplied rule slice. Lets tests and third-party rule packs
/// compose `[BUILTIN_RULES, &EXTRA_RULES].concat()` without forking
/// the dispatcher. The slice must be `'static` because the per-node
/// finding cache stores `&'static SqlRule` references.
pub fn attach_sql_antipatterns_with(
    entries: &mut [CallTreeNode],
    rules: &'static [SqlRule],
) {
    let cache = build_rule_cache(entries, rules);
    for e in entries.iter_mut() {
        attach_walk(e, &cache);
    }
}

/// Build the (fingerprint → matching rules) cache by visiting every
/// SQL literal under the entry trees. Each unique SQL is parsed once.
fn build_rule_cache(
    entries: &[CallTreeNode],
    rules: &'static [SqlRule],
) -> HashMap<u64, Vec<&'static SqlRule>> {
    let mut cache: HashMap<u64, Vec<&'static SqlRule>> = HashMap::new();
    for_each_sql_literal(entries, |sql| {
        let fp = fingerprint(sql);
        cache
            .entry(fp)
            .or_insert_with(|| parse_and_match(sql, rules));
    });
    cache
}

fn for_each_sql_literal(entries: &[CallTreeNode], mut visit: impl FnMut(&str)) {
    fn walk(node: &CallTreeNode, visit: &mut dyn FnMut(&str)) {
        for ec in &node.external_calls {
            if let Some(sql) = ec.sql_literal.as_deref() {
                visit(sql);
            }
        }
        for c in &node.children {
            walk(c, visit);
        }
    }
    for e in entries {
        walk(e, &mut visit);
    }
}

fn attach_walk(node: &mut CallTreeNode, cache: &HashMap<u64, Vec<&'static SqlRule>>) {
    // Multi-call-site dedup per node: if the same node has 4 sites
    // calling the same SQL, fire the rule ONCE per node, anchored at
    // the first call-site's line. Otherwise the Insights tab fills
    // with 4 identical rows for one symbol.
    let mut emitted_here: HashSet<(&'static str, u64)> = HashSet::new();
    // Clone first to avoid borrow conflict between the read of
    // `external_calls` and the mutation of `findings`. The clone is
    // cheap — externals are small structs.
    let externals = node.external_calls.clone();
    for ec in externals.iter() {
        let Some(sql) = ec.sql_literal.as_deref() else { continue };
        let fp = fingerprint(sql);
        let Some(rules) = cache.get(&fp) else { continue };
        for rule in rules {
            if emitted_here.insert((rule.id, fp)) {
                node.findings.push(rule.to_finding(ec.line, sql));
            }
        }
    }
    for c in node.children.iter_mut() {
        attach_walk(c, cache);
    }
}

/// Parse a SQL string with the generic dialect and return matching
/// rules across all parsed statements. On parse error: empty
/// (silent-skip per plan §8 false-positive policy).
///
/// Takes the rule slice as a parameter so the same parsing path
/// serves `BUILTIN_RULES` *and* downstream-composed catalogs (DIP).
fn parse_and_match(sql: &str, rules: &'static [SqlRule]) -> Vec<&'static SqlRule> {
    let dialect = GenericDialect {};
    let Ok(statements) = SqlParser::parse_sql(&dialect, sql) else {
        return Vec::new();
    };
    let mut hits: Vec<&'static SqlRule> = Vec::new();
    for stmt in &statements {
        for rule in check_with_rules(stmt, rules) {
            if !hits.iter().any(|r| r.id == rule.id) {
                hits.push(rule);
            }
        }
    }
    hits
}

// ────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────

/// Stable fingerprint for dedup. `DefaultHasher` is in std; the call
/// site is cold (once per unique SQL).
fn fingerprint(sql: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let normalized: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut h = DefaultHasher::new();
    normalized.hash(&mut h);
    h.finish()
}

fn truncate_sql(s: &str, max: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.len() <= max {
        one_line
    } else {
        let mut out: String = one_line.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ════════════════════════════════════════════════════════════════════
// .sql file scanner — first-class supplementary input (plan §3.2)
// ════════════════════════════════════════════════════════════════════
//
// Three additions slot in beneath the existing embedded-SQL pipeline
// without touching it:
//
//   * `SqlDialect` + `infer_dialect_for_path` — pick the right
//     sqlparser dialect from path/sibling-config hints.
//   * Sanitizers — `is_dbt_template`, `strip_psql_meta_commands`,
//     `strip_liquibase_directives` — silent-skip / strip content that
//     would otherwise blow up the parser (plan §8 FP policy).
//   * `scan_sql_file` + `attach_sql_file_findings` — orchestrator.
//     Reuses BUILTIN_RULES and `check_with_rules` verbatim — the rule
//     dispatcher is dialect-agnostic.
//
// Each function below holds to SRP: one verb per function, pure where
// possible, no I/O at the predicate level.
// ────────────────────────────────────────────────────────────────────

use crate::categories::Category;
use crate::graph::SymbolId;
use crate::SymbolKind;
use sqlparser::dialect::{
    BigQueryDialect, Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
    SnowflakeDialect,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// ────────────────────────────────────────────────────────────────────
// Dialect — small closed-set enum + heuristic inference
// ────────────────────────────────────────────────────────────────────

/// SQL dialect drift can drive the parser with. Mirrors the
/// sqlparser-rs dialect modules but is our own enum so the public API
/// doesn't leak the dependency's type system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlDialect {
    Postgres,
    MySql,
    Sqlite,
    MsSql,
    Snowflake,
    BigQuery,
    Generic,
}

impl SqlDialect {
    /// Parse the user-facing CLI / config name. Case-insensitive.
    /// Returns `None` for unknown values so the caller can decide
    /// whether to default or error.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Self::Postgres,
            "mysql" | "mariadb" => Self::MySql,
            "sqlite" | "sqlite3" => Self::Sqlite,
            "mssql" | "sqlserver" | "tsql" => Self::MsSql,
            "snowflake" => Self::Snowflake,
            "bigquery" | "googlesql" => Self::BigQuery,
            "generic" | "ansi" => Self::Generic,
            _ => return None,
        })
    }

    /// Bridge to sqlparser-rs's dialect trait objects. Each arm is one
    /// owning `Box<dyn Dialect>` so the parser can be invoked
    /// uniformly regardless of which dialect we picked.
    fn as_sqlparser(self) -> Box<dyn Dialect> {
        match self {
            Self::Postgres => Box::new(PostgreSqlDialect {}),
            Self::MySql => Box::new(MySqlDialect {}),
            Self::Sqlite => Box::new(SQLiteDialect {}),
            Self::MsSql => Box::new(MsSqlDialect {}),
            Self::Snowflake => Box::new(SnowflakeDialect {}),
            Self::BigQuery => Box::new(BigQueryDialect {}),
            Self::Generic => Box::new(GenericDialect {}),
        }
    }
}

/// Infer the SQL dialect for a `.sql` file using cheap path + sibling-
/// config signals. Falls back to [`SqlDialect::Generic`] — the v1
/// rules (SELECT *, DELETE/UPDATE no WHERE, INSERT no cols) all work
/// on Generic, so an unidentifiable file still gets the rule pack.
///
/// Inference order (most-specific first):
///
///   1. **Prisma**: file under `prisma/migrations/**` — read sibling
///      `prisma/schema.prisma` for `provider = "..."`.
///   2. **Path keyword**: path segment matches `postgres`/`mysql`/
///      `sqlite` literally (e.g. `db/postgres/migrations/`).
///   3. **Driver in nearest manifest**: walk up from the file looking
///      for `Cargo.toml`, `package.json`, `pyproject.toml`,
///      `requirements.txt`, `go.mod` and sniff for a known driver name.
///   4. **DATABASE_URL in nearest `.env`**: read the scheme prefix.
///
/// All checks are bounded — walk-up stops at `root` (or filesystem
/// root if `root=None`). I/O failures are silent (best-effort
/// inference, never panic).
pub fn infer_dialect_for_path(path: &Path, root: Option<&Path>) -> SqlDialect {
    if let Some(d) = infer_from_prisma_schema(path, root) {
        return d;
    }
    if let Some(d) = infer_from_path_keyword(path) {
        return d;
    }
    if let Some(d) = infer_from_nearest_manifest(path, root) {
        return d;
    }
    if let Some(d) = infer_from_env_files(path, root) {
        return d;
    }
    SqlDialect::Generic
}

fn infer_from_prisma_schema(path: &Path, root: Option<&Path>) -> Option<SqlDialect> {
    // Look for `prisma/schema.prisma` at any ancestor of `path`.
    let schema = walk_up_for_file(path, root, "schema.prisma")?;
    let content = fs::read_to_string(&schema).ok()?;
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("provider") {
            // e.g. `provider = "postgresql"`. Extract first quoted value.
            let quoted = rest.find('"').and_then(|q| {
                let after = &rest[q + 1..];
                after.find('"').map(|e| &after[..e])
            })?;
            return SqlDialect::parse(quoted);
        }
    }
    None
}

fn infer_from_path_keyword(path: &Path) -> Option<SqlDialect> {
    let s = path.to_string_lossy().to_ascii_lowercase();
    // Word-bounded checks: the substring must be set off by a path
    // separator or another non-alpha. Avoids false hits on
    // "mongo-postgres-bridge" style names.
    let candidates = [
        ("/postgres", SqlDialect::Postgres),
        ("/postgresql", SqlDialect::Postgres),
        ("/mysql", SqlDialect::MySql),
        ("/sqlite", SqlDialect::Sqlite),
        ("/mssql", SqlDialect::MsSql),
        ("/snowflake", SqlDialect::Snowflake),
        ("/bigquery", SqlDialect::BigQuery),
    ];
    for (needle, dialect) in candidates {
        if s.contains(needle) {
            return Some(dialect);
        }
    }
    None
}

fn infer_from_nearest_manifest(path: &Path, root: Option<&Path>) -> Option<SqlDialect> {
    // Each manifest type has a small known-driver table mapping driver
    // name → dialect. First manifest found wins.
    const MANIFESTS: &[(&str, &[(&str, SqlDialect)])] = &[
        (
            "Cargo.toml",
            &[
                ("postgres", SqlDialect::Postgres),
                ("tokio-postgres", SqlDialect::Postgres),
                ("sqlx-postgres", SqlDialect::Postgres),
                ("pgx", SqlDialect::Postgres),
                ("mysql_async", SqlDialect::MySql),
                ("mysql", SqlDialect::MySql),
                ("sqlx-mysql", SqlDialect::MySql),
                ("rusqlite", SqlDialect::Sqlite),
                ("sqlx-sqlite", SqlDialect::Sqlite),
            ],
        ),
        (
            "package.json",
            &[
                ("\"pg\"", SqlDialect::Postgres),
                ("\"postgres\"", SqlDialect::Postgres),
                ("\"mysql2\"", SqlDialect::MySql),
                ("\"mysql\"", SqlDialect::MySql),
                ("\"better-sqlite3\"", SqlDialect::Sqlite),
                ("\"sqlite3\"", SqlDialect::Sqlite),
                ("\"mssql\"", SqlDialect::MsSql),
                ("\"@google-cloud/bigquery\"", SqlDialect::BigQuery),
                ("\"snowflake-sdk\"", SqlDialect::Snowflake),
            ],
        ),
        (
            "pyproject.toml",
            &[
                ("psycopg", SqlDialect::Postgres),
                ("asyncpg", SqlDialect::Postgres),
                ("pymysql", SqlDialect::MySql),
                ("aiomysql", SqlDialect::MySql),
                ("mysqlclient", SqlDialect::MySql),
                ("aiosqlite", SqlDialect::Sqlite),
                ("pyodbc", SqlDialect::MsSql),
                ("snowflake-connector-python", SqlDialect::Snowflake),
                ("google-cloud-bigquery", SqlDialect::BigQuery),
            ],
        ),
        (
            "go.mod",
            &[
                ("jackc/pgx", SqlDialect::Postgres),
                ("lib/pq", SqlDialect::Postgres),
                ("go-sql-driver/mysql", SqlDialect::MySql),
                ("mattn/go-sqlite3", SqlDialect::Sqlite),
                ("denisenkom/go-mssqldb", SqlDialect::MsSql),
                ("snowflakedb/gosnowflake", SqlDialect::Snowflake),
            ],
        ),
    ];
    for (fname, drivers) in MANIFESTS {
        let manifest = walk_up_for_file(path, root, fname);
        let Some(m) = manifest else { continue };
        let Ok(content) = fs::read_to_string(&m) else { continue };
        for (needle, dialect) in *drivers {
            if content.contains(needle) {
                return Some(*dialect);
            }
        }
    }
    None
}

fn infer_from_env_files(path: &Path, root: Option<&Path>) -> Option<SqlDialect> {
    // Walk up looking for any `.env` / `.env.local` / `.env.example`
    // and grep for a DATABASE_URL line. First scheme that parses wins.
    let env_names = [".env", ".env.local", ".env.example", ".env.development"];
    for name in env_names {
        let Some(env) = walk_up_for_file(path, root, name) else { continue };
        let Ok(content) = fs::read_to_string(&env) else { continue };
        for line in content.lines() {
            let t = line.trim();
            if let Some(value) = t.strip_prefix("DATABASE_URL=") {
                let scheme = value.trim_matches('"').split("://").next()?;
                if let Some(d) = SqlDialect::parse(scheme) {
                    return Some(d);
                }
                // Some env files use `postgresql://...` — parse() handles that
                // alias. `mysql://`, `sqlite://` similarly. `mssql://` not standard
                // but tolerated by parse().
            }
        }
    }
    None
}

/// Walk from `path`'s parent up to `root` (exclusive of root's parent)
/// looking for a file named `target`. Returns the first hit.
fn walk_up_for_file(path: &Path, root: Option<&Path>, target: &str) -> Option<PathBuf> {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        let candidate = dir.join(target);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Stop at root if we have one — don't keep walking up past it.
        if let Some(r) = root {
            if dir == r {
                break;
            }
        }
        cur = dir.parent();
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Content sanitizers — silent-skip / strip non-SQL content
// ────────────────────────────────────────────────────────────────────

/// dbt template files contain Jinja substitutions (`{{ ref('users') }}`,
/// `{% if … %}`) that aren't valid SQL until rendered. We don't render
/// — we skip the file with no findings and let the user re-scan the
/// compiled `target/` output.
pub fn is_dbt_template(content: &str) -> bool {
    // Cheap two-byte sniff. Real dbt files always have one of these.
    content.contains("{{") || content.contains("{%")
}

/// Strip `psql` meta-commands (`\d`, `\timing`, `\copy`, `\i other.sql`,
/// …) without disturbing line numbering — meta lines become blank lines
/// so downstream statement line numbers remain correct.
///
/// Reference: <https://www.postgresql.org/docs/current/app-psql.html>
pub fn strip_psql_meta_commands(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        if line.trim_start().starts_with('\\') {
            // Keep the newline so line counts match. Blank the rest.
            out.push('\n');
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Strip Liquibase formatted-SQL directives (`--changeset author:id`,
/// `--rollback ...`) which are SQL comments syntactically but carry
/// load-bearing semantics for Liquibase and would otherwise be silently
/// ignored. We strip them so the remaining content parses cleanly.
/// Line numbering preserved.
///
/// Reference:
/// <https://docs.liquibase.com/concepts/changelogs/sql-format.html>
pub fn strip_liquibase_directives(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_directive = trimmed.starts_with("--changeset")
            || trimmed.starts_with("--rollback")
            || trimmed.starts_with("--liquibase")
            || trimmed.starts_with("--preconditions")
            || trimmed.starts_with("--validCheckSum");
        if is_directive {
            out.push('\n');
        } else {
            out.push_str(line);
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────
// Statement-position helper — byte offset → 1-based line number
// ────────────────────────────────────────────────────────────────────

/// Build a sorted `Vec<usize>` of byte offsets where each line starts.
/// O(n) once per file. Binary-search against this to map any byte offset
/// to a 1-based line number in O(log n).
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_for_offset(line_starts: &[usize], byte: usize) -> usize {
    // partition_point finds the first index whose value > byte.
    // 1-based: the line containing `byte` is that index (the partition
    // gives us how many line_starts are ≤ byte, which is the 1-based
    // line number).
    line_starts.partition_point(|&s| s <= byte).max(1)
}

// ────────────────────────────────────────────────────────────────────
// Statement splitting + per-statement scanning
// ────────────────────────────────────────────────────────────────────

/// A statement extracted from a `.sql` file with its 1-based start line.
struct LocatedStatement<'a> {
    sql: &'a str,
    line: usize,
}

/// Split `content` into per-statement slices, each carrying its 1-based
/// start line. Uses a small string-aware semicolon splitter — survives
/// semicolons inside string literals (`'a;b'`, `"a;b"`, `` `a;b` ``) and
/// line/block comments (`-- ; …`, `/* ; */`). Dollar-quoted strings
/// (`$$ … $$`, `$tag$ … $tag$`) preserved for PL/pgSQL bodies.
///
/// Conservative: when in doubt about being inside a quote, errs on the
/// side of "not a separator" — at worst we hand a multi-statement chunk
/// to the parser, which is correct behavior anyway.
fn split_statements_with_lines(content: &str) -> Vec<LocatedStatement<'_>> {
    let starts = line_starts(content);
    let bytes = content.as_bytes();
    let mut out: Vec<LocatedStatement<'_>> = Vec::new();
    let mut i: usize = 0;
    let len = bytes.len();
    let mut stmt_start: usize = 0;
    let mut in_single = false; // '
    let mut in_double = false; // "
    let mut in_backtick = false; // `
    let mut in_block_comment = false;
    let mut in_line_comment = false;
    let mut in_dollar: Option<String> = None; // captured tag without dollars

    while i < len {
        let c = bytes[i];

        // ── exit dollar-quoted ────────────────────────────────────
        if let Some(tag) = in_dollar.as_ref() {
            let close = format!("${tag}$");
            if bytes[i..].starts_with(close.as_bytes()) {
                i += close.len();
                in_dollar = None;
                continue;
            }
            i += 1;
            continue;
        }

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_single {
            // Backslash-escape handling kept off: SQL strings escape
            // single quotes by doubling them ('' ), not via backslash.
            if c == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if c == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            if c == b'`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        // ── enter states ───────────────────────────────────────────
        if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            in_block_comment = true;
            i += 2;
            continue;
        }
        if c == b'\'' {
            in_single = true;
            i += 1;
            continue;
        }
        if c == b'"' {
            in_double = true;
            i += 1;
            continue;
        }
        if c == b'`' {
            in_backtick = true;
            i += 1;
            continue;
        }
        if c == b'$' {
            // Dollar-quoted: $tag$ ... $tag$ (tag may be empty). Tag
            // chars are letters/digits/underscore.
            let mut j = i + 1;
            while j < len {
                let cj = bytes[j];
                if cj == b'$' {
                    break;
                }
                if !(cj.is_ascii_alphanumeric() || cj == b'_') {
                    j = i; // bail — not a tag
                    break;
                }
                j += 1;
            }
            if j > i && j < len && bytes[j] == b'$' {
                let tag = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("").to_string();
                in_dollar = Some(tag);
                i = j + 1;
                continue;
            }
        }

        // ── statement terminator ───────────────────────────────────
        if c == b';' {
            let chunk = &content[stmt_start..i];
            if !chunk.trim().is_empty() {
                let line = line_for_offset(&starts, leading_sql_offset(chunk, stmt_start));
                out.push(LocatedStatement { sql: chunk, line });
            }
            stmt_start = i + 1;
        }

        i += 1;
    }

    // Trailing statement without semicolon.
    let tail = &content[stmt_start..];
    if !tail.trim().is_empty() {
        let line = line_for_offset(&starts, leading_sql_offset(tail, stmt_start));
        out.push(LocatedStatement { sql: tail, line });
    }

    out
}

/// Offset (in *file* coordinates) of the first byte that begins the
/// statement's actual SQL — i.e. the first byte that is NOT whitespace,
/// NOT inside a leading `-- line comment`, and NOT inside a leading
/// `/* block comment */`.
///
/// Why this matters: a statement chunk often opens with comment lines
/// (`-- Flyway header`, `-- changeset alice:1`) on physical lines well
/// above the actual SQL keyword. Reporting findings on the comment line
/// would be misleading — the reader expects the line of the `SELECT`,
/// not the line of the human prose above it.
fn leading_sql_offset(chunk: &str, base: usize) -> usize {
    let bytes = chunk.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comment: skip to next \n.
        if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment: skip to matching `*/`. Conservative — if the
        // close isn't found, treat the rest of the chunk as comment.
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        return base + i;
    }
    base
}

// ────────────────────────────────────────────────────────────────────
// scan_sql_file — orchestrator for ONE file
// ────────────────────────────────────────────────────────────────────

/// Outcome of scanning one `.sql` file.
///
/// `findings.is_empty()` AND `skipped_reason.is_some()` means the file
/// was deliberately not analyzed (dbt template, parse failure across
/// the whole file). The caller can choose to surface that informationally
/// or drop it.
#[derive(Debug, Default)]
pub struct SqlFileScan {
    pub findings: Vec<Finding>,
    pub skipped_reason: Option<&'static str>,
}

/// Run the SQL rule catalog against one file's content. Pure function:
/// no I/O, no globals beyond the const rule catalog. Caller reads the
/// file and decides what to do with the result.
///
/// Dialect picks the sqlparser front-end; rule shapes are dialect-agnostic.
pub fn scan_sql_file(content: &str, dialect: SqlDialect) -> SqlFileScan {
    scan_sql_file_with(content, dialect, BUILTIN_RULES)
}

/// Generic variant taking a caller-supplied rule slice (Dependency-
/// Inversion seam; matches `attach_sql_antipatterns_with`).
pub fn scan_sql_file_with(
    content: &str,
    dialect: SqlDialect,
    rules: &'static [SqlRule],
) -> SqlFileScan {
    if is_dbt_template(content) {
        return SqlFileScan {
            findings: Vec::new(),
            skipped_reason: Some("dbt template — run `dbt compile` and re-scan target/"),
        };
    }
    let cleaned = strip_liquibase_directives(&strip_psql_meta_commands(content));
    let parser_dialect = dialect.as_sqlparser();
    let mut findings: Vec<Finding> = Vec::new();
    let mut any_parsed = false;
    for stmt in split_statements_with_lines(&cleaned) {
        // Per-statement parse: tolerate one broken statement without
        // dropping the rest of the file (plan §8 false-positive
        // policy — never crash, silent-skip on uncertainty).
        let Ok(parsed) = SqlParser::parse_sql(parser_dialect.as_ref(), stmt.sql) else {
            continue;
        };
        any_parsed = true;
        for s in &parsed {
            for rule in check_with_rules(s, rules) {
                findings.push(rule.to_finding(stmt.line, stmt.sql));
            }
        }
    }
    SqlFileScan {
        findings,
        skipped_reason: if any_parsed { None } else { Some("could not parse any statement") },
    }
}

// ────────────────────────────────────────────────────────────────────
// Synthetic CallTreeNode builder — make a `.sql` file look like an entry
// ────────────────────────────────────────────────────────────────────

/// Build a leaf `CallTreeNode` representing a single `.sql` file so the
/// existing viewer / Insights tab / summary roll-up infrastructure can
/// render its findings without any new schema or rendering code.
///
/// All graph metrics are zero / empty — these nodes are not on the call
/// graph by design. `kind = Function` because the existing `SymbolKind`
/// has no `SqlFile` variant and adding one would force cascade changes;
/// the viewer treats every entry's kind uniformly, so this is harmless.
fn make_sql_file_node(path: &Path, root: Option<&Path>, findings: Vec<Finding>) -> CallTreeNode {
    let rel = match root {
        Some(r) => path.strip_prefix(r).unwrap_or(path).to_path_buf(),
        None => path.to_path_buf(),
    };
    let file_str = rel.display().to_string();
    let name = rel
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_str.clone());
    let id = SymbolId(format!("{file_str}::<sql_file>::{name}"));
    CallTreeNode {
        id,
        name,
        kind: SymbolKind::Function,
        file: file_str,
        line: 1,
        depth: 0,
        parent_class: None,
        children: Vec::new(),
        truncated_reason: None,
        callers: Vec::new(),
        callers_count: 0,
        callees_count: 0,
        subtree_size: 1,
        category_self: Some(Category::Db),
        categories_reached: BTreeMap::new(),
        external_calls: Vec::new(),
        complexity: 0,
        loc: 0,
        nesting_depth: 0,
        parameter_count: 0,
        is_async: false,
        call_site_count: 0,
        is_recursive: false,
        pagerank: 0.0,
        percent_total: 0.0,
        percent_parent: 0.0,
        n_plus_one_risk: false,
        blocking_in_async: false,
        findings,
        // Discriminator the viewer can use to render `.sql` synthetic
        // nodes distinctly (e.g. a "SQL files" group in the entries
        // list) without us forcing a new schema field. Mirrors the
        // existing `docker.rs::label_call_tree_entries` convention.
        entry_labels: vec!["sql:file".to_string()],
    }
}

// ────────────────────────────────────────────────────────────────────
// attach_sql_file_findings — public orchestrator called from Report::build
// ────────────────────────────────────────────────────────────────────

/// Options for the `.sql`-file pass. Wraps the small set of choices the
/// orchestrator needs without forcing them through Report::build's
/// already-long signature one-by-one.
#[derive(Debug, Clone, Default)]
pub struct SqlFileOpts {
    /// Explicit dialect override (CLI `--sql-dialect <name>`). When
    /// `None`, drift uses [`infer_dialect_for_path`].
    pub dialect_override: Option<SqlDialect>,
}

/// Walk every `.sql` file under `root`, scan it, and append a synthetic
/// `CallTreeNode` per file with findings to `entries`.
///
/// **No-op when `root` is `None`** — that's the case for library callers
/// that built their `entries` manually and don't have a project root to
/// walk. Same fall-through pattern as `attach_recursive_findings` etc.
///
/// **Idempotent under repeated calls**: the synthetic-node ids are
/// derived from the file path, so a second invocation would push
/// duplicates — callers must call it once per Report::build, which is
/// what the existing pipeline guarantees.
pub fn attach_sql_file_findings(
    entries: &mut Vec<CallTreeNode>,
    root: &Path,
    opts: &SqlFileOpts,
) {
    let _ = scan_sql_files_into(entries, root, opts);
}

/// Build the Info-level marker finding attached to synthetic nodes
/// for files drift saw but couldn't parse a single statement of
/// (typically PL/pgSQL `DO $$…$$` anonymous blocks or other
/// dialect-specific constructs sqlparser-rs doesn't yet handle).
///
/// Distinct from a real antipattern finding via:
///   - `severity: Severity::Low` (informational, not an action item)
///   - `confidence: 1.0` (we KNOW we couldn't parse — no uncertainty)
///   - `effort: Effort::Medium` (workaround: switch to libpg_query or
///     manually review)
///   - `evidence[0].call = "SQL_UNPARSEABLE"` (rule-id chip the viewer
///     renders + the dedup key)
fn unparseable_marker(reason: &'static str) -> Finding {
    Finding {
        kind: FindingKind::SqlAntipattern,
        severity: Severity::Low,
        effort: Effort::Medium,
        confidence: 1.0,
        line: 1,
        message: format!("Drift couldn't analyze this `.sql` file: {reason}. Review manually."),
        evidence: vec![Evidence {
            call: "SQL_UNPARSEABLE".to_string(),
            line: 1,
            category: None,
        }],
        remediation: Some(
            "Drift's sqlparser-rs backend doesn't cover every dialect construct \
             (PL/pgSQL `DO $$…$$`, dialect-specific syntax). The file appears in \
             the report so you know it WAS scanned; review the SQL by hand or wait \
             for the planned libpg_query backend (plan §5.3)."
                .to_string(),
        ),
    }
}

/// Same as [`attach_sql_file_findings`] but returns the per-call
/// scan stats so the orchestrator can roll them into `Summary`.
///
/// Architectural contract (the "trust invariant" of any analyzer —
/// `pprof`, `SonarQube`, Chrome DevTools, Lighthouse, `cargo check`
/// all uphold it): **every analyzed unit appears in the report**.
/// Findings are LAYERED on top as severity markers; absence of
/// findings is itself information ("scanned, clean").
///
/// Concretely for `.sql` files:
///
///   - File parses successfully (any subset of statements) → emit a
///     synthetic `CallTreeNode`. `node.findings` may be empty. The
///     node is tagged `entry_labels: ["sql:file"]` so the viewer can
///     render it distinctly without a new schema field — same
///     mechanism `docker.rs` uses for container-entry rollups.
///   - File can't be parsed at all (dbt template, total garbage)
///     → silent skip (false-positive policy: refuse to fire when
///     uncertain — see plan §8).
///
/// Returns `(scanned, with_findings)` for the summary roll-up.
pub fn scan_sql_files_into(
    entries: &mut Vec<CallTreeNode>,
    root: &Path,
    opts: &SqlFileOpts,
) -> SqlFileScanStats {
    let files = crate::walker::discover_sql_files(root);
    let mut stats = SqlFileScanStats::default();
    for path in files {
        let Ok(content) = fs::read_to_string(&path) else { continue };
        let dialect = opts
            .dialect_override
            .unwrap_or_else(|| infer_dialect_for_path(&path, Some(root)));
        let scan = scan_sql_file(&content, dialect);
        // Two skip-shapes have different correct behavior. They are
        // distinguished by the `skipped_reason` string contents.
        //
        // a) "dbt template — …" → silent skip. Intentional non-SQL the
        //    user knows we can't analyze. Surfacing every dbt file as
        //    a node would flood real-repo reports with 100s of "drift
        //    skipped this" rows the user can't act on.
        //
        // b) "could not parse any statement" → EMIT a node carrying
        //    a single INFO-level marker finding. The file IS real SQL
        //    drift just couldn't handle (PL/pgSQL `DO $$…$$`, dialect-
        //    specific constructs, etc.). The user needs to know drift
        //    saw it but couldn't analyze, otherwise the trust-contract
        //    breaks — "did drift miss this file or did it find no
        //    issues?" becomes unanswerable.
        let is_dbt = scan
            .skipped_reason
            .map(|r| r.starts_with("dbt template"))
            .unwrap_or(false);
        if is_dbt {
            continue;
        }
        stats.scanned += 1;
        if !scan.findings.is_empty() {
            stats.with_findings += 1;
        }
        let mut findings = scan.findings;
        // If the parser couldn't handle a single statement, attach a
        // single Info-style marker finding so the synthetic node tells
        // the user what happened. We reuse `FindingKind::SqlAntipattern`
        // (no schema churn) and key the marker with the rule id
        // `SQL_UNPARSEABLE` so the dedup pass treats it as a distinct
        // rule and so the viewer can render it differently from real
        // antipattern hits.
        if let Some(reason) = scan.skipped_reason {
            findings.push(unparseable_marker(reason));
        }
        entries.push(make_sql_file_node(&path, Some(root), findings));
    }
    stats
}

/// Per-pass roll-up consumed by `Summary` so users can confirm
/// "drift scanned N `.sql` files, M had problems" at a glance.
#[derive(Debug, Default, Clone, Copy)]
pub struct SqlFileScanStats {
    pub scanned: usize,
    pub with_findings: usize,
}

// ────────────────────────────────────────────────────────────────────
// Tests — exercise each rule + the false-positive guards.
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_for(sql: &str) -> Vec<&'static str> {
        parse_and_match(sql, BUILTIN_RULES)
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    #[test]
    fn sql001_fires_on_bare_select_star() {
        assert_eq!(ids_for("SELECT * FROM users"), vec!["SQL001"]);
    }

    #[test]
    fn sql001_doesnt_fire_on_count_star() {
        // COUNT(*) is a function call whose arg is a wildcard — NOT a
        // SelectItem::Wildcard projection.
        assert_eq!(ids_for("SELECT COUNT(*) FROM users"), Vec::<&str>::new());
    }

    #[test]
    fn sql001_doesnt_fire_on_qualified_wildcard() {
        // `SELECT t.*` is QualifiedWildcard — narrower and almost always
        // intentional (joining + wanting all of one table). Don't flag.
        assert_eq!(
            ids_for("SELECT t.* FROM users t JOIN orders o ON t.id = o.user_id"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn sql002_fires_on_delete_without_where() {
        assert_eq!(ids_for("DELETE FROM users"), vec!["SQL002"]);
    }

    #[test]
    fn sql002_doesnt_fire_with_where() {
        assert_eq!(
            ids_for("DELETE FROM users WHERE id = 1"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn sql003_fires_on_update_without_where() {
        assert_eq!(
            ids_for("UPDATE users SET active = false"),
            vec!["SQL003"]
        );
    }

    #[test]
    fn sql003_doesnt_fire_with_where() {
        assert_eq!(
            ids_for("UPDATE users SET active = false WHERE id = 1"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn sql004_fires_on_values_no_columns() {
        assert_eq!(
            ids_for("INSERT INTO users VALUES (1, 'alice')"),
            vec!["SQL004"]
        );
    }

    #[test]
    fn sql004_doesnt_fire_with_columns() {
        assert_eq!(
            ids_for("INSERT INTO users (id, name) VALUES (1, 'alice')"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn sql004_doesnt_fire_on_insert_from_select() {
        // INSERT ... SELECT carries its own column mapping. SQL004 is
        // about positional VALUES binding, not about SELECT-sourced
        // inserts.
        assert_eq!(
            ids_for("INSERT INTO archive SELECT id, name FROM users"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn unparseable_sql_silently_skips() {
        // Dialect-specific syntax we don't understand → empty hits, not
        // a panic. The category-level detector still surfaces it.
        assert_eq!(ids_for("THIS IS NOT SQL"), Vec::<&str>::new());
    }

    #[test]
    fn multiple_rules_fire_on_one_statement_batch() {
        let sql = "DELETE FROM users; INSERT INTO t VALUES (1)";
        let hits = ids_for(sql);
        assert!(hits.contains(&"SQL002"), "expected SQL002, got {:?}", hits);
        assert!(hits.contains(&"SQL004"), "expected SQL004, got {:?}", hits);
    }

    #[test]
    fn fingerprint_normalizes_whitespace() {
        let a = fingerprint("SELECT id FROM users");
        let b = fingerprint("SELECT  id  FROM  users");
        let c = fingerprint("SELECT\tid\nFROM users");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    // ── SQL005 — leading wildcard LIKE ──────────────────────────────

    #[test]
    fn sql005_fires_on_leading_wildcard_like() {
        assert!(ids_for("SELECT id FROM users WHERE name LIKE '%alice'").contains(&"SQL005"));
    }

    #[test]
    fn sql005_fires_on_both_sides_wildcard() {
        assert!(ids_for("SELECT id FROM users WHERE name LIKE '%alice%'").contains(&"SQL005"));
    }

    #[test]
    fn sql005_doesnt_fire_on_trailing_only_wildcard() {
        // Trailing wildcard ('foo%') is index-friendly — don't flag.
        assert!(!ids_for("SELECT id FROM users WHERE name LIKE 'alice%'").contains(&"SQL005"));
    }

    // ── SQL006 — ORDER BY RANDOM ────────────────────────────────────

    #[test]
    fn sql006_fires_on_order_by_random() {
        assert!(ids_for("SELECT * FROM users ORDER BY RANDOM() LIMIT 1").contains(&"SQL006"));
    }

    #[test]
    fn sql006_fires_on_order_by_rand() {
        // MySQL's RAND() (case-insensitive match).
        assert!(ids_for("SELECT * FROM users ORDER BY RAND() LIMIT 1").contains(&"SQL006"));
    }

    #[test]
    fn sql006_doesnt_fire_on_plain_order_by() {
        assert!(!ids_for("SELECT * FROM users ORDER BY created_at DESC LIMIT 1").contains(&"SQL006"));
    }

    // ── SQL007 — OR chain on same column ────────────────────────────

    #[test]
    fn sql007_fires_on_or_chain_same_col() {
        let hits = ids_for("SELECT * FROM users WHERE id = 1 OR id = 2 OR id = 3");
        assert!(hits.contains(&"SQL007"), "got {:?}", hits);
    }

    #[test]
    fn sql007_doesnt_fire_on_two_branch_or() {
        // Two-branch OR is fine — only flag ≥3 on the same column.
        assert!(!ids_for("SELECT * FROM users WHERE id = 1 OR id = 2").contains(&"SQL007"));
    }

    #[test]
    fn sql007_doesnt_fire_on_or_across_columns() {
        // OR across different columns is legitimate (different predicates).
        assert!(!ids_for("SELECT * FROM users WHERE name = 'a' OR email = 'b' OR phone = 'c'")
            .contains(&"SQL007"));
    }

    // ── SQL009 — function on indexed column ─────────────────────────

    #[test]
    fn sql009_fires_on_lower_col() {
        assert!(ids_for("SELECT * FROM users WHERE LOWER(email) = 'a@b.com'").contains(&"SQL009"));
    }

    #[test]
    fn sql009_fires_on_date_col() {
        assert!(ids_for("SELECT * FROM events WHERE DATE(created_at) = '2024-01-01'")
            .contains(&"SQL009"));
    }

    #[test]
    fn sql009_doesnt_fire_when_function_is_on_literal() {
        // `WHERE col = LOWER('A@B.com')` is fine — the function isn't
        // wrapping the column. Note: GenericDialect may not collapse
        // this perfectly; we just assert no false positive on bare
        // column comparison.
        assert!(!ids_for("SELECT * FROM users WHERE email = 'a@b.com'").contains(&"SQL009"));
    }

    // ── SQL010 — Cartesian product ──────────────────────────────────

    #[test]
    fn sql010_fires_on_comma_join_no_predicate() {
        // FROM a, b with no WHERE = accidental cross-product.
        let hits = ids_for("SELECT * FROM users, orders");
        assert!(hits.contains(&"SQL010"), "got {:?}", hits);
    }

    #[test]
    fn sql010_doesnt_fire_on_explicit_join() {
        assert!(!ids_for("SELECT * FROM users u JOIN orders o ON u.id = o.user_id")
            .contains(&"SQL010"));
    }

    #[test]
    fn sql010_doesnt_fire_when_where_has_equality() {
        // Old-school comma-join with the equality in WHERE — strictly
        // bad style but not Cartesian (planner can use it). Our
        // heuristic accepts it.
        assert!(!ids_for("SELECT * FROM users u, orders o WHERE u.id = o.user_id")
            .contains(&"SQL010"));
    }

    // ── SQL011 — NOT IN subquery ────────────────────────────────────

    #[test]
    fn sql011_fires_on_not_in_subquery() {
        assert!(ids_for(
            "SELECT * FROM users WHERE id NOT IN (SELECT user_id FROM bans)"
        )
        .contains(&"SQL011"));
    }

    #[test]
    fn sql011_doesnt_fire_on_plain_in() {
        assert!(!ids_for(
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM friends)"
        )
        .contains(&"SQL011"));
    }

    #[test]
    fn sql011_doesnt_fire_on_not_in_list() {
        // `NOT IN (1, 2, 3)` is fine — only the SELECT-subquery form
        // has the NULL trap.
        assert!(!ids_for("SELECT * FROM users WHERE id NOT IN (1, 2, 3)").contains(&"SQL011"));
    }

    // ── SQL015 — implicit UNION distinct ────────────────────────────

    #[test]
    fn sql015_fires_on_bare_union() {
        let hits = ids_for("SELECT id FROM a UNION SELECT id FROM b");
        assert!(hits.contains(&"SQL015"), "got {:?}", hits);
    }

    #[test]
    fn sql015_doesnt_fire_on_union_all() {
        assert!(!ids_for("SELECT id FROM a UNION ALL SELECT id FROM b").contains(&"SQL015"));
    }

    // ── Complexity rules ────────────────────────────────────────────

    #[test]
    fn complex_joins_fires_at_6_joins() {
        // 6 tables = 5 joins (count_joins counts each Join node) + 1
        // base => 6 joined tables. Our matcher fires at ≥6 joined
        // tables.
        let sql = "
            SELECT * FROM a
            JOIN b ON a.id = b.a_id
            JOIN c ON b.id = c.b_id
            JOIN d ON c.id = d.c_id
            JOIN e ON d.id = e.d_id
            JOIN f ON e.id = f.e_id
        ";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL_COMPLEX_JOINS"),
            "expected SQL_COMPLEX_JOINS, got {:?}",
            hits
        );
    }

    #[test]
    fn complex_joins_doesnt_fire_at_3_joins() {
        let sql = "SELECT * FROM a JOIN b ON a.id = b.a_id JOIN c ON b.id = c.b_id";
        assert!(!ids_for(sql).contains(&"SQL_COMPLEX_JOINS"));
    }

    #[test]
    fn complex_subquery_depth_fires_at_4() {
        // Outer + 3 nested subqueries = depth 4.
        let sql = "SELECT * FROM (SELECT * FROM (SELECT * FROM (SELECT * FROM t) a) b) c";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL_COMPLEX_SUBQUERY_DEPTH"),
            "got {:?}",
            hits
        );
    }

    #[test]
    fn complex_subquery_depth_doesnt_fire_at_2() {
        let sql = "SELECT * FROM (SELECT * FROM t) a";
        assert!(!ids_for(sql).contains(&"SQL_COMPLEX_SUBQUERY_DEPTH"));
    }

    #[test]
    fn complex_or_chain_fires_at_7_branches() {
        let sql = "SELECT * FROM t WHERE a = 1 OR b = 2 OR c = 3 OR d = 4 OR e = 5 OR f = 6 OR g = 7";
        let hits = ids_for(sql);
        assert!(hits.contains(&"SQL_COMPLEX_OR_CHAIN"), "got {:?}", hits);
    }

    #[test]
    fn complex_or_chain_doesnt_fire_at_3_branches() {
        let sql = "SELECT * FROM t WHERE a = 1 OR b = 2 OR c = 3";
        assert!(!ids_for(sql).contains(&"SQL_COMPLEX_OR_CHAIN"));
    }

    // ── SQL_WIN_NO_PARTITION ────────────────────────────────────────

    #[test]
    fn win_no_partition_fires_on_row_number_order_by_only() {
        let sql = "SELECT ROW_NUMBER() OVER (ORDER BY created_at) AS rn FROM events";
        let hits = ids_for(sql);
        assert!(hits.contains(&"SQL_WIN_NO_PARTITION"), "got {:?}", hits);
    }

    #[test]
    fn win_no_partition_doesnt_fire_with_partition_by() {
        let sql = "SELECT ROW_NUMBER() OVER (PARTITION BY tenant_id ORDER BY created_at) AS rn FROM events";
        assert!(!ids_for(sql).contains(&"SQL_WIN_NO_PARTITION"));
    }

    #[test]
    fn win_no_partition_doesnt_fire_on_running_aggregate() {
        // SUM() OVER (ORDER BY x) for running totals legitimately omits
        // PARTITION BY. We only flag ranking/positional functions.
        let sql = "SELECT SUM(amount) OVER (ORDER BY created_at) AS running FROM events";
        assert!(!ids_for(sql).contains(&"SQL_WIN_NO_PARTITION"));
    }

    #[test]
    fn win_no_partition_fires_on_lag() {
        let sql = "SELECT LAG(price) OVER (ORDER BY ts) AS prev_price FROM ticks";
        assert!(ids_for(sql).contains(&"SQL_WIN_NO_PARTITION"));
    }

    // ── SQL_SUB_SCALAR_IN_PROJECTION ────────────────────────────────

    #[test]
    fn scalar_subquery_in_projection_fires() {
        let sql = "SELECT u.id, (SELECT MAX(o.created_at) FROM orders o WHERE o.user_id = u.id) AS last_order FROM users u";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL_SUB_SCALAR_IN_PROJECTION"),
            "got {:?}",
            hits
        );
    }

    #[test]
    fn scalar_subquery_doesnt_fire_when_subquery_is_in_where() {
        // Subquery in WHERE is the more common form and not the
        // per-row pathology this rule targets.
        let sql = "SELECT u.id FROM users u WHERE u.id IN (SELECT user_id FROM orders)";
        assert!(!ids_for(sql).contains(&"SQL_SUB_SCALAR_IN_PROJECTION"));
    }

    #[test]
    fn scalar_subquery_doesnt_fire_on_plain_join() {
        let sql = "SELECT u.id, MAX(o.created_at) FROM users u LEFT JOIN orders o ON o.user_id = u.id GROUP BY u.id";
        assert!(!ids_for(sql).contains(&"SQL_SUB_SCALAR_IN_PROJECTION"));
    }

    // ── SQL014 — HAVING without aggregate ───────────────────────────

    #[test]
    fn sql014_fires_on_having_without_aggregate() {
        // HAVING references only a plain column — should be in WHERE.
        let sql = "SELECT id FROM orders GROUP BY id HAVING id > 100";
        assert!(ids_for(sql).contains(&"SQL014_HAVING_NO_AGGREGATE"));
    }

    #[test]
    fn sql014_doesnt_fire_when_having_uses_aggregate() {
        let sql = "SELECT user_id, COUNT(*) FROM orders GROUP BY user_id HAVING COUNT(*) > 10";
        assert!(!ids_for(sql).contains(&"SQL014_HAVING_NO_AGGREGATE"));
    }

    // ── SQL017 — ambiguous group ────────────────────────────────────

    #[test]
    fn sql017_fires_on_aggregate_and_plain_without_group_by() {
        let sql = "SELECT name, COUNT(*) FROM users";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL017_AMBIGUOUS_GROUP"),
            "expected SQL017, got {hits:?}"
        );
    }

    #[test]
    fn sql017_doesnt_fire_with_group_by() {
        let sql = "SELECT name, COUNT(*) FROM users GROUP BY name";
        assert!(!ids_for(sql).contains(&"SQL017_AMBIGUOUS_GROUP"));
    }

    #[test]
    fn sql017_doesnt_fire_on_pure_aggregate() {
        // SELECT COUNT(*) FROM t is a single scalar — no ambiguity.
        let sql = "SELECT COUNT(*) FROM users";
        assert!(!ids_for(sql).contains(&"SQL017_AMBIGUOUS_GROUP"));
    }

    // ── SQL021 — GROUP BY ordinal ──────────────────────────────────

    #[test]
    fn sql021_fires_on_group_by_ordinal() {
        let sql = "SELECT name, COUNT(*) FROM users GROUP BY 1";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL021_GROUP_BY_ORDINAL"),
            "expected SQL021, got {hits:?}"
        );
    }

    #[test]
    fn sql021_doesnt_fire_on_named_group_by() {
        let sql = "SELECT name, COUNT(*) FROM users GROUP BY name";
        assert!(!ids_for(sql).contains(&"SQL021_GROUP_BY_ORDINAL"));
    }

    // ── SQL022 — ORDER BY constant ─────────────────────────────────

    #[test]
    fn sql022_fires_on_order_by_literal_string() {
        let sql = "SELECT id FROM users ORDER BY 'x'";
        assert!(ids_for(sql).contains(&"SQL022_ORDER_BY_CONSTANT"));
    }

    #[test]
    fn sql022_fires_on_order_by_null() {
        let sql = "SELECT id FROM users ORDER BY NULL";
        assert!(ids_for(sql).contains(&"SQL022_ORDER_BY_CONSTANT"));
    }

    #[test]
    fn sql022_doesnt_fire_on_real_order_by() {
        let sql = "SELECT id FROM users ORDER BY created_at DESC";
        assert!(!ids_for(sql).contains(&"SQL022_ORDER_BY_CONSTANT"));
    }

    // ── SQL023 — NOT IN with NULL literal ──────────────────────────

    #[test]
    fn sql023_fires_on_not_in_with_null_literal() {
        let sql = "SELECT id FROM users WHERE status NOT IN ('active', NULL, 'banned')";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL023_NOT_IN_WITH_NULL"),
            "expected SQL023, got {hits:?}"
        );
    }

    #[test]
    fn sql023_doesnt_fire_on_plain_not_in() {
        let sql = "SELECT id FROM users WHERE status NOT IN ('banned', 'archived')";
        assert!(!ids_for(sql).contains(&"SQL023_NOT_IN_WITH_NULL"));
    }

    // ── SQL025 — JSON path in WHERE ────────────────────────────────

    #[test]
    fn sql025_fires_on_json_path_arrow() {
        // Postgres `->>` (returns text). Common Django/SQLAlchemy
        // JSON-field filter shape.
        let sql = "SELECT id FROM users WHERE data->>'email' = 'a@b.com'";
        let hits = ids_for(sql);
        assert!(
            hits.contains(&"SQL025_JSON_PATH_IN_WHERE"),
            "expected SQL025, got {hits:?}"
        );
    }

    #[test]
    fn sql025_doesnt_fire_without_json_path() {
        let sql = "SELECT id FROM users WHERE email = 'a@b.com'";
        assert!(!ids_for(sql).contains(&"SQL025_JSON_PATH_IN_WHERE"));
    }

    #[test]
    fn rule_catalog_invariants() {
        // Sanity: every rule has a unique id, severity & effort are
        // populated (no defaults left over from copy-paste). Allowed
        // ID prefixes are:
        //   - `SQL*`  — DML antipattern rules (the round-1 catalog)
        //   - `MIG_*` — migration-safety rules (DDL hazards: index
        //               concurrency, drop/alter-type, FK NOT VALID,
        //               column-add NOT NULL no DEFAULT, etc.)
        // Future rule packs add their own prefix and extend this list.
        const ALLOWED_PREFIXES: &[&str] = &["SQL", "MIG_"];
        let mut seen = std::collections::HashSet::new();
        for rule in BUILTIN_RULES {
            assert!(
                seen.insert(rule.id),
                "duplicate rule id in catalog: {}",
                rule.id
            );
            assert!(
                ALLOWED_PREFIXES.iter().any(|p| rule.id.starts_with(p)),
                "rule id `{}` doesn't match any of {ALLOWED_PREFIXES:?}",
                rule.id,
            );
            assert!(!rule.message.is_empty(), "rule {} has empty message", rule.id);
            assert!(!rule.remediation.is_empty(), "rule {} has empty remediation", rule.id);
        }
    }

    // ── SQL_LARGE_IN_LIST ───────────────────────────────────────────

    #[test]
    fn sql_large_in_list_fires_at_100() {
        let lits: Vec<String> = (0..100).map(|i| i.to_string()).collect();
        let sql = format!("SELECT * FROM users WHERE id IN ({})", lits.join(","));
        assert!(
            ids_for(&sql).contains(&"SQL_LARGE_IN_LIST"),
            "expected SQL_LARGE_IN_LIST for IN list of 100"
        );
    }

    #[test]
    fn sql_large_in_list_doesnt_fire_at_50() {
        let lits: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        let sql = format!("SELECT * FROM users WHERE id IN ({})", lits.join(","));
        assert!(!ids_for(&sql).contains(&"SQL_LARGE_IN_LIST"));
    }

    // ── SQL_DISTINCT_NO_ORDER_BY ────────────────────────────────────

    #[test]
    fn sql_distinct_no_order_by_fires() {
        assert!(
            ids_for("SELECT DISTINCT name FROM users").contains(&"SQL_DISTINCT_NO_ORDER_BY"),
            "expected SQL_DISTINCT_NO_ORDER_BY when DISTINCT lacks ORDER BY"
        );
    }

    #[test]
    fn sql_distinct_no_order_by_doesnt_fire_with_order_by() {
        assert!(
            !ids_for("SELECT DISTINCT name FROM users ORDER BY name")
                .contains(&"SQL_DISTINCT_NO_ORDER_BY"),
            "should NOT fire when ORDER BY is present"
        );
    }

    #[test]
    fn sql_distinct_no_order_by_doesnt_fire_without_distinct() {
        assert!(!ids_for("SELECT name FROM users").contains(&"SQL_DISTINCT_NO_ORDER_BY"));
    }

    // ── SQL_OFFSET_DEEP_PAGINATION ──────────────────────────────────

    #[test]
    fn sql_offset_deep_fires_at_1000() {
        assert!(
            ids_for("SELECT * FROM users ORDER BY id LIMIT 10 OFFSET 1000")
                .contains(&"SQL_OFFSET_DEEP_PAGINATION"),
            "expected SQL_OFFSET_DEEP_PAGINATION at OFFSET 1000"
        );
    }

    #[test]
    fn sql_offset_deep_doesnt_fire_at_500() {
        assert!(
            !ids_for("SELECT * FROM users ORDER BY id LIMIT 10 OFFSET 500")
                .contains(&"SQL_OFFSET_DEEP_PAGINATION"),
            "should NOT fire below threshold"
        );
    }

    #[test]
    fn sql_offset_deep_doesnt_fire_without_offset() {
        assert!(
            !ids_for("SELECT * FROM users ORDER BY id LIMIT 10")
                .contains(&"SQL_OFFSET_DEEP_PAGINATION"),
            "no OFFSET clause → no finding"
        );
    }

    // ────────────────────────────────────────────────────────────────
    // .sql file scanner — sanitizers, statement splitting, dispatcher
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn dialect_parse_accepts_aliases() {
        assert_eq!(SqlDialect::parse("postgres"), Some(SqlDialect::Postgres));
        assert_eq!(SqlDialect::parse("PostgreSQL"), Some(SqlDialect::Postgres));
        assert_eq!(SqlDialect::parse("PG"), Some(SqlDialect::Postgres));
        assert_eq!(SqlDialect::parse("mariadb"), Some(SqlDialect::MySql));
        assert_eq!(SqlDialect::parse("sqlite3"), Some(SqlDialect::Sqlite));
        assert_eq!(SqlDialect::parse("tsql"), Some(SqlDialect::MsSql));
        assert_eq!(SqlDialect::parse("googlesql"), Some(SqlDialect::BigQuery));
        assert_eq!(SqlDialect::parse("ansi"), Some(SqlDialect::Generic));
        assert_eq!(SqlDialect::parse("oracle"), None);
    }

    #[test]
    fn dbt_template_detected_by_double_brace() {
        assert!(is_dbt_template("SELECT * FROM {{ ref('users') }}"));
        assert!(is_dbt_template("{% if var('x') %}SELECT 1{% endif %}"));
        assert!(!is_dbt_template("SELECT * FROM users"));
    }

    #[test]
    fn psql_meta_stripped_preserves_line_count() {
        let raw = "\\timing\n\\d users\nSELECT 1;\n";
        let cleaned = strip_psql_meta_commands(raw);
        // Same number of lines so downstream line numbers don't shift.
        assert_eq!(
            cleaned.matches('\n').count(),
            raw.matches('\n').count(),
            "line count must be preserved"
        );
        assert!(cleaned.contains("SELECT 1"));
        assert!(!cleaned.contains("\\timing"));
        assert!(!cleaned.contains("\\d users"));
    }

    #[test]
    fn liquibase_directives_stripped_preserves_lines() {
        let raw = "--liquibase formatted sql\n--changeset alice:1\nSELECT 1;\n--rollback DROP TABLE x;\n";
        let cleaned = strip_liquibase_directives(raw);
        assert_eq!(
            cleaned.matches('\n').count(),
            raw.matches('\n').count(),
        );
        assert!(cleaned.contains("SELECT 1"));
        assert!(!cleaned.contains("--liquibase"));
        assert!(!cleaned.contains("--changeset"));
        assert!(!cleaned.contains("--rollback"));
    }

    #[test]
    fn split_statements_respects_string_with_semicolon() {
        let raw = "SELECT 'a;b';\nUPDATE t SET x = 1;\n";
        let parts = split_statements_with_lines(raw);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].sql.trim().starts_with("SELECT"));
        assert!(parts[1].sql.trim().starts_with("UPDATE"));
    }

    #[test]
    fn split_statements_respects_line_comments() {
        let raw = "-- comment with ; semicolon\nSELECT 1;\n";
        let parts = split_statements_with_lines(raw);
        assert_eq!(parts.len(), 1, "the ; inside the comment is not a separator");
        assert!(parts[0].sql.contains("SELECT 1"));
    }

    #[test]
    fn split_statements_respects_block_comments() {
        let raw = "SELECT 1 /* a; b; c */ FROM t;\nSELECT 2;\n";
        let parts = split_statements_with_lines(raw);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].sql.contains("SELECT 1"));
        assert!(parts[1].sql.contains("SELECT 2"));
    }

    #[test]
    fn split_statements_respects_dollar_quoted() {
        // PL/pgSQL function body contains a semicolon that must NOT split.
        let raw = "CREATE FUNCTION f() RETURNS void AS $$ BEGIN PERFORM 1; END $$ LANGUAGE plpgsql;\nSELECT 1;\n";
        let parts = split_statements_with_lines(raw);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].sql.contains("CREATE FUNCTION"));
        assert!(parts[1].sql.contains("SELECT 1"));
    }

    #[test]
    fn line_for_offset_basic() {
        let starts = line_starts("a\nbb\nccc\n");
        // byte 0 is line 1, byte 2 (start of "bb") is line 2, byte 5 (start of "ccc") is line 3
        assert_eq!(line_for_offset(&starts, 0), 1);
        assert_eq!(line_for_offset(&starts, 2), 2);
        assert_eq!(line_for_offset(&starts, 5), 3);
        // EOF byte stays on the last line.
        assert_eq!(line_for_offset(&starts, 100), 4);
    }

    #[test]
    fn scan_sql_file_fires_sql001_on_select_star() {
        let scan = scan_sql_file("SELECT * FROM users;\n", SqlDialect::Generic);
        assert!(scan.skipped_reason.is_none());
        assert_eq!(scan.findings.len(), 1);
        assert_eq!(scan.findings[0].line, 1);
        // First evidence row carries the rule id chip.
        assert_eq!(scan.findings[0].evidence[0].call, "SQL001");
    }

    #[test]
    fn scan_sql_file_reports_correct_line_for_second_statement() {
        let raw = "SELECT 1;\nSELECT * FROM users;\n";
        let scan = scan_sql_file(raw, SqlDialect::Generic);
        // SELECT * is on line 2.
        assert_eq!(scan.findings.len(), 1);
        assert_eq!(scan.findings[0].line, 2);
    }

    #[test]
    fn scan_sql_file_skips_dbt_templates() {
        let raw = "SELECT * FROM {{ ref('users') }}\n";
        let scan = scan_sql_file(raw, SqlDialect::Generic);
        assert!(scan.findings.is_empty());
        assert!(scan.skipped_reason.is_some());
        assert!(scan
            .skipped_reason
            .unwrap()
            .contains("dbt template"));
    }

    #[test]
    fn scan_sql_file_returns_no_findings_on_unparseable() {
        // Total nonsense — sqlparser will reject every statement.
        let scan = scan_sql_file("THIS IS NOT SQL AT ALL", SqlDialect::Generic);
        assert!(scan.findings.is_empty(), "no findings on unparseable input");
    }

    #[test]
    fn scan_sql_file_psql_meta_stripped_then_lints_rest() {
        // The \timing line is psql meta — gets blanked out. SELECT *
        // on line 2 still fires SQL001.
        let raw = "\\timing\nSELECT * FROM events;\n";
        let scan = scan_sql_file(raw, SqlDialect::Generic);
        assert_eq!(scan.findings.len(), 1);
        assert_eq!(scan.findings[0].line, 2);
    }

    #[test]
    fn scan_sql_file_liquibase_directives_stripped() {
        let raw = "--liquibase formatted sql\n--changeset alice:1\nSELECT * FROM users;\n";
        let scan = scan_sql_file(raw, SqlDialect::Generic);
        assert_eq!(scan.findings.len(), 1);
        assert_eq!(scan.findings[0].line, 3);
    }

    #[test]
    fn attach_sql_file_findings_walks_fixture_dir() {
        use std::path::Path;
        let root = Path::new("tests/fixtures/sql-files");
        if !root.is_dir() {
            // Allow tests to run from a worktree where fixtures aren't
            // present (rare; just guards CI cleanliness).
            return;
        }
        let mut entries: Vec<CallTreeNode> = Vec::new();
        let opts = SqlFileOpts::default();
        attach_sql_file_findings(&mut entries, root, &opts);
        // Expect synthetic entries for: V1__bad_select.sql (2 findings),
        // V2__danger_delete_update.sql (2 findings), psql-meta.sql
        // (1 finding), liquibase-formatted.sql (1 finding).
        // dbt-template.sql is skipped (0 findings → no synthetic node).
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"V1__bad_select.sql"), "missing V1: got {names:?}");
        assert!(names.contains(&"V2__danger_delete_update.sql"), "missing V2: got {names:?}");
        assert!(names.contains(&"psql-meta.sql"), "missing psql-meta: got {names:?}");
        assert!(names.contains(&"liquibase-formatted.sql"), "missing liquibase: got {names:?}");
        assert!(
            !names.contains(&"dbt-template.sql"),
            "dbt template should be skipped, but synthetic node was emitted",
        );
    }

    // ────────────────────────────────────────────────────────────────
    // Migration-safety rules (MIG_*) — positive + negative per rule
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn mig_create_index_not_concurrent_fires() {
        assert_eq!(
            ids_for("CREATE INDEX idx ON users (email);"),
            vec!["MIG_CREATE_INDEX_NOT_CONCURRENT"]
        );
    }

    #[test]
    fn mig_create_index_not_concurrent_doesnt_fire_on_concurrently() {
        assert!(ids_for("CREATE INDEX CONCURRENTLY idx ON users (email);").is_empty());
    }

    #[test]
    fn mig_drop_table_fires() {
        assert_eq!(ids_for("DROP TABLE users;"), vec!["MIG_DROP_TABLE"]);
    }

    #[test]
    fn mig_drop_table_doesnt_fire_on_drop_index() {
        // DROP INDEX is different concern — should NOT trip the
        // table-drop rule. (We can add MIG_DROP_INDEX later; out of
        // scope for v1's top-6.)
        assert!(ids_for("DROP INDEX users_email_idx;").is_empty());
    }

    #[test]
    fn mig_drop_column_fires() {
        assert_eq!(
            ids_for("ALTER TABLE users DROP COLUMN deprecated_flag;"),
            vec!["MIG_DROP_COLUMN"]
        );
    }

    #[test]
    fn mig_drop_column_doesnt_fire_on_add_column() {
        assert!(ids_for("ALTER TABLE users ADD COLUMN flag boolean;").is_empty());
    }

    #[test]
    fn mig_alter_column_type_fires() {
        assert_eq!(
            ids_for("ALTER TABLE users ALTER COLUMN id TYPE bigint;"),
            vec!["MIG_ALTER_COLUMN_TYPE"]
        );
    }

    #[test]
    fn mig_alter_column_type_doesnt_fire_on_set_default() {
        // SET DEFAULT is metadata-only, no rewrite — should NOT fire.
        assert!(ids_for("ALTER TABLE users ALTER COLUMN id SET DEFAULT 1;").is_empty());
    }

    #[test]
    fn mig_add_fk_not_valid_fires_without_not_valid() {
        assert_eq!(
            ids_for(
                "ALTER TABLE orders ADD CONSTRAINT orders_user_fk \
                 FOREIGN KEY (user_id) REFERENCES users(id);"
            ),
            vec!["MIG_ADD_FK_NOT_VALID"],
        );
    }

    #[test]
    fn mig_add_fk_not_valid_doesnt_fire_with_not_valid() {
        let ids = ids_for(
            "ALTER TABLE orders ADD CONSTRAINT orders_user_fk \
             FOREIGN KEY (user_id) REFERENCES users(id) NOT VALID;",
        );
        assert!(
            !ids.contains(&"MIG_ADD_FK_NOT_VALID"),
            "NOT VALID present → no finding; got {ids:?}",
        );
    }

    #[test]
    fn mig_add_column_not_null_no_default_fires() {
        assert_eq!(
            ids_for("ALTER TABLE users ADD COLUMN status text NOT NULL;"),
            vec!["MIG_ADD_COLUMN_NOT_NULL_NO_DEFAULT"]
        );
    }

    #[test]
    fn mig_add_column_not_null_with_default_doesnt_fire() {
        let ids = ids_for("ALTER TABLE users ADD COLUMN status text NOT NULL DEFAULT 'active';");
        assert!(
            !ids.contains(&"MIG_ADD_COLUMN_NOT_NULL_NO_DEFAULT"),
            "NOT NULL + DEFAULT is safe on PG 11+; should not fire. got {ids:?}",
        );
    }

    #[test]
    fn mig_add_column_nullable_doesnt_fire() {
        // Nullable add is always safe — no constraint violation possible.
        let ids = ids_for("ALTER TABLE users ADD COLUMN status text;");
        assert!(
            !ids.contains(&"MIG_ADD_COLUMN_NOT_NULL_NO_DEFAULT"),
            "nullable add is safe; should not fire. got {ids:?}",
        );
    }

    #[test]
    fn mig_rules_dont_fire_on_dml() {
        // SELECT * should fire SQL001 but NOT any MIG_* rule.
        let ids = ids_for("SELECT * FROM users;");
        for id in &ids {
            assert!(
                !id.starts_with("MIG_"),
                "DML statement should not trip migration rules; got {ids:?}",
            );
        }
    }

    // ── Kind discrimination — the SQL/MIG split for findings_by_kind
    //    must hold so the summary report stays legible to users.
    //    Pin both directions so a future rule with a wrong prefix can't
    //    silently drift into the wrong bucket. ─────────────────────────

    #[test]
    fn sql_rules_carry_sql_antipattern_kind() {
        for rule in BUILTIN_RULES {
            if rule.id.starts_with("SQL") {
                assert_eq!(
                    rule.kind(),
                    FindingKind::SqlAntipattern,
                    "rule {} (SQL prefix) must map to SqlAntipattern kind",
                    rule.id,
                );
            }
        }
    }

    #[test]
    fn mig_rules_carry_migration_safety_kind() {
        for rule in BUILTIN_RULES {
            if rule.id.starts_with("MIG_") {
                assert_eq!(
                    rule.kind(),
                    FindingKind::MigrationSafety,
                    "rule {} (MIG_ prefix) must map to MigrationSafety kind",
                    rule.id,
                );
            }
        }
    }

    #[test]
    fn scan_emits_separate_kinds_for_dml_and_ddl() {
        // Mixed file: SELECT * (SQL001 → SqlAntipattern) + CREATE INDEX
        // without CONCURRENTLY (MIG_* → MigrationSafety). Both findings
        // must land on separate FindingKind values so the summary
        // `findings_by_kind` shows two distinct buckets.
        let raw = "SELECT * FROM users;\nCREATE INDEX idx_users_email ON users (email);\n";
        let scan = scan_sql_file(raw, SqlDialect::Generic);
        let kinds: std::collections::HashSet<FindingKind> =
            scan.findings.iter().map(|f| f.kind).collect();
        assert!(
            kinds.contains(&FindingKind::SqlAntipattern),
            "SELECT * should produce a SqlAntipattern finding; got kinds {:?}",
            kinds,
        );
        assert!(
            kinds.contains(&FindingKind::MigrationSafety),
            "CREATE INDEX without CONCURRENTLY should produce a MigrationSafety finding; got kinds {:?}",
            kinds,
        );
    }
}
