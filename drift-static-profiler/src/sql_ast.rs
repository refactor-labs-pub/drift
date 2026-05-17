//! Generic SQL AST predicate library — the foundation any rule
//! engine (SQL lint, ORM lint, migration lint, the upcoming
//! ORM→SQL predictor) composes against.
//!
//! Design principles (Robert C. Martin / Effective Rust):
//!
//!   - **Single Responsibility per function.** One predicate, one
//!     question. `query_has_distinct(stmt)` answers exactly that.
//!   - **Pure.** Every function here is `fn(&...) -> {bool, usize,
//!     Option<...>, Vec<...>}`. No I/O, no allocation beyond `Vec`
//!     returns, no globals.
//!   - **Composable.** Predicates take `&Statement` or `&Expr` (the
//!     two natural roots) so a caller can chain via `||` / `&&` /
//!     iterator methods. Walkers accept `&mut dyn FnMut(...)` so the
//!     caller controls early-exit and state.
//!   - **Public.** `sql_lint.rs`, `orm_lint.rs`, future modules
//!     compose without re-implementing. Adding a rule = compose;
//!     adding a predicate = one `pub fn` here with one unit test.
//!   - **Conservative.** When the AST shape is ambiguous, return
//!     "negative" (`false` / `0` / `None`) rather than guessing. The
//!     plan's false-positive policy (silent-skip on uncertainty)
//!     applies here too.

use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    JoinConstraint, JoinOperator, LimitClause, ObjectNamePart, OrderByKind, Query, SelectItem,
    SetExpr, SetOperator, SetQuantifier, Statement, TableFactor, TableWithJoins, Value,
};

// ────────────────────────────────────────────────────────────────────
// Walkers — the iteration primitives every predicate composes against.
// ────────────────────────────────────────────────────────────────────

/// Walk every `WHERE`-context `Expr` reachable from `stmt` — outer
/// SELECT's selection, joined SELECTs (via subqueries), set-operation
/// branches. The visitor sees each `Expr` node *including children*
/// so a rule can match top-down patterns.
///
/// Caller controls state via the closure capture; this fn does not
/// allocate.
pub fn walk_where_exprs(stmt: &Statement, visit: &mut dyn FnMut(&Expr)) {
    fn walk_query(q: &Query, visit: &mut dyn FnMut(&Expr)) {
        walk_set_expr(q.body.as_ref(), visit);
    }
    fn walk_set_expr(body: &SetExpr, visit: &mut dyn FnMut(&Expr)) {
        match body {
            SetExpr::Select(select) => {
                if let Some(w) = &select.selection {
                    walk_expr(w, visit);
                }
                for t in &select.from {
                    walk_table_with_joins(t, visit);
                }
            }
            SetExpr::Query(q) => walk_query(q, visit),
            SetExpr::SetOperation { left, right, .. } => {
                walk_set_expr(left, visit);
                walk_set_expr(right, visit);
            }
            _ => {}
        }
    }
    fn walk_table_with_joins(t: &TableWithJoins, visit: &mut dyn FnMut(&Expr)) {
        walk_table_factor(&t.relation, visit);
        for j in &t.joins {
            walk_table_factor(&j.relation, visit);
            if let JoinOperator::Inner(c)
            | JoinOperator::LeftOuter(c)
            | JoinOperator::RightOuter(c)
            | JoinOperator::FullOuter(c) = &j.join_operator
            {
                if let JoinConstraint::On(e) = c {
                    walk_expr(e, visit);
                }
            }
        }
    }
    fn walk_table_factor(tf: &TableFactor, visit: &mut dyn FnMut(&Expr)) {
        if let TableFactor::Derived { subquery, .. } = tf {
            walk_query(subquery, visit);
        }
    }
    fn walk_expr(e: &Expr, visit: &mut dyn FnMut(&Expr)) {
        visit(e);
        match e {
            Expr::BinaryOp { left, right, .. } => {
                walk_expr(left, visit);
                walk_expr(right, visit);
            }
            Expr::UnaryOp { expr, .. } => walk_expr(expr, visit),
            Expr::Nested(inner) => walk_expr(inner, visit),
            Expr::IsNull(inner) | Expr::IsNotNull(inner) => walk_expr(inner, visit),
            Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
                walk_expr(expr, visit);
                walk_expr(pattern, visit);
            }
            Expr::InList { expr, list, .. } => {
                walk_expr(expr, visit);
                for x in list {
                    walk_expr(x, visit);
                }
            }
            Expr::InSubquery { expr, subquery, .. } => {
                walk_expr(expr, visit);
                walk_query(subquery, visit);
            }
            Expr::Subquery(q) | Expr::Exists { subquery: q, .. } => walk_query(q, visit),
            _ => {}
        }
    }
    let Statement::Query(q) = stmt else { return };
    walk_query(q, visit);
}

// ────────────────────────────────────────────────────────────────────
// Structural counters — `usize`-valued predicates.
// ────────────────────────────────────────────────────────────────────

/// Count tables involved in the query — base table + every `JOIN`
/// keyword. A 6-table join (`FROM a JOIN b … JOIN f`) returns 6.
/// Recurses through subqueries and set-operation branches, taking
/// the **max** branch (not sum) for set ops so an n-arm UNION of
/// 3-table queries doesn't accidentally trip a complexity rule.
pub fn count_joins_in_query(q: &Query) -> usize {
    fn count_set_expr(body: &SetExpr) -> usize {
        match body {
            SetExpr::Select(s) => {
                let here: usize = s.from.iter().map(|t| 1 + t.joins.len()).sum();
                let nested = s
                    .from
                    .iter()
                    .map(|t| factor_joins(&t.relation))
                    .chain(
                        s.from
                            .iter()
                            .flat_map(|t| t.joins.iter().map(|j| factor_joins(&j.relation))),
                    )
                    .max()
                    .unwrap_or(0);
                here.max(nested)
            }
            SetExpr::Query(q) => count_joins_in_query(q),
            SetExpr::SetOperation { left, right, .. } => {
                count_set_expr(left).max(count_set_expr(right))
            }
            _ => 0,
        }
    }
    fn factor_joins(tf: &TableFactor) -> usize {
        if let TableFactor::Derived { subquery, .. } = tf {
            count_joins_in_query(subquery)
        } else {
            0
        }
    }
    count_set_expr(q.body.as_ref())
}

/// Deepest nesting of `Query` inside `Query` reachable. Top-level is
/// depth 1.
pub fn max_subquery_depth_in_query(q: &Query, depth: usize) -> usize {
    fn walk_set_expr(body: &SetExpr, depth: usize) -> usize {
        match body {
            SetExpr::Select(s) => {
                let mut max_d = depth;
                if let Some(w) = &s.selection {
                    max_d = max_d.max(expr_depth(w, depth));
                }
                for t in &s.from {
                    max_d = max_d.max(table_factor_depth(&t.relation, depth));
                    for j in &t.joins {
                        max_d = max_d.max(table_factor_depth(&j.relation, depth));
                    }
                }
                max_d
            }
            SetExpr::Query(q) => max_subquery_depth_in_query(q, depth + 1),
            SetExpr::SetOperation { left, right, .. } => {
                walk_set_expr(left, depth).max(walk_set_expr(right, depth))
            }
            _ => depth,
        }
    }
    fn table_factor_depth(tf: &TableFactor, depth: usize) -> usize {
        if let TableFactor::Derived { subquery, .. } = tf {
            max_subquery_depth_in_query(subquery, depth + 1)
        } else {
            depth
        }
    }
    fn expr_depth(e: &Expr, depth: usize) -> usize {
        match e {
            Expr::Subquery(q) | Expr::Exists { subquery: q, .. } => {
                max_subquery_depth_in_query(q, depth + 1)
            }
            Expr::InSubquery { subquery, .. } => max_subquery_depth_in_query(subquery, depth + 1),
            Expr::BinaryOp { left, right, .. } => {
                expr_depth(left, depth).max(expr_depth(right, depth))
            }
            Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => expr_depth(inner, depth),
            _ => depth,
        }
    }
    walk_set_expr(q.body.as_ref(), depth)
}

/// Length of the OR chain rooted at `e`. Returns 1 for non-OR.
pub fn or_chain_length(e: &Expr) -> usize {
    match e {
        Expr::BinaryOp { left, op, right } if matches!(op, BinaryOperator::Or) => {
            or_chain_length(left) + or_chain_length(right)
        }
        _ => 1,
    }
}

/// For an OR-chain `a = x OR a = y OR …`, return the column name on
/// each leaf where the leaf is `col = literal`. Non-matching leaves
/// contribute nothing.
pub fn collect_or_chain_columns(e: &Expr) -> Vec<String> {
    fn leaf_col(e: &Expr) -> Option<String> {
        let Expr::BinaryOp { left, op, right } = e else {
            return None;
        };
        if !matches!(op, BinaryOperator::Eq) {
            return None;
        }
        let lhs_col = identifier_name(left);
        let rhs_col = identifier_name(right);
        // Prefer the column-on-left form (canonical) but accept either.
        lhs_col.or(rhs_col)
    }
    fn walk(e: &Expr, out: &mut Vec<String>) {
        if let Expr::BinaryOp { left, op, right } = e {
            if matches!(op, BinaryOperator::Or) {
                walk(left, out);
                walk(right, out);
                return;
            }
        }
        if let Some(col) = leaf_col(e) {
            out.push(col);
        }
    }
    let mut out = Vec::new();
    walk(e, &mut out);
    out
}

/// **NEW.** Largest `IN (literal, literal, …)` list size found anywhere
/// in `stmt`'s WHERE-context expressions. Subquery IN (`IN (SELECT …)`)
/// does not contribute. Returns 0 when no IN-list is found.
///
/// Useful for `SQL_LARGE_IN_LIST` — Postgres planner limits + the
/// memory-cost of building a giant hash set bite around N>=100.
pub fn count_largest_in_list(stmt: &Statement) -> usize {
    let mut largest = 0usize;
    walk_where_exprs(stmt, &mut |e| {
        if let Expr::InList { list, .. } = e {
            if list.len() > largest {
                largest = list.len();
            }
        }
    });
    largest
}

// ────────────────────────────────────────────────────────────────────
// Feature-flag predicates — `bool`-valued questions about clauses.
// ────────────────────────────────────────────────────────────────────

/// True iff the top-level SELECT is `SELECT DISTINCT` (any variant —
/// `DISTINCT`, `DISTINCT ON (...)`).
pub fn query_has_distinct(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    s.distinct.is_some()
}

/// True iff the top-level query carries an `ORDER BY` clause.
pub fn query_has_order_by(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    q.order_by
        .as_ref()
        .map(|o| match &o.kind {
            OrderByKind::Expressions(v) => !v.is_empty(),
            _ => false,
        })
        .unwrap_or(false)
}

/// **NEW.** Best-effort integer value of `OFFSET N` on the top-level
/// query. Returns `None` when there's no `OFFSET`, when it's a
/// non-literal expression (`OFFSET ?`, `OFFSET $1`), or when the
/// literal isn't parseable as `usize`.
///
/// Useful for `SQL_OFFSET_DEEP_PAGINATION` — `OFFSET 100_000` is a
/// linear-scan tax. Keyset pagination is the fix.
pub fn query_offset_value(stmt: &Statement) -> Option<usize> {
    let Statement::Query(q) = stmt else { return None };
    // sqlparser 0.62 collapses LIMIT and OFFSET into a single
    // `LimitClause` enum. We support both the standard form (LIMIT N
    // OFFSET M) and MySQL's reversed form (LIMIT offset, limit).
    match q.limit_clause.as_ref()? {
        LimitClause::LimitOffset { offset, .. } => {
            literal_unsigned_int(&offset.as_ref()?.value)
        }
        LimitClause::OffsetCommaLimit { offset, .. } => literal_unsigned_int(offset),
    }
}

/// True iff `body` is (recursively) a `SetOperation` whose top
/// operator is `UNION` *without* `ALL` (implicit `DISTINCT`).
pub fn set_expr_has_implicit_union(body: &SetExpr) -> bool {
    if let SetExpr::SetOperation {
        op,
        set_quantifier,
        left,
        right,
    } = body
    {
        if matches!(op, SetOperator::Union) && !matches!(set_quantifier, SetQuantifier::All) {
            return true;
        }
        return set_expr_has_implicit_union(left) || set_expr_has_implicit_union(right);
    }
    false
}

// ────────────────────────────────────────────────────────────────────
// Expression-shape predicates — composable building blocks.
// ────────────────────────────────────────────────────────────────────

/// True if `e` is `Function(identifier)` — i.e. a function call whose
/// first argument is a bare column reference. Catches `LOWER(email)`,
/// `DATE(created_at)`, `COALESCE(col, 0)` — defeats a plain index.
pub fn is_function_on_identifier(e: &Expr) -> bool {
    let Expr::Function(f) = e else { return false };
    if f.name.0.is_empty() {
        return false;
    }
    let FunctionArguments::List(args) = &f.args else {
        return false;
    };
    let Some(first) = args.args.first() else {
        return false;
    };
    let FunctionArg::Unnamed(arg_expr) = first else {
        return false;
    };
    let FunctionArgExpr::Expr(inner) = arg_expr else {
        return false;
    };
    matches!(inner, Expr::Identifier(_) | Expr::CompoundIdentifier(_))
}

/// Recursive check: is any subexpression `col = expr` (an equality)?
pub fn where_contains_equality(e: &Expr) -> bool {
    match e {
        Expr::BinaryOp { op, left, right } => match op {
            BinaryOperator::Eq => true,
            BinaryOperator::And | BinaryOperator::Or => {
                where_contains_equality(left) || where_contains_equality(right)
            }
            _ => false,
        },
        Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => where_contains_equality(inner),
        _ => false,
    }
}

/// **GENERIC** version of the old `expr_calls_random` — true iff `e`
/// is a function call whose terminal name (case-insensitive) is in
/// `names`. Replaces a hand-coded match.
pub fn expr_call_name_matches(e: &Expr, names: &[&str]) -> bool {
    let Expr::Function(f) = e else { return false };
    let name = f
        .name
        .0
        .last()
        .map(part_name)
        .unwrap_or("")
        .to_ascii_uppercase();
    names.iter().any(|n| n.eq_ignore_ascii_case(&name))
}

/// Back-compat thin wrapper. `RANDOM`/`RAND`/`NEWID` is the canonical
/// "sort-the-whole-result-set" smell.
pub fn expr_calls_random(e: &Expr) -> bool {
    expr_call_name_matches(e, &["RANDOM", "RAND", "NEWID"])
}

// ────────────────────────────────────────────────────────────────────
// Identifier + literal extractors.
// ────────────────────────────────────────────────────────────────────

pub fn part_name(p: &ObjectNamePart) -> &str {
    match p {
        ObjectNamePart::Identifier(id) => id.value.as_str(),
        _ => "",
    }
}

pub fn identifier_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Identifier(id) => Some(id.value.clone()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|p| p.value.clone()),
        _ => None,
    }
}

/// Best-effort extraction of a string literal from an `Expr`, walking
/// through `Nested` and supporting `Value::SingleQuotedString` /
/// `Value::DoubleQuotedString` and raw variants. Returns `None` for
/// non-literals.
pub fn literal_string(e: &Expr) -> Option<&str> {
    match e {
        Expr::Value(v) => match &v.value {
            Value::SingleQuotedString(s)
            | Value::DoubleQuotedString(s)
            | Value::EscapedStringLiteral(s)
            | Value::SingleQuotedRawStringLiteral(s)
            | Value::DoubleQuotedRawStringLiteral(s) => Some(s.as_str()),
            _ => None,
        },
        Expr::Nested(inner) => literal_string(inner),
        _ => None,
    }
}

/// Best-effort extraction of an unsigned integer literal. `Expr::Value(
/// Value::Number("123", _))` → `Some(123)`. `Nested` is unwrapped.
/// `None` for non-literals or unparseable text.
pub fn literal_unsigned_int(e: &Expr) -> Option<usize> {
    match e {
        Expr::Value(v) => match &v.value {
            Value::Number(s, _) => s.parse::<usize>().ok(),
            _ => None,
        },
        Expr::Nested(inner) => literal_unsigned_int(inner),
        _ => None,
    }
}

/// True if `pattern` begins with `%` or `_` (SQL LIKE wildcards).
/// `ESCAPE` clauses can change this in theory; in practice almost no
/// one uses them, and Postgres docs explicitly call out leading
/// wildcards as the unindexable case.
pub fn has_leading_wildcard(pattern: &str) -> bool {
    matches!(pattern.chars().next(), Some('%') | Some('_'))
}

// ────────────────────────────────────────────────────────────────────
// HAVING / GROUP BY / aggregation predicates.
// ────────────────────────────────────────────────────────────────────

/// Names of SQL aggregate functions we recognize when looking for
/// "this expression contains an aggregate". Matched case-insensitively
/// — `count(*)` and `COUNT(*)` both fire.
///
/// Standard set (Postgres + MySQL + SQL Server overlap) plus the
/// common analytic-style aggregates that compile to the same flag
/// for our purposes.
const AGGREGATE_FUNCTION_NAMES: &[&str] = &[
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "ARRAY_AGG",
    "STRING_AGG",
    "JSON_AGG",
    "JSONB_AGG",
    "JSON_OBJECT_AGG",
    "BOOL_AND",
    "BOOL_OR",
    "EVERY",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "VAR_POP",
    "VAR_SAMP",
    "VARIANCE",
    "GROUP_CONCAT",
    "LISTAGG",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
];

/// True iff the expression tree contains any call to a recognized
/// aggregate function. Walks through `BinaryOp`, `UnaryOp`, `Nested`,
/// `Cast`, `Function`-argument lists.
pub fn expr_contains_aggregate(e: &Expr) -> bool {
    match e {
        Expr::Function(f) => {
            let name = f
                .name
                .0
                .last()
                .map(part_name)
                .unwrap_or("")
                .to_ascii_uppercase();
            if AGGREGATE_FUNCTION_NAMES.iter().any(|n| *n == name) {
                return true;
            }
            // Recurse through arguments — an aggregate nested inside a
            // non-aggregate function still counts.
            if let FunctionArguments::List(args) = &f.args {
                return args.args.iter().any(|a| {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(inner)) = a {
                        expr_contains_aggregate(inner)
                    } else {
                        false
                    }
                });
            }
            false
        }
        Expr::BinaryOp { left, right, .. } => {
            expr_contains_aggregate(left) || expr_contains_aggregate(right)
        }
        Expr::UnaryOp { expr, .. } | Expr::Nested(expr) => expr_contains_aggregate(expr),
        Expr::Cast { expr, .. } => expr_contains_aggregate(expr),
        _ => false,
    }
}

/// True iff the top-level SELECT has a `HAVING` clause whose
/// predicate contains *no* aggregate function. That predicate could
/// run in `WHERE` instead and benefit from pre-aggregate pruning.
pub fn having_lacks_aggregate(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    s.having
        .as_ref()
        .map(|h| !expr_contains_aggregate(h))
        .unwrap_or(false)
}

/// True iff the top-level SELECT has aggregate column(s) AND
/// non-aggregate column(s) AND no `GROUP BY` — the canonical
/// "ambiguous group" bug (Karwin ch.15). Postgres rejects; MySQL
/// silently picks an arbitrary row per group.
pub fn select_has_mixed_aggregation_no_group_by(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    if !group_by_is_empty(s) {
        return false;
    }
    let mut has_aggregate = false;
    let mut has_plain = false;
    for item in &s.projection {
        match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                if expr_contains_aggregate(e) {
                    has_aggregate = true;
                } else if matches!(e, Expr::Identifier(_) | Expr::CompoundIdentifier(_)) {
                    has_plain = true;
                }
            }
            _ => {}
        }
    }
    has_aggregate && has_plain
}

fn group_by_is_empty(s: &sqlparser::ast::Select) -> bool {
    use sqlparser::ast::GroupByExpr;
    match &s.group_by {
        GroupByExpr::Expressions(v, _) => v.is_empty(),
        _ => false,
    }
}

/// True iff `GROUP BY` uses ordinal positions (`GROUP BY 1, 2`) —
/// silently misaligns when the SELECT projection reorders.
pub fn group_by_uses_ordinal(stmt: &Statement) -> bool {
    use sqlparser::ast::GroupByExpr;
    let Statement::Query(q) = stmt else { return false };
    let SetExpr::Select(s) = q.body.as_ref() else { return false };
    let GroupByExpr::Expressions(exprs, _) = &s.group_by else {
        return false;
    };
    exprs
        .iter()
        .any(|e| matches!(e, Expr::Value(_) if literal_unsigned_int(e).is_some()))
}

/// True iff every expression in `ORDER BY` is a constant (literal or
/// literal-only BinaryOp) — i.e. the ORDER BY is a no-op the planner
/// has to recognize and discard. `ORDER BY NULL`, `ORDER BY 'x'`,
/// `ORDER BY 1=1` all fire.
pub fn order_by_is_all_constant(stmt: &Statement) -> bool {
    let Statement::Query(q) = stmt else { return false };
    let Some(order_by) = q.order_by.as_ref() else { return false };
    let OrderByKind::Expressions(items) = &order_by.kind else { return false };
    if items.is_empty() {
        return false;
    }
    items.iter().all(|it| expr_is_constant(&it.expr))
}

fn expr_is_constant(e: &Expr) -> bool {
    match e {
        Expr::Value(_) => true,
        Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => expr_is_constant(inner),
        Expr::BinaryOp { left, right, .. } => expr_is_constant(left) && expr_is_constant(right),
        _ => false,
    }
}

/// True iff WHERE contains a `NOT IN (literal, …)` list that includes
/// a `NULL` literal — the canonical "always-empty result" bug. The
/// IN-with-NULL gotcha turns the entire predicate into `UNKNOWN` and
/// the row count silently drops to zero.
pub fn where_has_not_in_with_null(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        if let Expr::InList {
            negated: true,
            list,
            ..
        } = e
        {
            if list.iter().any(|item| {
                matches!(
                    item,
                    Expr::Value(v) if matches!(v.value, sqlparser::ast::Value::Null),
                )
            }) {
                found = true;
            }
        }
    });
    found
}

/// True iff any expression in `WHERE` uses a JSON-path operator
/// (Postgres `->`, `->>`, `#>`, `#>>`) on a column. Without a
/// functional / GIN index on the path, this is a sequential scan
/// every time.
pub fn where_uses_json_path(stmt: &Statement) -> bool {
    let mut found = false;
    walk_where_exprs(stmt, &mut |e| {
        if let Expr::BinaryOp { op, .. } = e {
            if matches!(
                op,
                BinaryOperator::Arrow
                    | BinaryOperator::LongArrow
                    | BinaryOperator::HashArrow
                    | BinaryOperator::HashLongArrow,
            ) {
                found = true;
            }
        }
    });
    found
}

// ────────────────────────────────────────────────────────────────────
// Avoid unused-import warning — `Distinct` is referenced indirectly
// (via the `query_has_distinct` predicate which only checks `.is_some()`).
// Future rules will inspect `Distinct::On(cols)` for column-specific
// rules; the import stays so they don't have to add it.
#[allow(dead_code)]
const _DISTINCT_USED: Option<&Distinct> = None;

// ────────────────────────────────────────────────────────────────────
// Tests — each predicate exercised in isolation. New predicates land
// here together with their test; that's the contract for extending
// the library.
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser as SqlParser;

    fn parse(sql: &str) -> Statement {
        SqlParser::parse_sql(&GenericDialect {}, sql)
            .expect("parse")
            .into_iter()
            .next()
            .expect("at least one statement")
    }

    #[test]
    fn count_joins_one_base_table() {
        let s = parse("SELECT * FROM users");
        let Statement::Query(q) = &s else { panic!() };
        assert_eq!(count_joins_in_query(q), 1);
    }

    #[test]
    fn count_joins_three_table_chain() {
        let s = parse("SELECT * FROM a JOIN b ON a.id=b.a_id JOIN c ON b.id=c.b_id");
        let Statement::Query(q) = &s else { panic!() };
        assert_eq!(count_joins_in_query(q), 3);
    }

    #[test]
    fn count_joins_subquery_with_more_joins_dominates() {
        // Outer has 1 table; subquery joins 4. Result = 4 (max, not sum).
        let s = parse(
            "SELECT * FROM (SELECT * FROM a JOIN b ON a.id=b.a_id JOIN c ON b.id=c.b_id JOIN d ON c.id=d.c_id) sub",
        );
        let Statement::Query(q) = &s else { panic!() };
        assert!(count_joins_in_query(q) >= 4);
    }

    #[test]
    fn subquery_depth_top_level_is_one() {
        let s = parse("SELECT * FROM users");
        let Statement::Query(q) = &s else { panic!() };
        assert_eq!(max_subquery_depth_in_query(q, 1), 1);
    }

    #[test]
    fn subquery_depth_three_nested() {
        let s = parse("SELECT * FROM (SELECT * FROM (SELECT * FROM t) a) b");
        let Statement::Query(q) = &s else { panic!() };
        assert_eq!(max_subquery_depth_in_query(q, 1), 3);
    }

    #[test]
    fn or_chain_length_three() {
        // Parse a tiny SQL containing the OR chain we want to test.
        let s = parse("SELECT * FROM t WHERE a = 1 OR a = 2 OR a = 3");
        let Statement::Query(q) = &s else { panic!() };
        let SetExpr::Select(sel) = q.body.as_ref() else { panic!() };
        let w = sel.selection.as_ref().expect("WHERE");
        assert_eq!(or_chain_length(w), 3);
    }

    #[test]
    fn or_chain_columns_collects_same_column() {
        let s = parse("SELECT * FROM t WHERE a = 1 OR a = 2 OR a = 3");
        let Statement::Query(q) = &s else { panic!() };
        let SetExpr::Select(sel) = q.body.as_ref() else { panic!() };
        let w = sel.selection.as_ref().expect("WHERE");
        let cols = collect_or_chain_columns(w);
        assert_eq!(cols, vec!["a", "a", "a"]);
    }

    #[test]
    fn has_leading_wildcard_cases() {
        assert!(has_leading_wildcard("%foo"));
        assert!(has_leading_wildcard("_foo"));
        assert!(has_leading_wildcard("%foo%"));
        assert!(!has_leading_wildcard("foo%"));
        assert!(!has_leading_wildcard("foo"));
    }

    #[test]
    fn expr_call_name_matches_case_insensitive() {
        let s = parse("SELECT * FROM t ORDER BY RaNdOm()");
        // Grab the ORDER BY function expr.
        let Statement::Query(q) = &s else { panic!() };
        let order_by = q.order_by.as_ref().expect("order by");
        let OrderByKind::Expressions(exprs) = &order_by.kind else { panic!() };
        let e = &exprs[0].expr;
        assert!(expr_call_name_matches(e, &["random", "rand"]));
        assert!(expr_calls_random(e), "back-compat wrapper still matches");
    }

    // ── new predicates ──────────────────────────────────────────────

    #[test]
    fn count_largest_in_list_simple() {
        let s = parse("SELECT * FROM users WHERE id IN (1, 2, 3, 4, 5)");
        assert_eq!(count_largest_in_list(&s), 5);
    }

    #[test]
    fn count_largest_in_list_zero_when_no_in() {
        let s = parse("SELECT * FROM users WHERE id = 1");
        assert_eq!(count_largest_in_list(&s), 0);
    }

    #[test]
    fn count_largest_in_list_ignores_in_subquery() {
        // `IN (SELECT ...)` is `InSubquery`, not `InList` — must not contribute.
        let s = parse("SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)");
        assert_eq!(count_largest_in_list(&s), 0);
    }

    #[test]
    fn count_largest_in_list_takes_max() {
        // Two IN lists; we report the larger.
        let s = parse(
            "SELECT * FROM users WHERE id IN (1,2) AND status IN ('a','b','c','d','e')",
        );
        assert_eq!(count_largest_in_list(&s), 5);
    }

    #[test]
    fn query_has_distinct_true_for_select_distinct() {
        let s = parse("SELECT DISTINCT name FROM users");
        assert!(query_has_distinct(&s));
    }

    #[test]
    fn query_has_distinct_false_for_plain_select() {
        let s = parse("SELECT name FROM users");
        assert!(!query_has_distinct(&s));
    }

    #[test]
    fn query_has_order_by_true_when_clause_present() {
        let s = parse("SELECT * FROM users ORDER BY id");
        assert!(query_has_order_by(&s));
    }

    #[test]
    fn query_has_order_by_false_when_absent() {
        let s = parse("SELECT * FROM users");
        assert!(!query_has_order_by(&s));
    }

    #[test]
    fn query_offset_value_simple_literal() {
        let s = parse("SELECT * FROM users LIMIT 10 OFFSET 5000");
        assert_eq!(query_offset_value(&s), Some(5000));
    }

    #[test]
    fn query_offset_value_none_when_absent() {
        let s = parse("SELECT * FROM users LIMIT 10");
        assert_eq!(query_offset_value(&s), None);
    }
}
