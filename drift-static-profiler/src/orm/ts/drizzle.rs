//! Drizzle — reified expression tree (F1 family per master plan §CCC.2).
//!
//! Phase 2 v1 rules:
//! - `DRZ-LMT-001` — `.limit()` without `.orderBy()` (non-deterministic)
//! - `DRZ-N1-002` — `db.select()...where(eq(t.id, loopVar))` inside a loop
//! - `DRZ-CTR-003` — `.select().from(t)` followed by `.length` (use `count()`)

use crate::insights::{Effort, Evidence, Severity};
use crate::orm::context::{BindingKind, CallChain, ChainRoot, PyOrmContext, TsClientKind};
use crate::orm::dialect::OrmDialect;
use crate::orm::shape::{matches_by_shape, ComboRule, RootPredicate, ShapeSpec};
use crate::orm::sql_ir::{
    LimitSpec, OrmKind, PredictedSql, PredictedStatement, Projection, SqlDialect, SqlFidelity,
    SqlOp, TableRef, WhereExpr,
};
use crate::orm::{Framework, MatchHit, OrmRule};

fn hit(chain: &CallChain, note: &str) -> MatchHit {
    MatchHit {
        line: chain.steps.last().map(|s| s.line).unwrap_or(1),
        byte_range: chain.byte_range.clone(),
        extra_evidence: vec![Evidence {
            call: note.to_string(),
            line: chain.steps.last().map(|s| s.line).unwrap_or(1),
            category: None,
        }],
    }
}

fn is_drizzle_chain(chain: &CallChain, ctx: &PyOrmContext<'_>) -> bool {
    let root_text = match &chain.root {
        ChainRoot::Identifier(t) | ChainRoot::Binding(t) | ChainRoot::LoopVar(t) => t.clone(),
        _ => return false,
    };
    let methods: Vec<&str> = chain.steps.iter().map(|s| s.method.as_str()).collect();
    let has_query_op = methods
        .iter()
        .any(|m| matches!(*m, "select" | "insert" | "update" | "delete"));

    // Direct identifier root: `db.select()…`, `drizzle.select()…`,
    // `tx.select()…` (transaction callback param from `db.transaction(tx => …)`).
    if has_query_op && matches!(root_text.as_str(), "db" | "drizzle" | "tx") {
        return true;
    }
    // Receiver pattern: tree-sitter gives `this.db.select()` as
    // root=`this` with first step `db`. This is the canonical class-field
    // shape used by every Drizzle-in-a-class codebase (Supabase RLS
    // wrappers, repository classes, etc.). Without this branch the
    // detector silently skips every `this.db.<op>` chain.
    if has_query_op && matches!(root_text.as_str(), "this" | "self") {
        if let Some(first) = methods.first() {
            if matches!(*first, "db" | "drizzle" | "client" | "rlsBase" | "admin") {
                return true;
            }
        }
    }
    if let Some(b) = ctx.binding_at(&root_text, chain.byte_range.start) {
        if let BindingKind::TsClient(facts) = &b.kind {
            return matches!(facts.kind, TsClientKind::Drizzle);
        }
    }
    false
}

// ─── DRZ-LMT-001: .limit() without .orderBy() ───────────────────────────

fn matches_drz_lmt_001(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !is_drizzle_chain(chain, ctx) {
            continue;
        }
        let methods: Vec<&str> = chain.steps.iter().map(|s| s.method.as_str()).collect();
        if methods.contains(&"limit") && !methods.contains(&"orderBy") {
            out.push(hit(chain, "DRZ-LMT-001"));
        }
    }
    out
}

// ─── DRZ-N1-002: select with where referencing loop var ─────────────────

fn matches_drz_n1_002(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !chain.in_loop {
            continue;
        }
        if !is_drizzle_chain(chain, ctx) {
            continue;
        }
        // any chain in a loop with select+where → likely N+1
        let methods: Vec<&str> = chain.steps.iter().map(|s| s.method.as_str()).collect();
        if methods.contains(&"select") && methods.contains(&"where") {
            out.push(hit(chain, "DRZ-N1-002"));
        }
    }
    out
}

// ─── DRZ-CTR-003: full-row select used for counting ─────────────────────
//
// Detection: a `db.select().from(t)` chain assigned to a variable that
// later appears as the root of a `<bound>.length` field-access chain.

fn matches_drz_ctr_003(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    let mut drizzle_list_bindings: std::collections::HashSet<String> = Default::default();
    for chain in &ctx.chains {
        if !is_drizzle_chain(chain, ctx) {
            continue;
        }
        let methods: Vec<&str> = chain.steps.iter().map(|s| s.method.as_str()).collect();
        if !methods.contains(&"select") || !methods.contains(&"from") {
            continue;
        }
        for (name, bs) in &ctx.bindings {
            for b in bs {
                if b.byte_range == chain.byte_range {
                    drizzle_list_bindings.insert(name.clone());
                }
            }
        }
    }
    if drizzle_list_bindings.is_empty() {
        return out;
    }
    for chain in &ctx.chains {
        // chain of shape `<name>.length` — root binding + first step 'length'.
        let root_text = match &chain.root {
            ChainRoot::Binding(t) | ChainRoot::Identifier(t) | ChainRoot::LoopVar(t) => t.clone(),
            _ => continue,
        };
        if !drizzle_list_bindings.contains(&root_text) {
            continue;
        }
        if chain.steps.first().map(|s| s.method.as_str()) == Some("length") {
            out.push(hit(chain, "DRZ-CTR-003"));
        }
    }
    out
}

pub const DRIZZLE_RULES: &[OrmRule] = &[
    OrmRule {
        id: "DRZ-LMT-001",
        framework: Framework::Generic,
        severity: Severity::Medium,
        effort: Effort::Trivial,
        message: "Drizzle `.limit(N)` without `.orderBy(...)` — result rows are non-deterministic.",
        remediation: "Add `.orderBy(...)` before `.limit(...)`.",
        confidence: 0.90,
        matches: matches_drz_lmt_001,
    },
    OrmRule {
        id: "DRZ-N1-002",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "Drizzle `db.select(...).where(...)` inside a loop — N+1 round-trips.",
        remediation: "Collect keys and run a single `.where(inArray(t.id, ids))` query outside the loop.",
        confidence: 0.85,
        matches: matches_drz_n1_002,
    },
    OrmRule {
        id: "DRZ-CTR-003",
        framework: Framework::Generic,
        severity: Severity::Low,
        effort: Effort::Trivial,
        message: "Drizzle full-row select used for counting — use `count(*)`.",
        remediation: "Replace `(await db.select().from(t)).length` with `db.select({ c: count() }).from(t)`.",
        confidence: 0.55,
        matches: matches_drz_ctr_003,
    },
];

// ─── DrizzleDialect — predict_all ───────────────────────────────────────

/// Shape-based fallback for Drizzle. The schema-declaration functions
/// (`pgTable`, `mysqlTable`, `sqliteTable`) and TS-only inference
/// helpers (`$inferSelect`, `$inferInsert`) are uniquely Drizzle.
/// `inArray` / `notInArray` are Drizzle's array operators.
/// The combo `db.select().from(...)` is the canonical query shape and
/// the chain root being a singleton `db` / `tx` / `drizzle` reliably
/// distinguishes it from a custom builder.
pub(crate) const DRIZZLE_SHAPE: ShapeSpec = ShapeSpec {
    anchors: &[
        "$inferSelect",
        "$inferInsert",
        "pgTable",
        "mysqlTable",
        "sqliteTable",
        "inArray",
        "notInArray",
        "arrayContains",
        "arrayContained",
    ],
    combos: &[
        ComboRule {
            first_method: "select",
            root: RootPredicate::Equals("db"),
            continuation_any: &["from"],
        },
        ComboRule {
            first_method: "insert",
            root: RootPredicate::Equals("db"),
            continuation_any: &["values"],
        },
        ComboRule {
            first_method: "update",
            root: RootPredicate::Equals("db"),
            continuation_any: &["set"],
        },
        ComboRule {
            first_method: "delete",
            root: RootPredicate::Equals("db"),
            continuation_any: &["where"],
        },
    ],
};

pub struct DrizzleDialect;

impl OrmDialect for DrizzleDialect {
    fn orm(&self) -> OrmKind {
        OrmKind::Generic
    }

    fn matches(&self, ctx: &PyOrmContext<'_>) -> bool {
        ctx.imports.has_any_starting_with("drizzle-orm")
            || ctx.imports.has_any_starting_with("drizzle")
            || matches_by_shape(&ctx.chains, &DRIZZLE_SHAPE)
    }

    fn predict_all(&self, ctx: &PyOrmContext<'_>) -> Vec<PredictedSql> {
        let mut out = Vec::new();
        for chain in &ctx.chains {
            if !is_drizzle_chain(chain, ctx) {
                continue;
            }
            let mut op = SqlOp::Select;
            let mut table = String::new();
            let mut limit: Option<LimitSpec> = None;
            let mut order_by: Vec<String> = Vec::new();
            let mut where_present = false;
            for step in &chain.steps {
                match step.method.as_str() {
                    "select" => op = SqlOp::Select,
                    "insert" => op = SqlOp::Insert,
                    "update" => op = SqlOp::Update,
                    "delete" => op = SqlOp::Delete,
                    "from" | "into" => {
                        if let Some(t) = step.args_text.first() {
                            table = t.trim_matches('`').to_string();
                        }
                    }
                    "where" => where_present = true,
                    "limit" => {
                        limit = step
                            .args_text
                            .first()
                            .and_then(|a| a.parse::<u64>().ok())
                            .map(LimitSpec::Literal)
                            .or(Some(LimitSpec::Variable));
                    }
                    "orderBy" => {
                        order_by = step.args_text.clone();
                    }
                    _ => {}
                }
            }
            if table.is_empty() {
                continue;
            }
            let mut stmt = PredictedStatement {
                op,
                tables: vec![TableRef::name(table)],
                projection: Projection::Unspecified,
                where_expr: if where_present {
                    Some(WhereExpr::Raw {
                        text: "<where>".into(),
                        has_interpolation: false,
                    })
                } else {
                    None
                },
                order_by,
                limit,
                offset: None,
                in_loop: chain.in_loop,
                joins: Vec::new(),
            };
            let _ = &mut stmt;
            let line = chain.steps.last().map(|s| s.line).unwrap_or(1);
            out.push(PredictedSql {
                orm: OrmKind::Generic,
                dialect: SqlDialect::Postgres,
                statements: vec![stmt],
                fidelity: vec![SqlFidelity::Partial],
                source_range: chain.byte_range.clone(),
                line,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orm::ts::build_context;
    use tree_sitter::Parser;

    fn ctx<'a>(src: &'a str) -> (PyOrmContext<'a>, tree_sitter::Tree) {
        let mut p = Parser::new();
        p.set_language(&crate::languages::typescript::language())
            .unwrap();
        let tree = p.parse(src, None).unwrap();
        let c = build_context(src, unsafe { std::mem::transmute(&tree) });
        (c, tree)
    }

    fn run_rule(rule_id: &str, src: &str) -> Vec<MatchHit> {
        let (c, _t) = ctx(src);
        let rule = DRIZZLE_RULES.iter().find(|r| r.id == rule_id).unwrap();
        (rule.matches)(&c)
    }

    #[test]
    fn drz_lmt_001_fires_on_limit_without_order() {
        let src = "db.select().from(users).limit(10);\n";
        let hits = run_rule("DRZ-LMT-001", src);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn drz_lmt_001_clean_with_orderby() {
        let src = "db.select().from(users).orderBy(users.id).limit(10);\n";
        let hits = run_rule("DRZ-LMT-001", src);
        assert!(hits.is_empty());
    }

    #[test]
    fn drz_n1_002_fires_in_loop() {
        let src = "for (const id of ids) {\n  await db.select().from(users).where(eq(users.id, id));\n}\n";
        let hits = run_rule("DRZ-N1-002", src);
        assert!(!hits.is_empty());
    }

    #[test]
    fn drz_n1_002_safe_outside_loop() {
        // Single query at module level — not N+1.
        let src = "const rows = await db.select().from(users).where(eq(users.id, 1));\n";
        let hits = run_rule("DRZ-N1-002", src);
        assert!(hits.is_empty(), "DRZ-N1-002 must NOT fire outside a loop");
    }

    #[test]
    fn drz_n1_002_safe_with_in_array_batch() {
        // `inArray` is the canonical fix for N+1 — single query for many ids.
        let src = "const rows = await db.select().from(users).where(inArray(users.id, ids));\n";
        let hits = run_rule("DRZ-N1-002", src);
        assert!(hits.is_empty(), "DRZ-N1-002 must NOT fire on inArray batch query");
    }

    #[test]
    fn shape_anchor_pg_table_fires_without_import() {
        let src = "export const users = pgTable('users', { id: serial('id').primaryKey() });\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &DRIZZLE_SHAPE));
    }

    #[test]
    fn shape_combo_db_select_from_fires_without_import() {
        let src = "const rows = await db.select().from(users);\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &DRIZZLE_SHAPE));
    }

    #[test]
    fn shape_negative_bare_select_does_not_fire() {
        // Plain `select(...)` without `db.` root must NOT trigger Drizzle.
        let src = "const x = select(User).where(eq(User.id, 1));\n";
        let (c, _t) = ctx(src);
        assert!(!matches_by_shape(&c.chains, &DRIZZLE_SHAPE));
    }

    // ─── Receiver pattern (this.db / this.drizzle) ────────────────────────
    //
    // Regression tests for the class-field shape every real-world Drizzle
    // codebase uses (Supabase RLS wrappers, repository classes, etc.).
    // Without `is_drizzle_chain` accepting root=`this` + first-step=`db`
    // these chains were silently skipped — verified on a 110-file Drizzle
    // project: zero ORM findings before the fix.

    #[test]
    fn drz_lmt_001_fires_on_this_db_limit_without_order() {
        let src = "class Store { db: any; async f() { return this.db.select().from(users).limit(10); } }\n";
        let hits = run_rule("DRZ-LMT-001", src);
        assert_eq!(hits.len(), 1, "this.db.select().limit(10) must trigger DRZ-LMT-001");
    }

    #[test]
    fn drz_n1_002_fires_on_this_db_in_loop() {
        let src = "class Store { db: any; async f(ids: number[]) { for (const id of ids) { await this.db.select().from(users).where(eq(users.id, id)); } } }\n";
        let hits = run_rule("DRZ-N1-002", src);
        assert!(!hits.is_empty(), "this.db.select().where(...) in loop must trigger DRZ-N1-002");
    }

    #[test]
    fn predict_all_emits_for_this_db_chain() {
        // `predict_all` shares the same gate, so an unbounded `this.db.select().from(t)`
        // must reach SQL-IR (SQLIR-012 lives in the cross-ORM catalog and gates on this).
        let src = "class Store { db: any; async getAll() { return this.db.select().from(appSettings); } }\n";
        let (c, _t) = ctx(src);
        let preds = DrizzleDialect.predict_all(&c);
        assert_eq!(preds.len(), 1, "this.db.select().from(t) must produce one PredictedSql");
    }
}
