//! Prisma — schema-DSL + client API (F4 family per master plan §CCC.6).
//!
//! Phase 2 v1 rules:
//! - `PRI-INC-001` — `findMany({ include: { … } })` with deep includes
//! - `PRI-N1-002` — `prisma.<model>.findUnique(...)` inside a loop
//! - `PRI-RAW-003` — `$queryRawUnsafe(`…${var}…`)` interpolated raw SQL
//! - `PRI-PAG-004` — `skip` ≥ 1000 (deep pagination)

use crate::insights::{Effort, Evidence, Severity};
use crate::orm::context::{BindingKind, CallChain, ChainRoot, PyOrmContext, TsClientKind};
use crate::orm::dialect::OrmDialect;
use crate::orm::shape::{matches_by_shape, ShapeSpec};
use crate::orm::sql_ir::{
    LimitSpec, OffsetSpec, OrmKind, PredictedSql, PredictedStatement, Projection, SqlDialect,
    SqlFidelity, SqlOp, TableRef, WhereExpr,
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

fn is_prisma_chain(chain: &CallChain, ctx: &PyOrmContext<'_>) -> bool {
    let root_text = match &chain.root {
        ChainRoot::Identifier(t) | ChainRoot::Binding(t) | ChainRoot::LoopVar(t) => t.clone(),
        _ => return false,
    };
    if matches!(root_text.as_str(), "prisma" | "db" | "client") {
        return true;
    }
    // Receiver pattern: `this.prisma.user.findMany()` arrives as root=`this`
    // with first step = the client field. Without this branch every class-field
    // wrapper around the Prisma client is silently skipped.
    if matches!(root_text.as_str(), "this" | "self") {
        if let Some(first) = chain.steps.first() {
            if matches!(first.method.as_str(), "prisma" | "db" | "client") {
                return true;
            }
        }
    }
    if let Some(b) = ctx.binding_at(&root_text, chain.byte_range.start) {
        if let BindingKind::TsClient(facts) = &b.kind {
            return matches!(facts.kind, TsClientKind::Prisma);
        }
    }
    false
}

// ─── PRI-INC-001: deep `include` nesting ────────────────────────────────

fn matches_pri_inc_001(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !is_prisma_chain(chain, ctx) {
            continue;
        }
        for step in &chain.steps {
            if !matches!(
                step.method.as_str(),
                "findMany" | "findUnique" | "findFirst"
            ) {
                continue;
            }
            for arg in &step.args_text {
                // Conservative: depth measured by balanced `include: {` count.
                let depth = include_depth(arg);
                if depth >= 3 {
                    out.push(hit(chain, "PRI-INC-001"));
                    break;
                }
            }
        }
    }
    out
}

fn include_depth(arg: &str) -> usize {
    let mut depth = 0_usize;
    let mut max = 0_usize;
    let mut i = 0;
    let bytes = arg.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"include") {
            // skip whitespace + ':' to a '{'
            let mut j = i + b"include".len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    depth += 1;
                    if depth > max {
                        max = depth;
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        if bytes[i] == b'{' {
            // unrelated brace — don't count
        }
        if bytes[i] == b'}' && depth > 0 {
            depth -= 1;
        }
        i += 1;
    }
    max
}

// ─── PRI-N1-002: findUnique / findFirst in loop ─────────────────────────

fn matches_pri_n1_002(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !chain.in_loop {
            continue;
        }
        if !is_prisma_chain(chain, ctx) {
            continue;
        }
        let last = chain
            .steps
            .last()
            .map(|s| s.method.as_str())
            .unwrap_or("");
        if matches!(last, "findUnique" | "findFirst") {
            out.push(hit(chain, "PRI-N1-002"));
        }
    }
    out
}

// ─── PRI-RAW-003: $queryRawUnsafe / $executeRawUnsafe with template ─────

fn matches_pri_raw_003(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        for step in &chain.steps {
            if matches!(
                step.method.as_str(),
                "$queryRawUnsafe" | "$executeRawUnsafe"
            ) {
                for arg in &step.args_text {
                    // Template literal with interpolation (`...${x}...`)
                    // — flag.
                    let has_template_interp =
                        arg.starts_with('`') && arg.contains("${");
                    if has_template_interp || arg.contains(" + ") {
                        out.push(hit(chain, "PRI-RAW-003"));
                        break;
                    }
                }
            }
        }
    }
    out
}

// ─── PRI-PAG-004: skip ≥ 1000 ───────────────────────────────────────────

fn matches_pri_pag_004(ctx: &PyOrmContext<'_>) -> Vec<MatchHit> {
    let mut out = Vec::new();
    for chain in &ctx.chains {
        if !is_prisma_chain(chain, ctx) {
            continue;
        }
        for step in &chain.steps {
            if !matches!(
                step.method.as_str(),
                "findMany" | "findFirst"
            ) {
                continue;
            }
            for arg in &step.args_text {
                if let Some(n) = extract_skip_value(arg) {
                    if n >= 1000 {
                        out.push(hit(chain, "PRI-PAG-004"));
                        break;
                    }
                }
            }
        }
    }
    out
}

fn extract_skip_value(arg: &str) -> Option<u64> {
    let idx = arg.find("skip")?;
    let rest = &arg[idx + 4..];
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    num.parse().ok()
}

pub const PRISMA_RULES: &[OrmRule] = &[
    OrmRule {
        id: "PRI-INC-001",
        framework: Framework::Generic,
        severity: Severity::Medium,
        effort: Effort::Medium,
        message: "Deep `include` nesting in Prisma `findMany/findUnique/findFirst` — Cartesian shape risk.",
        remediation: "Flatten to separate queries, switch a nested relation to a follow-up `findMany({where: {…IN}})`, or use `relationLoadStrategy: 'join'`.",
        confidence: 0.70,
        matches: matches_pri_inc_001,
    },
    OrmRule {
        id: "PRI-N1-002",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "`prisma.<model>.findUnique` / `findFirst` inside a loop — N+1 round-trips.",
        remediation: "Collect ids and call `findMany({ where: { id: { in: ids } } })` once.",
        confidence: 0.90,
        matches: matches_pri_n1_002,
    },
    OrmRule {
        id: "PRI-RAW-003",
        framework: Framework::Generic,
        severity: Severity::High,
        effort: Effort::Small,
        message: "Template-literal interpolation inside `$queryRawUnsafe` / `$executeRawUnsafe` — SQL injection.",
        remediation: "Use `prisma.$queryRaw\\`SELECT … ${value}\\`` (tagged template) so Prisma parameterizes.",
        confidence: 0.95,
        matches: matches_pri_raw_003,
    },
    OrmRule {
        id: "PRI-PAG-004",
        framework: Framework::Generic,
        severity: Severity::Medium,
        effort: Effort::Medium,
        message: "Prisma `skip` ≥1000 — Postgres must scan and discard every prior row.",
        remediation: "Use cursor-based pagination (`cursor: { id: lastSeen }, take: N`).",
        confidence: 0.95,
        matches: matches_pri_pag_004,
    },
];

// ─── PrismaDialect — predict_all ────────────────────────────────────────

/// Shape-based fallback for Prisma. The Prisma client is usually
/// imported directly, but factory wrappers (`getPrisma()`) and global
/// singletons exported from a `db.ts` module hide the import in the
/// leaf file.
///
/// Two evidence classes:
///   1. **Dollar-prefixed methods** (`$queryRaw`, `$executeRaw`, …) —
///      uniquely Prisma; one occurrence proves usage.
///   2. **Prisma-exclusive verb spellings** (`findMany`, `findUnique`,
///      `findUniqueOrThrow`, `findFirstOrThrow`, `createMany`,
///      `groupBy`, `upsert`) — TypeORM/Sequelize/Mongoose use different
///      names (`find`, `findOne`, `findOneOrFail`, `findAll`, …).
///
/// We deliberately do NOT use a `ModuleAttrEquals` combo: the TS
/// chain reconstructor records `prisma.user.findMany()` as
/// `ChainRoot::Identifier("prisma")` with methods `["user",
/// "findMany"]`, not as a `ModuleAttr` root. The anchor list is
/// sufficient because every Prisma chain ends in one of these verbs.
pub(crate) const PRISMA_SHAPE: ShapeSpec = ShapeSpec {
    anchors: &[
        "$queryRaw",
        "$queryRawUnsafe",
        "$executeRaw",
        "$executeRawUnsafe",
        "$transaction",
        "$connect",
        "$disconnect",
        "findUniqueOrThrow",
        "findFirstOrThrow",
        "findUnique",
        "findMany",
        "createMany",
        "upsert",
        "groupBy",
    ],
    combos: &[],
};

pub struct PrismaDialect;

impl OrmDialect for PrismaDialect {
    fn orm(&self) -> OrmKind {
        OrmKind::Generic
    }

    fn matches(&self, ctx: &PyOrmContext<'_>) -> bool {
        ctx.imports.has_any_starting_with("@prisma/client")
            || ctx.imports.has_any_starting_with("@prisma")
            || ctx.imports.has_any_starting_with("prisma")
            || ctx
                .imports
                .aliases
                .keys()
                .any(|k| k == "PrismaClient")
            || ctx
                .imports
                .modules
                .values()
                .flatten()
                .any(|v| v == "PrismaClient")
            || matches_by_shape(&ctx.chains, &PRISMA_SHAPE)
    }

    fn predict_all(&self, ctx: &PyOrmContext<'_>) -> Vec<PredictedSql> {
        let mut out = Vec::new();
        for chain in &ctx.chains {
            if !is_prisma_chain(chain, ctx) {
                continue;
            }
            let methods: Vec<&str> = chain.steps.iter().map(|s| s.method.as_str()).collect();
            let model = methods.first().map(|s| s.to_string()).unwrap_or_default();
            let op_method = methods.get(1).copied().unwrap_or("");
            let op = match op_method {
                "findMany" | "findUnique" | "findFirst" | "count" => SqlOp::Select,
                "create" | "createMany" => SqlOp::Insert,
                "update" | "updateMany" => SqlOp::Update,
                "delete" | "deleteMany" => SqlOp::Delete,
                _ => continue,
            };
            let mut stmt = PredictedStatement {
                op,
                tables: vec![TableRef::name(model.clone())],
                projection: if op_method == "count" {
                    Projection::Aggregate("COUNT(*)".into())
                } else {
                    Projection::Full
                },
                ..Default::default()
            };
            stmt.in_loop = chain.in_loop;
            if op_method == "findUnique" {
                stmt.limit = Some(LimitSpec::Literal(1));
            }
            let mut fidelity = SqlFidelity::Partial; // most Prisma args are objects → Partial
            // Pull where/take/skip from second-step args.
            if let Some(args0) = chain.steps.get(1).and_then(|s| s.args_text.first()) {
                if let Some(t) = extract_take(args0) {
                    stmt.limit = Some(LimitSpec::Literal(t));
                }
                if let Some(s) = extract_skip_value(args0) {
                    stmt.offset = Some(OffsetSpec::Literal(s));
                }
                if args0.contains("where") {
                    stmt.where_expr = Some(WhereExpr::Raw {
                        text: "<where>".into(),
                        has_interpolation: false,
                    });
                }
                if !args0.contains('$') && !args0.contains("${") {
                    fidelity = SqlFidelity::Partial;
                }
            }
            let line = chain.steps.last().map(|s| s.line).unwrap_or(1);
            out.push(PredictedSql {
                orm: OrmKind::Generic,
                dialect: SqlDialect::Postgres,
                statements: vec![stmt],
                fidelity: vec![fidelity],
                source_range: chain.byte_range.clone(),
                line,
            });
        }
        out
    }
}

fn extract_take(arg: &str) -> Option<u64> {
    let idx = arg.find("take")?;
    let rest = &arg[idx + 4..];
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
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
        let rule = PRISMA_RULES.iter().find(|r| r.id == rule_id).unwrap();
        (rule.matches)(&c)
    }

    #[test]
    fn pri_raw_003_fires_on_template_interp() {
        let src = "prisma.$queryRawUnsafe(`SELECT * FROM users WHERE name = ${name}`);\n";
        let hits = run_rule("PRI-RAW-003", src);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn pri_pag_004_fires_on_deep_skip() {
        let src = "prisma.user.findMany({ skip: 5000, take: 50 });\n";
        let hits = run_rule("PRI-PAG-004", src);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn pri_n1_002_fires_in_for_loop() {
        let src = "for (const id of ids) {\n  await prisma.user.findUnique({ where: { id } });\n}\n";
        let hits = run_rule("PRI-N1-002", src);
        assert!(!hits.is_empty(), "findUnique in for-of must fire N+1");
    }

    #[test]
    fn pri_n1_002_does_not_fire_outside_loop() {
        let src = "await prisma.user.findUnique({ where: { id: 1 } });\n";
        let hits = run_rule("PRI-N1-002", src);
        assert!(hits.is_empty());
    }

    #[test]
    fn pri_n1_002_fires_inside_for_each() {
        // Array-method callbacks register as loop bodies in TS.
        let src = "ids.forEach(async (id) => { await prisma.user.findUnique({ where: { id } }); });\n";
        let hits = run_rule("PRI-N1-002", src);
        assert!(!hits.is_empty(), "findUnique inside .forEach must fire N+1");
    }

    #[test]
    fn pri_n1_002_does_not_fire_on_find_many_bulk() {
        // The correct batch form — single round-trip.
        let src = "await prisma.user.findMany({ where: { id: { in: ids } } });\n";
        let hits = run_rule("PRI-N1-002", src);
        assert!(hits.is_empty(), "findMany with `in` is the fix; must not fire");
    }

    #[test]
    fn pri_inc_001_fires_on_deep_include() {
        let src = "await prisma.user.findMany({ include: { posts: { include: { comments: { include: { author: true } } } } } });\n";
        let hits = run_rule("PRI-INC-001", src);
        assert!(!hits.is_empty(), "deep include nesting must fire");
    }

    #[test]
    fn dialect_matches_when_prisma_imported() {
        let src = "import { PrismaClient } from '@prisma/client';\nconst prisma = new PrismaClient();\n";
        let (c, _t) = ctx(src);
        assert!(PrismaDialect.matches(&c));
    }

    #[test]
    fn shape_anchor_dollar_query_raw_fires_without_import() {
        // Factory-wrapped client, no `@prisma/client` import in this file.
        let src = "const rows = await db.$queryRaw(`SELECT 1`);\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &PRISMA_SHAPE));
        assert!(PrismaDialect.matches(&c));
    }

    #[test]
    fn shape_anchor_find_many_fires_without_import() {
        // `findMany` is a Prisma-exclusive verb spelling — no other ORM uses it.
        let src = "const users = await db.user.findMany({ where: { active: true } });\n";
        let (c, _t) = ctx(src);
        assert!(matches_by_shape(&c.chains, &PRISMA_SHAPE));
    }

    #[test]
    fn shape_negative_typeorm_find_one_does_not_fire() {
        // TypeORM-style `find()` / `findOne()` must NOT register as Prisma.
        let src = "const u = await repo.findOne({ where: { id: 1 } });\n";
        let (c, _t) = ctx(src);
        assert!(!matches_by_shape(&c.chains, &PRISMA_SHAPE));
    }

    // Regression: `this.prisma.user.findMany()` is the class-field shape
    // every NestJS / repository codebase uses. Tree-sitter splits the
    // receiver into root=`this` + first step=`prisma`; before the fix the
    // chain gate rejected it and PRI-INC-001 silently skipped these chains.
    #[test]
    fn is_prisma_chain_accepts_this_prisma() {
        let src = "class UserService { async list() { return this.prisma.user.findMany({ include: { posts: { include: { comments: { include: { author: true } } } } } }); } }\n";
        let hits = run_rule("PRI-INC-001", src);
        assert!(!hits.is_empty(), "this.prisma.user.findMany must trigger PRI-INC-001");
    }
}
