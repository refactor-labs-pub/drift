# Python ORM Static Analysis — Implementation Plan

Add purely static, tree-sitter-backed detectors for Django, SQLAlchemy, and Alembic to `drift-static-profiler`. No runtime hooks, no Python interpreter, no `pip install` of the target project.

Target layout (per user request):

```
src/orm/
  mod.rs                    — dispatcher; declares the OrmRule trait + Catalog
  python/
    mod.rs                  — Python-specific shared scaffolding (binding map, loop-detector, queryset-id propagation)
    django.rs               — Django ORM rule catalog + matchers
    sqlalchemy.rs           — SQLAlchemy 1.x/2.x rule catalog + matchers
    alembic.rs              — Alembic migration safety rule catalog
```

The wider repo already has [research/ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md](ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md) and [research/MIGRATION_AST_SHAPES.md](MIGRATION_AST_SHAPES.md) which enumerate the antipatterns; this plan covers **how to detect them statically with tree-sitter + Rust**, not what to detect.

---

## 1 — Why purely static, and what that buys / costs us

### Buys
- Zero install of the user's project. We parse files directly — no Django settings module, no SQLAlchemy `MetaData`.
- Runs on any branch / PR / dead code. Runtime tools (`nplusone`, `django-silk`, `dogpile.cache` hooks) only see code that executes.
- O(LOC) — single tree-sitter pass per file, shared with the existing pipeline. We pay zero extra parse cost.

### Costs
- No real type inference. `def foo(qs)` — we can't know `qs` is a QuerySet without callers.
- Aliases and dynamic attribute access (`getattr(obj, name)`) defeat us. We accept this and report `confidence < 1.0`.
- Cross-file model graph (Django `ForeignKey`/SQLAlchemy `relationship()`) requires a second indexing pass — feasible but not in v1.

### Prior art benchmark
- **richardhapb/django-check** (Rust, LSP) is the closest analog. Crates: `parser` / `ir` (ModelGraph) / `passes` / `diagnostic`. Does **not** use tree-sitter — uses its own Python parser. Builds a project-wide ModelGraph. We will follow the same architecture (parser → IR → passes → diagnostic), but reuse tree-sitter as the parser and the existing `Finding` shape as the diagnostic.
- **nplusone** (Python, runtime). Reference for the *what*. We replicate its findings shape statically.
- **Squawk** (Rust, Postgres migration linter). Reference for Alembic rules. It parses raw SQL DDL; we operate at the Python-call layer (`op.add_column`, `op.alter_column`) so we catch things *before* they hit raw SQL.
- **Semgrep django rules**. Pattern-based. We will mine their YAML for low-cost rules (column-trim, exists-vs-count) but rebuild as Rust predicates since tree-sitter queries are 10–100× faster than Semgrep's pattern matcher.

---

## 2 — The static analysis pipeline

```
tree-sitter parse  ──►  per-file capture (tags.rs)  ──►  per-file ORM context  ──►  rule pass  ──►  Finding
        (already)         (already; widen captures)         (new)                  (new)         (existing)
```

### 2.1 Widen the tree-sitter captures

`src/languages/python.rs` already captures `@def.*`, `@ref.*`, `@import.*`, `@ref.sql_literal`. We add three new ORM-specific captures, scoped via predicates so non-ORM files cost zero:

```scheme
; Variable assignment whose RHS is a call — used by binding-map.
; e.g.  qs = User.objects.filter(...)
(assignment
  left: (identifier) @bind.lhs
  right: (call
    function: [(identifier) @bind.rhs.fn
               (attribute attribute: (identifier) @bind.rhs.method)])) @bind.assign

; For-loop iterator + body — used to find loops over querysets.
(for_statement
  left: (_) @loop.var
  right: (_) @loop.iter
  body: (_) @loop.body) @loop.stmt

; Decorator over a function — used for @transaction.atomic, @cache, etc.
(decorated_definition
  (decorator (_) @decorator.expr)
  definition: (function_definition name: (identifier) @decorator.fn)) @decorator.site
```

These captures are **language-agnostic shapes** (loops, bindings, decorators) — Java/Go/TS detectors will reuse them later, so they belong in `python.rs` for now and graduate to `languages/python.rs` proper.

### 2.2 Per-file ORM context

New struct in `src/orm/python/mod.rs`:

```rust
pub struct PyOrmContext<'a> {
    pub file: &'a str,
    pub imports: ImportMap,           // dotted_name → alias OR alias → fqn
    pub bindings: BindingMap,         // var_name → BindingKind
    pub for_loops: Vec<LoopRange>,    // byte-range + loop-var bindings
    pub class_defs: Vec<ClassDef>,    // for Django Model / SQLAlchemy declarative_base subclasses
    pub references: &'a [Reference],  // shared with the rest of drift-static-profiler
}

pub enum BindingKind {
    DjangoQuerySet { model: Option<String> },   // qs = User.objects.filter(...)
    DjangoManager  { model: Option<String> },   // mgr = User.objects
    DjangoModelInst{ model: Option<String> },   // u = qs.first()
    SaSelect        { entity: Option<String> }, // stmt = select(User)
    SaSession,                                  // sess = Session(); sess = sessionmaker()()
    AlembicOp,                                  // imported as `from alembic import op`
    Unknown,
}
```

**Inference rules** (cheap; no SAT, no fixpoint):
- `X.objects.filter(...)` / `X.objects.all()` / `X.objects.exclude(...)` etc → DjangoQuerySet{model=X}
- `<DjangoQuerySet>.filter(...)` / `.exclude(...)` / `.annotate(...)` / `.select_related(...)` / `.prefetch_related(...)` → DjangoQuerySet (propagate model)
- `<DjangoQuerySet>.first()` / `.get(...)` / `.last()` → DjangoModelInst (propagate model)
- `select(X)` (imported from `sqlalchemy`) → SaSelect{entity=X}
- `from alembic import op` → bindings["op"] = AlembicOp

**Loop binding propagation:**
- `for u in qs:` where bindings["qs"] == DjangoQuerySet{model=User} ⇒ inside loop body, bindings["u"] = DjangoModelInst{model=User}
- Loop body scope = byte-range of the `body: (_) @loop.body` capture. References within that range carry an `in_loop: bool` flag (already on `ExternalCall`).

### 2.3 Rule catalog — `OrmRule` trait

Mirror the existing `SqlRule` shape (`src/sql_lint.rs:67`):

```rust
pub struct OrmRule {
    pub id: &'static str,
    pub framework: Framework,           // Django | SqlAlchemy | Alembic | Generic
    pub severity: Severity,
    pub effort: Effort,
    pub message: &'static str,
    pub remediation: &'static str,
    pub matches: fn(&PyOrmContext, &Reference) -> Option<MatchHit>,
}

pub struct MatchHit {
    pub line: usize,
    pub confidence: f64,
    pub extra_evidence: Vec<Evidence>,
}
```

Per-framework catalogs live next to their matchers:
- `django.rs`: `pub const DJANGO_RULES: &[OrmRule] = &[...]`
- `sqlalchemy.rs`: `pub const SQLALCHEMY_RULES: &[OrmRule] = &[...]`
- `alembic.rs`: `pub const ALEMBIC_RULES: &[OrmRule] = &[...]`

The dispatcher in `orm/mod.rs` concatenates them (Open/Closed; same pattern as `sql_lint.rs`'s `BUILTIN_RULES`).

### 2.4 Plug-in points (file:line in current repo)

| Where | What |
|---|---|
| [src/languages/python.rs:18](../src/languages/python.rs#L18) `TAGS_QUERY` | append the 3 new captures (`@bind.*`, `@loop.*`, `@decorator.*`) |
| [src/tags.rs:308](../src/tags.rs#L308) `extract_sql_string` neighborhood | extend the capture-name match to populate a new `BindingHint` field on `Reference` |
| [src/insights.rs:518](../src/insights.rs#L518) `collect_node_findings` | add `out.extend(orm::python::collect_findings(sym, ctx));` |
| [src/report.rs:226](../src/report.rs#L226) attach-pass region | (optional, only if we go cross-file) attach a project-wide ModelGraph pass |
| [Cargo.toml:50](../Cargo.toml#L50) `[dependencies]` | nothing new — we reuse `tree-sitter-python`, `sqlparser`, `serde_json` |
| [schema/profile.schema.json:434](../schema/profile.schema.json) `FindingKind` enum | append `"django_antipattern"`, `"sqlalchemy_antipattern"`, `"alembic_migration"` (or fold all under a single `"orm_antipattern"` and use `evidence[0].call` to carry the rule id) — see §6 |

---

## 3 — v1 rule set (high-precision, low-FP)

This is the subset of [ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md](ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md) we ship first. Each rule is purely syntactic on tree-sitter captures + the `PyOrmContext` binding map — no cross-file resolution needed.

### 3.1 Django (target: 12 rules in v1)

| ID | Rule | Detection shape | Conf |
|---|---|---|---|
| `DJ-N1-001` | Iterating a queryset and accessing `x.related` without `select_related` / `prefetch_related` | `for x in qs:` ∧ `x.<attr>.<deeper>` in loop body ∧ `qs` chain has no `select_related`/`prefetch_related`/`only` mentioning `<attr>` | 0.85 |
| `DJ-N1-002` | `.count()` followed by another use of the same queryset | `qs.count()` ∧ later `for _ in qs:` or `qs.filter(...)` in same function — issues two queries | 0.70 |
| `DJ-N1-003` | `len(qs)` instead of `qs.count()` | `call function:(identifier "len") arguments:(qs_binding)` | 0.95 |
| `DJ-N1-004` | `qs.count() > 0` / `qs.count() == 0` instead of `qs.exists()` | `comparison_operator` LHS=`<qs>.count()` RHS=literal 0 | 0.95 |
| `DJ-N1-005` | `bool(qs)` / `if qs:` truthy-check forces full evaluation | `if` whose condition is a queryset identifier directly | 0.80 |
| `DJ-PERF-006` | `obj.save()` in a loop (use `bulk_update` / `bulk_create`) | call `<DjangoModelInst>.save()` ∧ `in_loop=true` | 0.85 |
| `DJ-PERF-007` | `Model.objects.create(...)` in a loop (use `bulk_create`) | call `<DjangoManager>.create(...)` ∧ `in_loop=true` | 0.90 |
| `DJ-PERF-008` | `.update(...)` per-row vs `qs.update(...)` | `<DjangoModelInst>.<attr> = ...` then `.save()` in loop | 0.65 |
| `DJ-EAGER-009` | `qs.iterator()` after `prefetch_related` | both calls in same chain — `prefetch_related` is silently dropped pre-4.1 | 0.85 |
| `DJ-PROJ-010` | `qs.values('m2m_field')` triggers cartesian | `values()`/`values_list()` arg matches a relation name in same-file Model | 0.55 (needs ModelGraph for 0.95) |
| `DJ-RAW-011` | `Model.objects.raw(f"...{user}")` or `extra(where=f"...")` — SQLi vector | f-string / `%` formatting inside `raw()`/`extra()` args | 0.95 |
| `DJ-PAG-012` | `Paginator(qs, ...)` over very deep page | `OFFSET` heuristic — flag `Paginator` with a `.page(...)` call whose arg is not bounded | 0.40 (advisory) |

### 3.2 SQLAlchemy (target: 10 rules in v1)

| ID | Rule | Detection shape | Conf |
|---|---|---|---|
| `SA-N1-001` | Iteration over `session.scalars(...).all()` then `.<rel>` access — N+1 default | for-loop body accesses `<row>.<rel>` ∧ no `joinedload`/`selectinload`/`raiseload` in chain | 0.80 |
| `SA-N1-002` | `joinedload(rel)` on a *-to-many collection | argument to `joinedload` matches a relationship marked `uselist=True` / typed `Mapped[List[...]]` in same file | 0.70 |
| `SA-N1-003` | `yield_per(N)` combined with `joinedload(...)` / `subqueryload(...)` | both options on same `Select` chain | 0.95 |
| `SA-PERF-004` | `Query.with_entities(...)` followed by `.all()` then `len(...)` | use `func.count()` instead | 0.85 |
| `SA-PERF-005` | Single-row `session.execute(select(X).where(X.id == id))` in a loop — should be `IN (...)` | `select(...).where(... == <loop_var>...)` ∧ in_loop | 0.85 |
| `SA-DTO-006` | `select(User)` to read 1–2 cols then projected — should be `select(User.id, User.name)` | full-entity select followed only by `.id`/`.name` access | 0.55 |
| `SA-SESS-007` | `session.add(obj)` in a loop without batched `flush` / `commit` outside loop | `session.add` ∧ in_loop ∧ no `session.flush` outside loop in same fn | 0.65 |
| `SA-LAZY-008` | `relationship(..., lazy="dynamic")` field used via `len()`/`list()`/iteration | dynamic relationships return AppenderQuery; `len()` triggers COUNT, `list()` loads all | 0.85 |
| `SA-EXEC-009` | `session.execute(text(f"...{x}"))` — SQLi-shaped + bypasses ORM | f-string inside `text(...)` first arg | 0.95 |
| `SA-AUTO-010` | `autoflush=True` (default) within a hot loop — flushes per query | session created with `autoflush=False` is fine; flag when long for-loop contains both `session.add` and `session.execute(select(...))` | 0.55 |

### 3.3 Alembic (target: 8 rules in v1)

Alembic migrations are individual Python files in `<project>/alembic/versions/*.py` shaped as `def upgrade(): ... def downgrade(): ...`. Each `op.*` call maps to DDL. Rule catalog mirrors Squawk's at the Python layer.

| ID | Rule | Detection shape | Conf |
|---|---|---|---|
| `AL-NULL-001` | `op.add_column(..., nullable=False)` without a preceding `op.execute(...)` UPDATE / data-migration in same `upgrade()` | order matters: scan for `add_column` with `nullable=False` and no prior `op.execute` / `op.bulk_insert` in fn body | 0.85 |
| `AL-NULL-002` | `op.alter_column(..., nullable=False)` without a preceding backfill | same shape; `alter_column` with `nullable=False` ∧ no prior `op.execute("UPDATE ...")` | 0.85 |
| `AL-DROP-003` | `op.drop_column(...)` — breaking change unless prior deploy stopped reading it | always warn unless `# drift-allow: drop-column` comment | 0.50 (advisory) |
| `AL-IDX-004` | `op.create_index(...)` without `postgresql_concurrently=True` | call with no kwarg `postgresql_concurrently` | 0.90 |
| `AL-FK-005` | `op.create_foreign_key(...)` without `postgresql_not_valid` followed later by `VALIDATE CONSTRAINT` | single-call shape | 0.75 |
| `AL-RENAME-006` | `op.alter_column(... new_column_name=...)` — backward-incompatible | always warn | 0.50 |
| `AL-DATA-007` | `op.bulk_insert(table, [...])` of >1000 rows inside `upgrade()` | list literal arg with >1000 elements; should chunk | 0.70 |
| `AL-SQLI-008` | `op.execute(f"...{var}...")` — SQLi-shaped, even in migrations | f-string in `op.execute` arg | 0.95 |

---

## 4 — Step-by-step build order

Each step is independently testable and shippable.

### Step 1 — Skeleton + scaffolding (no rules yet, just plumbing)
- Create `src/orm/mod.rs`, `src/orm/python/mod.rs` with empty `Framework` enum, `OrmRule` struct, `PyOrmContext` struct.
- Wire `collect_findings()` stub into [src/insights.rs:518](../src/insights.rs#L518) — returns `vec![]`.
- Add integration test: parse a fixture file, assert `collect_findings()` is called.

**Done when:** `cargo test` passes; the new dispatcher is in the call path but emits nothing.

### Step 2 — Widen tree-sitter captures + build the binding map
- Append `@bind.*`, `@loop.*`, `@decorator.*` captures to [src/languages/python.rs:18](../src/languages/python.rs#L18).
- Populate `PyOrmContext.bindings` / `for_loops` from the new captures in `tags.rs`.
- Test: feed a synthetic Python file with `qs = User.objects.filter(...)` and `for u in qs:`; assert `bindings["qs"] = DjangoQuerySet{model=User}` and `bindings["u"] = DjangoModelInst{model=User}` *only inside the loop body byte-range*.

**Done when:** Binding propagation tests pass; existing tags-extraction tests still pass.

### Step 3 — Django rules (highest-ROI, simplest)
Implement rules in this order:
1. `DJ-N1-003` (`len(qs)`) — pure syntactic, 0 binding required → smoke-test the rule plumbing
2. `DJ-N1-004` (`count() > 0`) — comparison-operator AST walk
3. `DJ-PERF-007` (`Manager.create()` in loop) — uses `in_loop` flag + binding map
4. `DJ-N1-001` (queryset iteration + related access) — full binding-map exercise
5. Remaining DJ-*

Each rule = one tiny matcher function + one fixture file under `tests/fixtures/orm/django/`.

**Done when:** Each rule has a positive fixture and at least 2 negative fixtures (no false positive on similar-shaped non-ORM code).

### Step 4 — SQLAlchemy rules
Same shape as Django. Two extra wrinkles:
- 1.x vs 2.x: 1.x uses `session.query(...)`, 2.x uses `select(...)` + `session.scalars(...)`. Detect API generation from imports: `from sqlalchemy.orm import Session` + `select` in imports ⇒ 2.x.
- `relationship()` declarations live on Model classes — for `SA-LAZY-008` we need to scan class bodies for `Mapped[...]` annotations or `relationship(lazy="dynamic")` kwargs.

### Step 5 — Alembic rules
- Alembic detection gate: file path matches `.*/alembic/versions/.*\.py` OR file imports `from alembic import op`. Use the second — robust against project-specific layout.
- All Alembic rules are intra-function (look only at `def upgrade(): ...`). No cross-file analysis needed for v1.

### Step 6 — Report integration + JSON schema update
- Decide `FindingKind` strategy (§6 below).
- Update [schema/profile.schema.json](../schema/profile.schema.json) and the validator test in [tests/integration.rs](../tests/integration.rs).

### Step 7 — Docs + corpus
- Add `tests/fixtures/orm/{django,sqlalchemy,alembic}/` with one file per rule (positive + negative).
- Smoke-test on 3 OSS Django projects and 3 OSS SQLAlchemy projects, count findings by rule id, eyeball false-positive rate, tune `confidence`.

---

## 5 — Hard edges and how we handle them

### 5.1 Aliasing / re-export
`from django.db.models import QuerySet as QS` — track via `@import.alias` (already captured by `python.rs`). `ImportMap` resolves both directions.

### 5.2 Cross-file model graph
v1 ships without it. `DJ-PROJ-010` (cartesian on `.values('m2m')`) and the `SA-N1-002` *-to-many detection both want it. v2 plan: a project-wide pass that scans for `class X(models.Model):` or `class X(Base):` bodies, extracts `ForeignKey` / `relationship()` declarations, builds a `ProjectModelGraph`, and re-runs the rules that opted into it. Same crate, separate pass — no schema change.

### 5.3 Conditional `select_related` / `prefetch_related`
```python
qs = User.objects.all()
if include_posts:
    qs = qs.prefetch_related("posts")
for u in qs: u.posts.all()  # N+1 on the `else` branch
```
We flag with `confidence: 0.55` — accept the FP. Alternative: control-flow-sensitive binding (`BindingKind::Maybe(...)`) is doable but doubles the binding-map size; defer to v2.

### 5.4 Dynamic attribute access
`getattr(obj, attr_name)` — give up, emit nothing. We chose precision over recall.

### 5.5 Decorated methods / managers
Custom managers (`class UserManager(models.Manager): def active(self): return self.filter(...)`) — `User.objects.active()` should be inferable as `DjangoQuerySet` because `active()` is defined on a `Manager` subclass. v1: only built-in Manager methods (`filter`, `all`, `exclude`, `get`, …). v2: scan `class _(models.Manager)` bodies and learn the custom methods.

### 5.6 Async (Django 4.x, SQLAlchemy AsyncSession)
`aiter(qs)` / `await qs.aiterator()` — the same rules apply; just add async-call shapes to the capture set.

---

## 6 — `FindingKind` strategy

Two options:

**(a) One flat kind per framework** — adds 3 new variants (`django_antipattern`, `sqlalchemy_antipattern`, `alembic_migration`). Pros: viewer can filter "show only Django" with one click. Cons: schema enum grows by 3 every time we add a framework (Tortoise, Peewee, Beanie, …).

**(b) One umbrella kind `orm_antipattern`** — rule id carried in `evidence[0].call` (matches how `sql_lint.rs:97` does it). Pros: schema stable, follows existing convention. Cons: viewer needs to read evidence to group by framework.

**Recommendation: (b).** It mirrors the SQL-lint convention and the rule id prefix (`DJ-*` / `SA-*` / `AL-*`) cleanly identifies the framework. Schema change: one new enum value `"orm_antipattern"` in [schema/profile.schema.json:434](../schema/profile.schema.json). Code change: one new variant `FindingKind::OrmAntipattern` in [src/insights.rs:91](../src/insights.rs#L91).

---

## 7 — Effort estimate

| Step | Effort |
|---|---|
| 1 — Skeleton | 0.5 day |
| 2 — Captures + binding map | 1.5 days |
| 3 — Django rules (12) | 2 days |
| 4 — SQLAlchemy rules (10) | 2 days |
| 5 — Alembic rules (8) | 1 day |
| 6 — Report integration + schema | 0.5 day |
| 7 — Fixtures + OSS corpus tuning | 1.5 days |
| **Total v1** | **~9 working days** |

v2 (cross-file ModelGraph, custom Manager learning, control-flow-sensitive bindings, async, Tortoise/Peewee): another 2–3 weeks.

---

## 8 — Open questions for the user

1. **One umbrella `FindingKind` or per-framework?** Plan recommends (b); confirm before §6.
2. **Should detection be gated by import?** (e.g. only run Django rules if `django.db.models` is imported in the file). Recommend yes — keeps cost near-zero on non-ORM files. Counter-argument: a file that imports nothing but uses an injected `qs` parameter is missed.
3. **Confidence threshold for inclusion in the report?** drift-static-profiler currently includes all findings. Some v1 rules (`DJ-PAG-012`, `AL-DROP-003`) are advisory at ~0.40–0.50 confidence — should those go in a separate `informational` bucket?
4. **Cross-file model graph in v1 or v2?** v2 keeps v1 shippable in 9 days; v1+graph pushes total to ~3 weeks and requires a second-phase report.rs pass.

---

## Sources

- [richardhapb/django-check](https://github.com/richardhapb/django-check) — closest prior art, Rust LSP-based static N+1 detector (no tree-sitter; custom parser + ModelGraph IR)
- [django-check VSCode extension](https://marketplace.visualstudio.com/items?itemName=richardhapb.Django-Check)
- [django-query-doctor v2.0 forum post](https://forum.djangoproject.com/t/django-query-doctor-v2-0-sql-compilation-cache-ast-serializer-analysis-baseline-regression-detection/44909)
- [nplusone (jmcarp)](https://github.com/jmcarp/nplusone) — Python runtime detector; reference for *what* to detect
- [Squawk — Postgres migration linter](https://squawkhq.com/docs/rules) — model for Alembic rule catalog
- [Semgrep django ruleset](https://semgrep.dev/p/django) — pattern catalog mining source
- [SQLAlchemy 2.0 Relationship Loading Techniques](https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html)
- [SQLAlchemy lazy="dynamic" + yield_per incompatibility](https://docs.sqlalchemy.org/en/20/orm/queryguide/api.html)
- [Django QuerySet API reference](https://docs.djangoproject.com/en/6.0/ref/models/querysets/)
- [Alembic Operation Reference](https://alembic.sqlalchemy.org/en/latest/ops.html)
- [Django antipatterns catalog](https://www.django-antipatterns.com/) — community catalog
- Internal: [ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md](ORM_EAGERLOAD_DTO_CACHE_STREAM_CATALOG.md), [MIGRATION_AST_SHAPES.md](MIGRATION_AST_SHAPES.md), [ORM_TO_SQL_TRANSLATION.md](ORM_TO_SQL_TRANSLATION.md)

---

# Addendum — Deeper Research (round 2)

Findings from a second research pass after the initial code/architecture review. This appendix supersedes the corresponding sections above where it disagrees with them; the rest of the plan stands. **Net effect:** sharper algorithm, fewer architectural ambiguities, ~14–18 day v1 (was 9), and a credible path to v2 cross-file precision.

## A — How `django-check` actually does it (read the source)

Verified by reading [crates/django-check_semantic/src/passes/{n_plus_one,functions,mod}.rs](https://github.com/richardhapb/django-check/tree/main/crates/django-check_semantic/src/passes) and [src/ir/model.rs](https://github.com/richardhapb/django-check/blob/main/crates/django-check_semantic/src/ir/model.rs).

### A.1 The data model (the part we copy verbatim)

```rust
// project-wide
pub struct ModelGraph {
    models: HashMap<String, ModelDef>,  // keyed by Model class name
}
pub struct ModelDef {
    pub name: String,
    pub file_path: String,
    pub line: usize,
    pub relations: Vec<Relation>,
    is_abstract: bool,
}
pub struct Relation {
    pub field_name: String,
    pub target_model: String,
    pub relation_type: RelationType,   // FK | OneToOne | ManyToMany | GenericFK
    related_name: String,
}
```

Lookup operations: `graph.get("User")`, `graph.dependents("User")` (reverse relations), `graph.dependency_depth()` (cycle-aware). It really is *just a string-keyed HashMap*. No fancy graph crate — `petgraph` (already a dependency) is overkill for this shape.

### A.2 The attribute-chain extractor (the function we copy verbatim, ~20 lines)

This is the load-bearing helper that turns `user.profile.settings.theme` into `("user", ["profile", "settings", "theme"])`. From django-check `passes/mod.rs`:

```rust
fn extract_attribute_chain(attr: &ExprAttribute) -> (&str, Vec<&str>) {
    let mut chain = vec![attr.attr.id.as_str()];
    let mut current = attr.value.as_ref();
    loop {
        match current {
            Expr::Attribute(attr) => {
                chain.push(attr.attr.id.as_str());
                current = attr.value.as_ref();
            }
            Expr::Call(call) => { current = call.func.as_ref(); }
            Expr::Name(name) => { chain.reverse(); return (name.id.as_str(), chain); }
            _ => return ("", Vec::new()),
        }
    }
}
```

Our tree-sitter port walks `attribute` nodes recursively (instead of `Expr::Attribute`) — same shape, ~30 lines. **This function is the actual core algorithm** — everything else is bookkeeping around it.

### A.3 The N+1 algorithm itself

```
on entering for_statement:
  resolve(iterable_expr) → maybe QuerySet or ModelInstance
  if yes: push LoopContext { loop_var, queryset_state }

on attribute access whose base == loop_var:
  (root, chain) = extract_attribute_chain(node)
  walked = ""
  for segment in chain:
    walked.push(segment)
    if walked not in queryset_state.prefetched_relations:
      emit_n_plus_one(node, walked)
      break  # don't double-report deeper segments

on leaving for_statement: pop LoopContext
```

Total: **~1100 lines of Rust** in [django-check passes/n_plus_one.rs](https://github.com/richardhapb/django-check/blob/main/crates/django-check_semantic/src/passes/n_plus_one.rs) — including ~50 test cases. That's the realistic size for our `orm/python/django.rs` N+1 detector alone. The original plan estimated "2 days for 12 Django rules"; this finding alone justifies the revised estimate of 3–4 days for Django.

### A.4 What django-check **doesn't** do — and why we don't either

Reading [`passes/functions.rs`](https://github.com/richardhapb/django-check/blob/main/crates/django-check_semantic/src/passes/functions.rs) directly: despite the README's claim of "interprocedural analysis", the pass operates only within one function at a time (`functions.iter().enumerate().next_back()` selects the most-recent function). They do not track:

- Function-call data flow (helper functions that return querysets)
- Return-value propagation
- Cross-function parameter inference

In their model, `def get_users() -> QuerySet[User]: return User.objects.all()` followed by `for u in get_users():` is **not** detected. The way they ship that case is via an explicit type annotation on the parameter (`def fn(users: QuerySet[User])`), which they recognize by name.

**Decision for our v1: same scope, same trade-off.** Cross-function flow lives in v2 alongside the project-wide ModelGraph.

## B — Why we are **not** adopting `stack-graphs`

[stack-graphs](https://github.com/github/stack-graphs) is GitHub's tree-sitter-based, language-agnostic name-resolution framework — exactly what we'd want for cross-file binding resolution. But:

- **Archived 2025-09-09** — read-only on GitHub, no longer supported. ([repo notice](https://github.com/github/stack-graphs))
- **Python implementation at v0.3.0** (Dec 2024 release), changelog shows recent crashes on lambdas and nested function definitions
- Heavyweight: requires defining graph-construction rules in TSG (tree-sitter-graph DSL); the [stack graphs paper](https://arxiv.org/pdf/2211.01224) is 36 pages of algorithmic substrate
- Our actual need is narrower: 3 frameworks, ~30 rules, single-language. A direct tree-walk implementation is ~500 lines; stack-graphs would be ~2000 lines plus the .tsg files

**Decision:** roll our own ~500-line binding tracker. Revisit stack-graphs (or its successor) for v2 cross-file ModelGraph **only if** drift-static-profiler grows a real semantic layer.

Background reading kept for the record:
- [Introducing stack graphs](https://github.blog/open-source/introducing-stack-graphs/) — the algorithmic intuition (symbol stacks, push-on-reference, pop-on-definition, pathfinding through merged per-file graphs)
- [tree-sitter-graph](https://github.com/tree-sitter/tree-sitter-graph) — the underlying DSL, still alive (not archived)

## C — Adopt Ruff's `BindingKind` taxonomy as the reference model

[Ruff](https://github.com/astral-sh/ruff) is the canonical Rust-based Python static analyzer. Its [`ruff_python_semantic` crate](https://github.com/astral-sh/ruff/tree/main/crates/ruff_python_semantic) ships a 21-variant `BindingKind` enum that's the de facto vocabulary for Python static analysis:

```rust
pub enum BindingKind<'a> {
    Annotation, Argument, NamedExprAssignment, Assignment,
    TypeParam, LoopVar, WithItemVar,
    Global(Option<BindingId>), Nonlocal(BindingId, ScopeId),
    Builtin, ClassDefinition(ScopeId), FunctionDefinition(ScopeId),
    Export(Export<'a>), FutureImport,
    Import(Import<'a>), FromImport(FromImport<'a>), SubmoduleImport(SubmoduleImport<'a>),
    Deletion, BoundException, UnboundException(Option<BindingId>), DunderClassCell,
}
pub struct Binding<'a> {
    pub kind: BindingKind<'a>,
    pub range: TextRange,                  // ← byte-offset anchor (not line)
    pub scope: ScopeId,
    pub context: ExecutionContext,
    pub source: Option<NodeId>,
    pub references: Vec<ResolvedReferenceId>,
    pub exceptions: Exceptions,
    pub flags: BindingFlags,
}
```

**Two lessons we steal:**

1. **`TextRange` (byte offsets), not line numbers.** drift-static-profiler currently anchors `Reference` by line. For ORM analysis we need precise byte ranges (e.g. to test "is *this exact reference* inside *that loop body*?"). The existing `Reference.byte_offset` field is the right anchor — extend the binding map to use byte ranges, not lines.

2. **Bindings carry references back-pointers.** Ruff's `Binding.references: Vec<ResolvedReferenceId>` is the "all the places this name is used" inverted index. Our `PyOrmContext.bindings: HashMap<&str, Vec<RefId>>` mirrors it.

**Decision:** extend `BindingKind` with ORM-specific variants (`DjangoQuerySet { facts: QuerySetFacts }`, etc.) and use byte-range anchors. Do **not** depend on `ruff_python_semantic` directly — Ruff's lifetimes (`<'a>` bound to a source-buffer arena) are wired into their parser; we don't want to pull in their parser. Copy the shapes; don't link.

## D — Tree lifetime: walk before drop (verified)

Verified by reading [src/tags.rs:200–215](../src/tags.rs#L200): the tree-sitter `Tree` is created at `parser.parse(source, None)` inside `extract_tags_inner` and **dropped at function return**. Only the extracted `FileTags` survives.

This gives us a clean tree-lifetime answer that the original plan §2.3 left ambiguous:

**Decision:** add a **second tree walk inside `extract_tags_inner`**, before the tree is dropped, that builds `PyOrmContext`. Attach `PyOrmContext` to `FileTags` as an optional field (only populated for files with framework imports — cost zero on non-ORM files).

```
fn extract_tags_inner(...) -> FileTags {
    let tree = parser.parse(source, None)?;
    // existing: run TAGS_QUERY, build references/symbols/imports
    let tags = run_existing_capture_pass(&tree, source, ...)?;
    // NEW: if file imports a framework, walk the tree once more
    let orm_ctx = if tags.imports.has_any_orm() {
        Some(orm::python::build_context(&tree, source, &tags)?)
    } else { None };
    tree.drop();  // existing behavior
    FileTags { ..tags, orm_ctx }
}
```

Cost: one extra `TreeCursor` traversal per ORM file. tree-sitter cursor traversal is ~0.5ms/file at the sizes we see in real Django projects. Acceptable.

This **eliminates** the need to widen the existing `TAGS_QUERY` with `@bind.*` / `@loop.*` / `@attr.access` captures. Original plan §2.1 was wrong about this; the right place to extract this information is a code walk, not a capture pattern. Tree-sitter queries are good at pattern matching; they're bad at "show me every assignment whose LHS is in scope of this for-loop's body" — that needs imperative walking.

## E — SQLAlchemy 2.0 typed annotations are a partial free model graph

The 2.0 typed declarative API gives us cardinality without any extra inference:

```python
class User(Base):
    posts: Mapped[List["Post"]] = relationship(back_populates="user")   # collection
    profile: Mapped["Profile"] = relationship(back_populates="user")    # scalar
```

[SQLAlchemy 2.0 docs](https://docs.sqlalchemy.org/en/20/orm/basic_relationships.html) — the `Mapped[X]` vs `Mapped[List[X]]` distinction is *parsed by SQLAlchemy itself* to set `uselist`. We can parse the same shape from the tree-sitter AST:

```scheme
(assignment
  left: (identifier) @rel.field
  type: (generic_type
    (identifier) @rel.mapped (#eq? @rel.mapped "Mapped")
    (type_parameter
      (type [(identifier) @rel.target              ; scalar:  Mapped[Post]
             (subscript                            ; collection: Mapped[List[Post]]
               value: (identifier) @rel.container
               subscript: (type (identifier) @rel.target))]))))
```

This gives us, per-file, a populated mini-ModelGraph for the classes defined in that file. **Promotes SA-N1-002** (joinedload on collection) from confidence 0.70 → 0.95 because we now *know* cardinality. **Promotes SA-LAZY-008** (dynamic-relationship + `len()`) similarly.

Caveat: forward-reference strings (`Mapped["Post"]` where `Post` is defined in another file) require either v2 cross-file lookup or an in-file fallback to "assume the class exists and is a Model". For v1, in-file is enough — most real codebases declare relationships next to the model class.

## F — Alembic ordering is trivial AST traversal, not data-flow

Worth being explicit: Alembic migration safety rules look like data-flow problems but aren't.

```python
def upgrade():
    op.add_column("users", sa.Column("email", sa.String(), nullable=True))   # ok
    op.execute("UPDATE users SET email = ''")                                # backfill
    op.alter_column("users", "email", nullable=False)                        # ok b/c backfill above
```

To prove `AL-NULL-002` (`alter_column(nullable=False)` without preceding backfill), we don't need a CFG. We need **statement-order traversal of the function body**:

```rust
struct AlembicState {
    added_nullable: HashMap<(String, String), usize>,  // (table, col) → stmt_idx
    backfilled: HashSet<(String, String)>,             // (table, col)
}

fn check_upgrade(body: &[Statement]) -> Vec<Finding> {
    let mut state = AlembicState::default();
    for (i, stmt) in body.iter().enumerate() {
        match classify(stmt) {
            OpAddColumn { table, col, nullable: true }  => state.added_nullable.insert((table, col), i);
            OpExecute(sql) if sql.contains("UPDATE")    => for (t, c) in mentioned(sql) { state.backfilled.insert((t, c)); }
            OpAlterColumn { table, col, nullable: false } if !state.backfilled.contains(&(table, col)) =>
                findings.push(...),
            _ => {}
        }
    }
    findings
}
```

Linear walk through the `function_definition` body block. Tree-sitter gives us body statements in source order. No CFG, no fixed-point iteration. ~150 lines for the full Alembic rule set.

**Caveats:**

- Conditionals (`if some_flag: op.execute(...)`) defeat the simple linear walk. v1 policy: treat `if`-branched statements as *not* preceding (conservative — may FP, won't FN). v2 can do branch-merging.
- Helper functions (`def _backfill(): ...` called from `upgrade()`) defeat the walk. v1 policy: opaque helper = "no backfill happened". v2 can inline.
- SQL inside `op.execute("UPDATE ...")` — we already have `sqlparser-rs` in the build (`Cargo.toml:59`); use it to extract `(table, column)` tuples instead of substring search.

## G — Performance budget (concrete)

The original plan handwaved performance. Pinning numbers:

| Phase | Cost per file | Total for 10K-file repo |
|---|---|---|
| tree-sitter parse (existing) | ~0.5 ms | 5 s |
| existing TAGS_QUERY pass | ~0.3 ms | 3 s |
| **NEW: ORM context walk** (only ORM files) | ~1 ms × ~10% files = 0.1 ms avg | **+1 s** |
| **NEW: rule pass** (per ORM file) | ~0.5 ms × ~10% = 0.05 ms avg | **+0.5 s** |
| **NEW: cross-file ModelGraph build (v2)** | ~10 ms one-shot | **+10 s** (v2 only) |

References:
- Tree-sitter query benchmarks: ~16 ms for a single execution on full-file traversal; queries with shallow captures are much faster ([tree-sitter discussions #1976](https://github.com/tree-sitter/tree-sitter/discussions/1976))
- Ruff parses + lints ~10K files in ~1 s on M1 — for reference; we're slower because tree-sitter is less optimized than Ruff's hand-written parser, but tree-sitter is reused across our existing pipeline

**v1 budget:** add ≤2 s to existing pipeline on 10K-file repos. **v2 budget:** add ≤15 s including cross-file resolution.

Drift's existing pipeline parallelizes parsing via [Rayon](../Cargo.toml#L39); the ORM passes inherit that parallelism for free as long as `PyOrmContext` is built inside `extract_tags_inner` (which runs in parallel).

## H — False-positive calibration: what "good" looks like

No public benchmark exists for static N+1 detection specifically. Adjacent benchmarks ([SAST FP rate comparison](https://www.mobb.ai/blog/sast-tools-false-positive-comparison)):

- Veracode: ~1% FP rate (enterprise SAST)
- Checkmarx: ~36% FP rate
- SonarQube: ~1% on OWASP Benchmark, much higher in the wild

For our class of rule (performance, not security), reasonable targets:

- **High-confidence rules** (≥0.90 confidence): target **<5%** FP rate. Examples: `DJ-N1-003` (`len(qs)` — `qs` is provably a queryset), `AL-SQLI-008` (f-string in `op.execute`), `SA-EXEC-009` (f-string in `text()`).
- **Standard rules** (0.70–0.89): target **<15%** FP rate. Examples: `DJ-N1-001` (loop iteration + related access), `SA-PERF-005` (per-row select in loop).
- **Advisory rules** (<0.70): target **<30%** FP rate; emitted as `informational` severity. Examples: `DJ-PAG-012` (deep pagination), `AL-DROP-003` (column drop).

**Calibration plan:**

1. Hand-curate a ground-truth corpus of 200 findings across 5 OSS Django/SQLAlchemy projects (suggested targets: `sentry`, `mastodon-py`, `Saleor`, `pretalx`, `airflow`). Public, large, and well-engineered — false positives are real bugs in the detector, not in the code.
2. Each finding classified as TP / FP / opinion-dependent.
3. Per-rule FP rate gated at ship time: rules above their tier's FP budget get downgraded confidence or held back from v1.

This replaces the original plan's vague "smoke test on 3 OSS projects". It's also why the revised effort estimate (§I) is 2–3 days for corpus work, not 1.5.

## I — Revised effort estimate (supersedes §7 above)

| Step | Original | Revised | Why |
|---|---|---|---|
| 1 — Skeleton | 0.5 d | 0.5 d | unchanged |
| 2 — Tree walker + binding map | 1.5 d | **3–4 d** | django-check's algorithm is ~500 LOC; chain extractor + loop ctx + prefetched-set tracking |
| 3 — Django rules | 2 d | **3–4 d** | first rules ~1d each for plumbing exercise; remaining ~2/d |
| 4 — SQLAlchemy rules | 2 d | **3–4 d** | adds `Mapped[]` annotation parsing (E above) |
| 5 — Alembic rules | 1 d | **1.5–2 d** | adds SQL extraction from `op.execute` strings |
| 6 — Report + schema | 0.5 d | 0.5 d | unchanged; **but** schema gets 3 distinct FindingKinds (not umbrella — reversing the original recommendation per the review) |
| 7 — Corpus + calibration | 1.5 d | **2–3 d** | 5 projects, 200 findings, per-rule FP-rate gating |
| **v1 total** | 9 d | **~14–18 d** | |

v2 (cross-file ModelGraph, custom managers, control-flow-sensitive bindings, async, Tortoise/Peewee): unchanged 2–3 weeks, but more credible given the v1 foundation is right-shaped.

## J — Open questions resolved by this round

The original plan's §8 questions, now with research-backed answers:

1. **Umbrella `FindingKind` or per-framework?** Per-framework — three variants (`DjangoAntipattern`, `SqlAlchemyAntipattern`, `AlembicMigration`). Rationale: §I above + the architecture-review note. (Reverses the original recommendation.)
2. **Gate by import?** Yes, in `extract_tags_inner` via `tags.imports.has_any_orm()`. §D above.
3. **Confidence threshold for inclusion?** Three tiers: high (≥0.90, <5% FP target), standard (0.70–0.89, <15%), advisory (<0.70, emitted as `informational`). §H above.
4. **Cross-file ModelGraph in v1?** No, v2. v1 ships with intraprocedural + in-file `Mapped[]` mini-graph. §A.4 above.

## K — What's still open

Things this research round did **not** resolve; will need to be settled during implementation:

- **Comprehension-scope handling.** Python's `[x.related for x in qs]` is a list comprehension, not a `for_statement`. tree-sitter-python exposes `list_comprehension` / `set_comprehension` / `dict_comprehension` / `generator_expression`. Each has a `for_in_clause`. Whether `LoopContext` semantics apply uniformly across all four is something we'll have to verify on fixtures.
- **`async for`.** `async_for_statement` in tree-sitter-python is its own node. Should behave identically for our purposes, but worth a fixture.
- **Class-body bindings.** `class UserView: queryset = User.objects.all()` — `queryset` is a class attribute, used as `self.queryset` later. Whether to model `self.X` as a binding inheriting from the class body is a v1.5 call.
- **Custom `Manager` subclasses with chainable methods.** `User.objects.active().popular()` where `active`/`popular` are custom Manager methods. v2.
- **Salsa-style incrementality.** [ty](https://astral.sh/blog/ty) uses Salsa to re-analyze only changed scopes; 80× faster than Pyright on PyTorch. drift-static-profiler is batch today, but as it grows toward IDE-grade latency, Salsa adoption is the eventual path. **Out of scope for v1/v2.**

## Additional sources (round 2)

- [django-check N+1 pass source (~1100 LOC)](https://github.com/richardhapb/django-check/blob/main/crates/django-check_semantic/src/passes/n_plus_one.rs) — reference algorithm
- [django-check ModelGraph IR](https://github.com/richardhapb/django-check/blob/main/crates/django-check_semantic/src/ir/model.rs) — data shape
- [Ruff `ruff_python_semantic` crate](https://github.com/astral-sh/ruff/tree/main/crates/ruff_python_semantic) — Binding/Scope vocabulary
- [Stack graphs (archived)](https://github.com/github/stack-graphs) — name-resolution framework we decided **against**
- [Stack graphs paper (Creager 2022)](https://arxiv.org/pdf/2211.01224) — algorithmic background
- [Introducing stack graphs (GitHub Blog)](https://github.blog/open-source/introducing-stack-graphs/)
- [Astral ty: incremental Python type checker](https://astral.sh/blog/ty) — Salsa-based incrementality reference
- [Astral ty (deepwiki)](https://deepwiki.com/astral-sh/ruff/6-red-knot-type-checker)
- [SQLAlchemy 2.0 Basic Relationship Patterns](https://docs.sqlalchemy.org/en/20/orm/basic_relationships.html) — `Mapped[List[X]]` vs `Mapped[X]`
- [Pyflakes checker source](https://github.com/PyCQA/pyflakes/blob/main/pyflakes/checker.py) — older but battle-tested scope/binding model
- [SAST FP rate comparison (Mobb)](https://www.mobb.ai/blog/sast-tools-false-positive-comparison) — adjacent FP-rate benchmarks
- [Tree-sitter query performance discussion](https://github.com/tree-sitter/tree-sitter/discussions/1976)
