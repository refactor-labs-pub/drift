//! GORM — Go's most-popular ORM. Phase 4 v1 rules:
//! - `GORM-N1-001` — `db.First(&x, id)` / `db.Find(&x).First()` in loop
//! - `GORM-RAW-002` — `db.Raw(fmt.Sprintf(...))` — `%s` interpolation
//! - `GORM-AUTO-003` — `db.AutoMigrate(...)` at top-level / in `main`
//! - `GORM-SAVE-004` — `db.Create(&x)` / `db.Save(&x)` in loop

use crate::insights::{Effort, Evidence, Severity};
use crate::orm::context::{CallChain, ChainRoot, PyOrmContext};
use crate::orm::dialect::OrmDialect;
use crate::orm::shape::{matches_by_shape, ComboRule, RootPredicate, ShapeSpec};
use crate::orm::sql_ir::{
    OrmKind, PredictedSql, PredictedStatement, Projection, SqlDialect, SqlFidelity, SqlOp,
    TableRef,
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

fn root_text(chain: &CallChain) -> String {
    match &chain.root {
        ChainRoot::Identifier(t) | ChainRoot::Binding(t) | ChainRoot::LoopVar(t) => t.clone(),
        _ => String::new(),
    }
}

fn is_db_chain(chain: &CallChain) -> bool {
    let r = root_text(chain);
    // Direct identifier: `db.First(…)`, `tx.Find(…)`, `myDB.Where(…)`.
    if r == "db" || r == "DB" || r == "tx" || r.ends_with("DB") {
        return true;
    }
    // Receiver pattern: `r.db.First(…)` / `s.DB.Find(…)` /
    // `h.db.Where(…)`. Tree-sitter gives root=`r` with first step =
    // `db`. This is the canonical "repository / service with a *gorm.DB
    // field" shape used by virtually every real-world Go project; the
    // old `trim_start_matches("h.")` heuristic never matched it because
    // selector_expression splits the receiver and field into separate
    // chain steps.
    if let Some(first) = chain.steps.first() {
        let f = first.method.as_str();
        if f == "db" || f == "DB" || f == "tx" || f.ends_with("DB") {
            return true;
        }
    }
    false
}

fn matches_gorm_n1_001(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !chain.in_loop {
            continue;
        }
        if !is_db_chain(chain) {
            continue;
        }
        let last = chain
            .steps
            .last()
            .map(|s| s.method.as_str())
            .unwrap_or("");
        if matches!(last, "First" | "Take" | "Last" | "FirstOrInit" | "FirstOrCreate") {
            out.push(hit(chain, "GORM-N1-001"));
        }
    }
    out
}


fn matches_gorm_raw_002(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        for step in &chain.steps {
            if !matches!(step.method.as_str(), "Raw" | "Exec") {
                continue;
            }
            for arg in &step.args_text {
                // Detect fmt.Sprintf inside arg, OR `+` concat with non-literal.
                if arg.contains("fmt.Sprintf")
                    || arg.contains("fmt.Sprint")
                    || arg.contains("\" + ")
                    || arg.contains(" + \"")
                {
                    out.push(hit(chain, "GORM-RAW-002"));
                    break;
                }
            }
        }
    }
    out
}

fn matches_gorm_auto_003(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !is_db_chain(chain) {
            continue;
        }
        let last = chain
            .steps
            .last()
            .map(|s| s.method.as_str())
            .unwrap_or("");
        if last == "AutoMigrate" {
            out.push(hit(chain, "GORM-AUTO-003"));
        }
    }
    out
}

fn matches_gorm_save_004(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !chain.in_loop {
            continue;
        }
        if !is_db_chain(chain) {
            continue;
        }
        let last = chain
            .steps
            .last()
            .map(|s| s.method.as_str())
            .unwrap_or("");
        if matches!(last, "Create" | "Save" | "Update" | "Updates") {
            out.push(hit(chain, "GORM-SAVE-004"));
        }
    }
    out
}

pub const GORM_RULES: &[OrmRule] = &[
    OrmRule {
        id: "GORM-N1-001",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "GORM `db.First/Take/Last(...)` inside a loop — N+1 queries.",
        remediation: "Collect ids and use `db.Where(\"id IN ?\", ids).Find(&users)` once.",
        confidence: 0.90,
        matches: matches_gorm_n1_001,
    },
    OrmRule {
        id: "GORM-RAW-002",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "GORM `db.Raw(fmt.Sprintf(...))` — formatted SQL is injection.",
        remediation: "Use parameter placeholders: `db.Raw(\"SELECT * FROM u WHERE name = ?\", name)`.",
        confidence: 0.95,
        matches: matches_gorm_raw_002,
    },
    OrmRule {
        id: "GORM-AUTO-003",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Medium,
        message: "GORM `db.AutoMigrate(...)` runs at boot — can drop columns and lock tables in prod.",
        remediation: "Run migrations as a separate offline step (golang-migrate, atlas) gated by deploy.",
        confidence: 0.95,
        matches: matches_gorm_auto_003,
    },
    OrmRule {
        id: "GORM-SAVE-004",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "GORM `db.Create(&x)` / `Save(&x)` inside a loop — one INSERT per row.",
        remediation: "Build a slice and call `db.Create(&users)` (GORM batches automatically).",
        confidence: 0.90,
        matches: matches_gorm_save_004,
    },
];

/// Shape-based fallback for GORM. Anchors target GORM-unique methods:
/// `AutoMigrate`, `Preload`, `Joins`, `FirstOrCreate`, `FirstOrInit`,
/// `Scopes`, `Pluck`. Most chain methods (`Where`, `Find`, `Save`) are
/// too generic for an anchor; the combo gates on root binding being
/// `db` / `tx` to disambiguate from custom builders.
pub(crate) const GORM_SHAPE: ShapeSpec = ShapeSpec {
    anchors: &[
        "AutoMigrate",
        "Preload",
        "Joins",
        "FirstOrCreate",
        "FirstOrInit",
        "Scopes",
        "Pluck",
        "Omit",
    ],
    combos: &[
        ComboRule {
            first_method: "Where",
            root: RootPredicate::Equals("db"),
            continuation_any: &["Find", "First", "Take", "Last", "Count"],
        },
        ComboRule {
            first_method: "Where",
            root: RootPredicate::Equals("tx"),
            continuation_any: &["Find", "First", "Take", "Last", "Count"],
        },
        ComboRule {
            first_method: "Model",
            root: RootPredicate::Equals("db"),
            continuation_any: &["Updates", "Update", "Where"],
        },
    ],
};

pub struct GormDialect;

impl OrmDialect for GormDialect {
    fn orm(&self) -> OrmKind {
        OrmKind::Generic
    }

    fn matches(&self, ctx: &PyOrmContext<'_>) -> bool {
        ctx.imports
            .modules
            .keys()
            .any(|m| m.contains("gorm.io") || m.contains("jinzhu/gorm"))
            || matches_by_shape(&ctx.chains, &GORM_SHAPE)
    }

    fn predict_all(&self, ctx: &PyOrmContext<'_>) -> Vec<PredictedSql> {
        let mut out = Vec::new();
        for chain in &ctx.chains {
            if !is_db_chain(chain) {
                continue;
            }
            let last = chain
                .steps
                .last()
                .map(|s| s.method.as_str())
                .unwrap_or("");
            let op = match last {
                "First" | "Take" | "Last" | "Find" | "Scan" | "Pluck" => SqlOp::Select,
                "Update" | "Updates" => SqlOp::Update,
                "Delete" | "Unscoped" => SqlOp::Delete,
                // For Create/Save we'd want a column-list aware emitter
                // before triggering SQLIR-003. Phase 1: don't predict
                // for these — the GORM-SAVE-004 rule covers them.
                _ => continue,
            };
            let stmt = PredictedStatement {
                op,
                tables: vec![TableRef::name("<gorm>".to_string())],
                projection: Projection::Unspecified,
                in_loop: chain.in_loop,
                ..Default::default()
            };
            let line = chain.steps.last().map(|s| s.line).unwrap_or(1);
            out.push(PredictedSql {
                orm: OrmKind::Generic,
                dialect: SqlDialect::Postgres,
                statements: vec![stmt],
                fidelity: vec![SqlFidelity::Skeletal],
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
    use crate::orm::go::build_context;
    use tree_sitter::Parser;

    fn ctx<'a>(src: &'a str) -> (PyOrmContext<'a>, tree_sitter::Tree) {
        let mut p = Parser::new();
        p.set_language(&crate::languages::go::language()).unwrap();
        let tree = p.parse(src, None).unwrap();
        let c = build_context(src, unsafe { std::mem::transmute(&tree) });
        (c, tree)
    }

    fn run_rule(rule_id: &str, src: &str) -> Vec<MatchHit> {
        let (c, _t) = ctx(src);
        let rule = GORM_RULES.iter().find(|r| r.id == rule_id).unwrap();
        (rule.matches)(&c)
    }

    #[test]
    fn gorm_n1_001_fires_in_for_range() {
        let src = "package main\nfunc f(db *Gorm, ids []int64) {\nfor _, id := range ids {\n  var u User\n  db.First(&u, id)\n}\n}\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(!hits.is_empty(), "GORM First in range loop must fire");
    }

    #[test]
    fn gorm_n1_001_safe_outside_loop() {
        // Single First at top level — not N+1.
        let src = "package main\nfunc f(db *Gorm, id int64) {\n  var u User\n  db.First(&u, id)\n}\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(hits.is_empty(), "GORM-N1-001 must NOT fire outside a loop");
    }

    #[test]
    fn gorm_n1_001_safe_with_find_in_batch() {
        // `db.Find(&users, ids)` is the canonical batched fix for N+1.
        let src = "package main\nfunc f(db *Gorm, ids []int64) {\n  var users []User\n  db.Find(&users, ids)\n}\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(hits.is_empty(), "GORM-N1-001 must NOT fire on batched Find");
    }

    #[test]
    fn gorm_raw_002_fires_on_sprintf() {
        let src = "package main\nfunc f(db *Gorm, name string) {\ndb.Raw(fmt.Sprintf(\"SELECT * FROM u WHERE name='%s'\", name))\n}\n";
        let hits = run_rule("GORM-RAW-002", src);
        assert!(!hits.is_empty());
    }

    #[test]
    fn gorm_auto_003_fires_on_automigrate() {
        let src = "package main\nfunc main() { db.AutoMigrate(&User{}) }\n";
        let hits = run_rule("GORM-AUTO-003", src);
        assert!(!hits.is_empty());
    }

    #[test]
    fn gorm_save_004_fires_in_loop() {
        let src = "package main\nfunc f(db *Gorm, us []User) {\nfor _, u := range us {\n  db.Create(&u)\n}\n}\n";
        let hits = run_rule("GORM-SAVE-004", src);
        assert!(!hits.is_empty());
    }

    #[test]
    fn shape_anchor_auto_migrate_fires_without_import() {
        let src = "package main\nfunc f(db *Gorm) { db.AutoMigrate(&User{}) }\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &GORM_SHAPE));
    }

    #[test]
    fn shape_combo_db_where_find_fires() {
        let src = "package main\nfunc f(db *Gorm) { var u User; db.Where(\"id = ?\", 1).Find(&u) }\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &GORM_SHAPE));
    }

    #[test]
    fn shape_negative_unrelated_where_does_not_fire() {
        // Custom builder with `xs.Where(...)` — lowercase non-`db`/`tx` root.
        let src = "package main\nfunc f(xs []int) { xs.Where(\"x\") }\n";
        let (c, _t) = ctx(src);
        assert!(!matches_by_shape(&c.chains, &GORM_SHAPE));
    }

    // ─── GORM N+1 — additional coverage ──────────────────────────────────

    #[test]
    fn gorm_n1_001_does_not_fire_outside_loop() {
        // `db.First(&u, id)` at the top level is a single query — not N+1.
        let src = "package main\nfunc f(db *Gorm) { var u User; db.First(&u, 1) }\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(hits.is_empty(), "GORM-N1-001 must not fire outside a loop");
    }

    #[test]
    fn gorm_n1_001_does_not_fire_on_bulk_find() {
        // `db.Where("id IN ?", ids).Find(&users)` is the correct bulk pattern.
        let src = "package main\nfunc f(db *Gorm, ids []int64) { var us []User; db.Where(\"id IN ?\", ids).Find(&us) }\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(hits.is_empty(), "GORM-N1-001 must not fire on a single bulk Find call");
    }

    #[test]
    fn gorm_n1_001_fires_on_tx_first_in_loop() {
        // `tx.First(&u, id)` inside a loop — same N+1 pattern as `db`.
        let src = "package main\nfunc f(tx *Gorm, ids []int64) {\nfor _, id := range ids {\n  var u User\n  tx.First(&u, id)\n}\n}\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(!hits.is_empty(), "GORM-N1-001 must fire on tx.First in loop");
    }

    #[test]
    fn gorm_save_004_does_not_fire_outside_loop() {
        // Single `db.Create(&u)` is fine — only the in-loop variant is N+1.
        let src = "package main\nfunc f(db *Gorm) { var u User; db.Create(&u) }\n";
        let hits = run_rule("GORM-SAVE-004", src);
        assert!(hits.is_empty(), "GORM-SAVE-004 must not fire on a single Create call");
    }

    // ─── Receiver pattern (r.db / s.DB / repo.db) ────────────────────────
    //
    // Regression: the canonical "repository struct with a *gorm.DB field"
    // shape every real-world Go project uses. Tree-sitter renders
    // `r.db.First(&u, id)` as root=`r` with first step `db`. The old
    // `trim_start_matches("h.")` heuristic never matched it because the
    // selector splits the receiver and field into separate chain steps.

    #[test]
    fn gorm_n1_001_fires_on_receiver_db_in_loop() {
        let src = "package main\nfunc (r *UserRepo) Load(ids []int64) {\nfor _, id := range ids {\n  var u User\n  r.db.First(&u, id)\n}\n}\n";
        let hits = run_rule("GORM-N1-001", src);
        assert!(!hits.is_empty(), "r.db.First in a loop must trigger GORM-N1-001");
    }

    #[test]
    fn gorm_save_004_fires_on_receiver_db_create_in_loop() {
        let src = "package main\nfunc (s *Service) Bulk(us []User) {\nfor _, u := range us {\n  s.db.Create(&u)\n}\n}\n";
        let hits = run_rule("GORM-SAVE-004", src);
        assert!(!hits.is_empty(), "s.db.Create in a loop must trigger GORM-SAVE-004");
    }

    #[test]
    fn gorm_auto_003_fires_on_receiver_db_automigrate() {
        let src = "package main\nfunc (a *App) Init() { a.DB.AutoMigrate(&User{}) }\n";
        let hits = run_rule("GORM-AUTO-003", src);
        assert!(!hits.is_empty(), "a.DB.AutoMigrate must trigger GORM-AUTO-003");
    }
}
