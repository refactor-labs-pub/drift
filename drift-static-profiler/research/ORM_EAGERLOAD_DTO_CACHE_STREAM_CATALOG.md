I have enough material. Let me write the comprehensive deliverable.

---

# Drift Static Profiler — Per-ORM Detector Catalog

This is the load-bearing reference for the 200+ detector rules in `drift-static-profiler`. Every entry is named so a Rust + tree-sitter detector can match literally on method-name, receiver pattern, framework import, or AST shape.

---

## PART 1 — Eager-Loading API Menus (per ORM)

### 1.1 Django ORM

Module: `django.db.models` (and `django.db.models.query`).

| Method | Import | SQL emitted | When to use | Anti-pattern / failure mode |
|---|---|---|---|---|
| `QuerySet.select_related(*fields)` | `django.db.models.QuerySet` | Single SELECT with INNER/LEFT JOIN per FK/OneToOne | FK / OneToOne, single-valued forward relations | Calling on `ManyToMany` or reverse FK silently does nothing; chaining too many JOINs blows row width |
| `QuerySet.prefetch_related(*lookups)` | same | Original SELECT + 1 extra SELECT per relation using `WHERE id IN (...)` | M2M, reverse FK, GenericRelation | Combined with `iterator()` without `chunk_size` (pre-4.1) silently drops prefetch; very large `IN` lists hit PG `max_locks_per_transaction` |
| `Prefetch(lookup, queryset=, to_attr=)` | `django.db.models.Prefetch` | Same as above but you control the inner `queryset` (filter/order/`select_related`/`only`) | Nested or filtered prefetch | Forgetting `to_attr` overwrites the relation manager; an inner `slice` triggers `AssertionError` |
| `QuerySet.only(*fields)` | `QuerySet` | `SELECT col1, col2, pk FROM ...`; other cols become Deferred | Trim wide tables | Accessing a deferred attribute later issues a single-row `SELECT` per access — N+1 in disguise |
| `QuerySet.defer(*fields)` | `QuerySet` | Inverse of `only`: omit listed cols | Defer a TEXT/JSON column | Same deferred-attribute N+1 |
| `QuerySet.annotate(**kwargs)` | `QuerySet` | Adds `(SELECT ...) AS alias` or aggregate to outer SELECT | Replace `len(qs.related.all())` with `Count('related')` | Annotation with `Count` over JOIN multiplies rows — wrap in `Subquery` |
| `Subquery(queryset)` + `OuterRef("field")` | `django.db.models.Subquery, OuterRef` | Correlated subquery in SELECT/WHERE | Replace per-row queries inside loops | Forgetting `.values('pk')[:1]` returns multi-col subquery → Postgres error |
| `FilteredRelation(relation, condition=Q(...))` | `django.db.models.FilteredRelation` | LEFT JOIN with extra ON-clause filter | Joins where you need a filtered FK | Only valid in Django ≥ 2.0; ignored when used with `prefetch_related` |
| `QuerySet.extra(select=, where=, ...)` | `QuerySet` | Raw SQL fragments | Legacy escape hatch | Deprecated (Django ≥ 4.x); SQLi vector if interpolated |
| `QuerySet.iterator(chunk_size=N)` | `QuerySet` | Server-side cursor on PG; `LIMIT/OFFSET` fallback | Stream millions of rows | Without `chunk_size`, ignores `prefetch_related`; combined with `select_for_update` raises |
| `QuerySet.values(*fields)` | `QuerySet` | `SELECT field1, field2 ...` returning `dict` | Dict projection, replace entity hydration | `values('m2m')` produces cartesian; cannot follow `prefetch_related` |
| `QuerySet.values_list(*fields, flat=True, named=True)` | `QuerySet` | Same SELECT; returns tuples or namedtuples | Column extraction | `flat=True` only legal with single field |
| `QuerySet.in_bulk(ids)` | `QuerySet` | `SELECT ... WHERE pk IN (...)` returns dict | Bulk lookup by PK | Loading millions of ids blows memory |
| Community: `django-auto-prefetch`, `bulk_select_related` | third-party | Auto inserts `select_related` for accessed FKs | Defensive | Hides real prefetch needs; mismatched cardinality |

Docs: <https://docs.djangoproject.com/en/6.0/ref/models/querysets/>, <https://code.djangoproject.com/ticket/29984>

### 1.2 SQLAlchemy 2.x

Modules: `sqlalchemy.orm` (`joinedload`, `selectinload`, `subqueryload`, `lazyload`, `noload`, `raiseload`, `immediateload`, `contains_eager`, `with_loader_criteria`, `load_only`, `defer`, `undefer`, `with_polymorphic`, `selectin_polymorphic`, `Bundle`); applied via `select(...).options(...)` or `Query.options(...)`.

| Loader | Import | SQL emitted | When to use | Anti-pattern |
|---|---|---|---|---|
| `joinedload(rel)` | `sqlalchemy.orm.joinedload` | Single SELECT with LEFT OUTER JOIN (anonymized alias) | Many-to-one / one-to-one | On a *-to-many collection causes **row explosion** (cartesian); breaks `LIMIT` semantics — use `selectinload` instead |
| `joinedload(rel, innerjoin=True)` | same | INNER JOIN variant | When child is non-null | Hides parents with no child |
| `selectinload(rel)` | `sqlalchemy.orm.selectinload` | Original SELECT + secondary `SELECT ... WHERE pk IN (?, ?, ...)` | Default choice for collections / m2m | Single round-trip overhead per relation; not compatible with `yield_per` if the inner load returns more rows than buffer |
| `subqueryload(rel)` | `sqlalchemy.orm.subqueryload` | Re-issues original query as subquery + JOIN | Legacy; superseded by `selectinload` in 1.4+ | Bad with LIMIT/ORDER_BY; expensive plan re-execution; effectively deprecated |
| `lazyload(rel)` | `sqlalchemy.orm.lazyload` | Per-access SELECT | Default; opt-in for previously eager-loaded | The N+1 source |
| `noload(rel)` | `sqlalchemy.orm.noload` | Returns empty / None for that rel | Skip loading entirely | Surprises consumers expecting data |
| `raiseload(rel)` | `sqlalchemy.orm.raiseload` | Raises `InvalidRequestError` on access | Production guard against N+1 | Crashes downstream code that depends on lazy access |
| `raiseload(rel, sql_only=True)` | same | Raises only when SQL would be emitted | Production guard | — |
| `immediateload(rel)` | `sqlalchemy.orm.immediateload` | SELECT issued immediately after parent load | Tiny collections | Equivalent to N+1 if parents are many |
| `contains_eager(rel)` | `sqlalchemy.orm.contains_eager` | Tells ORM that user-supplied JOIN already includes the rel cols | When you `.join()` manually | Without manual JOIN → empty / weird results |
| `with_loader_criteria(Entity, condition)` | `sqlalchemy.orm.with_loader_criteria` | Adds global WHERE to all loads of `Entity` | Soft-delete / tenancy | Hidden global filter — explainability problem |
| `load_only(*cols)` | `sqlalchemy.orm.load_only` | `SELECT pk, col1, col2 ...` | Column trim | Lazy access to omitted cols → per-row SELECT |
| `defer(col)` / `undefer(col)` | `sqlalchemy.orm.defer`, `undefer` | Inverse of load_only | Defer wide columns | Same deferred-load N+1 |
| `with_polymorphic(Base, [Sub1, Sub2])` | `sqlalchemy.orm.with_polymorphic` | Single SELECT joining all subclass tables | Polymorphic fetch | Forgetting polymorphic JOIN → per-row subclass SELECT |
| `selectin_polymorphic(Base, [Subs])` | `sqlalchemy.orm.selectin_polymorphic` | Secondary IN-load for subclasses | Better than `with_polymorphic` for sparse subclasses | Per-subclass round-trip |
| `Bundle(name, *exprs)` | `sqlalchemy.orm.Bundle` | Plain SELECT of columns grouped under namespace | Lightweight projection | Bundle of relationships is ill-formed |
| `lazy="select"` (relationship kwarg) | `sqlalchemy.orm.relationship(lazy=...)` | Lazy default | Default | N+1 risk |
| `lazy="joined"` | same | Implicit `joinedload` on every load | Always-needed small one-to-one | Row explosion on collections |
| `lazy="subquery"` | same | Implicit `subqueryload` | Legacy | Same as `subqueryload` |
| `lazy="selectin"` | same | Implicit `selectinload` | Best default for collections | Always fires one extra round-trip even if rel unused |
| `lazy="raise"` | same | `InvalidRequestError` on any access | Strictest guard | Crashes |
| `lazy="raise_on_sql"` | same | Raises only if SQL would fire | Allows in-identity-map access | — |
| `lazy="noload"` | same | Never loads | Write-only models | Reads silently empty |
| `lazy="dynamic"` | same | Returns `AppenderQuery` (a Query object) | Large collections you query, not iterate | Calling `len()` triggers COUNT; iterating loads all |
| `lazy="write_only"` | `sqlalchemy.orm.WriteOnlyMapped` (2.0+) | Write-only collection | Append-only m2m / large rel | Reads must use explicit `select()` |
| `with_entities(*exprs)` (1.x style) | `Query.with_entities` | Reduces SELECT to listed cols | Projection | 2.0 prefers `select(Entity.col1, Entity.col2)` |

Docs: <https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html>, <https://docs.sqlalchemy.org/en/21/orm/queryguide/api.html>

### 1.3 Tortoise ORM

Module: `tortoise.queryset.QuerySet`; `tortoise.query_utils.Prefetch`.

| Method | SQL | Notes / anti-pattern |
|---|---|---|
| `QuerySet.select_related(*fields)` | JOIN | FK / O2O only |
| `QuerySet.prefetch_related(*lookups)` | +1 query per level using `IN` | Each depth = +1 query |
| `Prefetch(relation, queryset=, to_attr=)` from `tortoise.query_utils` | Same +1 with custom inner QS | Required for filtered prefetch |
| `QuerySet.only(*fields)` | Column trim | Lazy access on omitted field → re-fetch |
| `QuerySet.values(*fields, *kwargs)` | dict rows | Cannot select a relation directly — must traverse `actions__id` |
| `QuerySet.values_list(*fields, flat=False)` | tuple rows | `flat=True` only on single field |
| `QuerySet.fetch_related(*rels)` (model method) | Triggers prefetch on already-loaded models | Equivalent to Django `prefetch_related_objects` |

Docs: <https://tortoise.github.io/query.html>

### 1.4 Hibernate (JPA + Hibernate-native)

Packages: `jakarta.persistence`, `org.hibernate.annotations`, `org.hibernate.engine`.

| API | FQN | SQL | When / failure |
|---|---|---|---|
| `@OneToMany(fetch=FetchType.LAZY|EAGER)` | `jakarta.persistence.FetchType` | EAGER → JOIN at every load | EAGER on collections is the canonical anti-pattern |
| `@Fetch(FetchMode.JOIN)` | `org.hibernate.annotations.Fetch` | LEFT OUTER JOIN | Ignored when using `EntityGraph` |
| `@Fetch(FetchMode.SELECT)` | same | Secondary SELECT per parent | Pure N+1 unless combined with `@BatchSize` |
| `@Fetch(FetchMode.SUBSELECT)` | same | One subselect SELECT for all parents (collections only) | Works only for collections; subselect re-runs original query — beware paginated parents |
| `@BatchSize(size=N)` | `org.hibernate.annotations.BatchSize` | Batches lazy SELECTs into `WHERE id IN (?,?,...)` | Default `BatchFetchStyle` rounds up to padded buckets |
| `@FetchProfile(name=, fetchOverrides=)` | `org.hibernate.annotations.FetchProfile` | Activates per-session fetch override | Must call `session.enableFetchProfile()` |
| `@EntityGraph` (`type = LOAD | FETCH`) | `jakarta.persistence.EntityGraph` | LOAD = listed eager + defaults; FETCH = listed eager, rest lazy | `MultipleBagFetchException` if two `List` collections are fetched in same graph |
| `@NamedEntityGraph(name=, attributeNodes=)` | `jakarta.persistence.NamedEntityGraph` | Reusable graph applied via query hint | — |
| `EntityManager.find(cls, id, hints)` with `"jakarta.persistence.fetchgraph"` or `"jakarta.persistence.loadgraph"` | hint key constants `EntityGraph.GRAPH_TYPE_LOAD/FETCH` | Applies graph | Using wrong hint key silently ignored |
| `Hibernate.initialize(proxy)` | `org.hibernate.Hibernate` | Forces SELECT on a lazy proxy | Per-call N+1 |
| HQL/JPQL `JOIN FETCH` | — | LEFT JOIN + selects child cols | Cartesian if multiple collections; loses pagination semantics |
| `Query.setFetchSize(int)` | `org.hibernate.query.Query` | JDBC fetch hint | Driver-dependent |
| Second-level cache + query cache | see Part 3 | Skips SQL on hit | — |

Docs: <https://docs.hibernate.org/orm/current/javadocs/org/hibernate/annotations/FetchMode.html>, <https://www.baeldung.com/jpa-entity-graph>

### 1.5 Spring Data JPA

Package: `org.springframework.data.jpa.repository`.

| API | FQN | Effect |
|---|---|---|
| `@EntityGraph(attributePaths={"a","b.c"})` | `org.springframework.data.jpa.repository.EntityGraph` | Ad-hoc fetch graph on repo method |
| `@EntityGraph(value="name", type=LOAD|FETCH)` | same | Reference `@NamedEntityGraph` |
| `@Query("SELECT u FROM User u JOIN FETCH u.orders")` | `org.springframework.data.jpa.repository.Query` | JPQL fetch join |
| Interface projection (e.g., `interface UserName { String getName(); }`) | — | SELECT only needed cols; closed projection only |
| DTO projection in `@Query("SELECT new com.x.UserDto(u.id, u.name) FROM User u")` | — | Constructor expression |
| `<T> List<T> findBy(..., Class<T> type)` | dynamic projection | Same |
| `@QueryHints(@QueryHint(name="javax.persistence.fetchgraph", value="UserWithRoles"))` | `jakarta.persistence.QueryHint` | Pass graph hint |

Anti-pattern: open (proxy-based) projections defeat eager fetch — they trigger full entity load then map. Docs: <https://docs.spring.io/spring-data/jpa/reference/repositories/projections.html>

### 1.6 jOOQ

Package: `org.jooq.impl.DSL`.

| API | FQN | Effect |
|---|---|---|
| `DSL.multiset(Select)` | `org.jooq.impl.DSL.multiset` | Nested collection as a single column → no row multiplication |
| `DSL.multisetAgg(...)` | `org.jooq.impl.DSL.multisetAgg` | Aggregate variant — collect per group |
| `DSL.row(...)` | `org.jooq.impl.DSL.row` | Nested record |
| `Field<T>.convertFrom(targetType, fn)` | `org.jooq.Field#convertFrom` | Ad-hoc projection conversion |
| `ResultQuery.fetchGroups(keyFn, valueFn)` | `org.jooq.ResultQuery#fetchGroups` | Grouped Map result; not the same as DB-side aggregation |
| `ResultQuery.fetchMap(keyFn, valueFn)` | same | Map result; throws on duplicate keys |
| `select(...).from(...).leftJoin(...).fetchInto(MyDto.class)` | — | DTO projection |

Anti-pattern: simulating to-many fetching with multiple joins (the classic `MultipleBagFetchException`-style cartesian) instead of `MULTISET`. Docs: <https://blog.jooq.org/jooq-3-15s-new-multiset-operator-will-change-how-you-think-about-sql/>

### 1.7 Sequelize

Package: `sequelize` (v6/v7). Type names from `IncludeOptions`.

| Option | Effect | Anti-pattern |
|---|---|---|
| `include: [Model]` | LEFT OUTER JOIN | Multiple `hasMany` includes → cartesian product |
| `include: [{ model, required: true }]` | Converts to INNER JOIN | Filters parents silently |
| `include: [{ model, separate: true }]` | Emits a separate SELECT (similar to `selectinload`) | Only legal with `hasMany`; required if you want `limit` on the include |
| `include: [{ model, attributes: [...] }]` | Column trim for join | Forgetting FK → broken hydration |
| Nested `include: [{ model, include: [...] }]` | Cascading JOINs | Each level multiplies rows |
| `subQuery: false` on top-level | Disables inner-subquery wrapper Sequelize adds for paginated includes | Pagination off, totals wrong |
| `raw: true` | Skip model hydration | Returns flat-key cartesian; nested rels collapse to dotted keys |
| `nest: true` (with `raw: true`) | Dotted keys → nested objects | — |
| `duplicating: false` on include | Prevent Sequelize's auto-subquery | Use with extreme care |
| `attributes: { exclude: [...], include: [[fn(...), alias]] }` | Compound projection | — |

Docs: <https://sequelize.org/docs/v7/querying/select-in-depth/>

### 1.8 TypeORM

Package: `typeorm` 0.3.x.

| API | Effect |
|---|---|
| `repo.find({ relations: { posts: true } })` | LEFT JOINs |
| `repo.find({ relations: ["posts","comments"] })` | Same |
| `qb.leftJoinAndSelect("user.posts", "post")` | JOIN + select cols |
| `qb.innerJoinAndSelect(...)` | INNER variant |
| `qb.leftJoinAndMapMany("user.posts", Post, "post", "post.userId = user.id")` | JOIN + map to property |
| `qb.loadRelationIdAndMap("user.postIds", "user.posts")` | Adds just FK ids |
| `qb.loadRelationCountAndMap("user.postCount", "user.posts")` | Adds COUNT subquery, maps to property |
| `qb.relation(User, "posts").of(user).loadMany()` | Lazy fetch by id |
| `repo.find({ select: { id: true, name: true } })` | Column trim |
| Eager relations: `@OneToMany({ eager: true })` | LEFT JOIN on every `find` | Always-paid cost |

Anti-pattern: `leftJoinAndSelect` with multiple collections → row explosion; eager relations on common entity make every query expensive. Docs: <https://typeorm.io/docs/relations/relations-faq/>

### 1.9 Prisma

Package: `@prisma/client`.

| API | SQL | Anti-pattern |
|---|---|---|
| `findMany({ include: { posts: true } })` | 2 queries (parent + IN-load of children) | Default; nested `include` = +1 query per level |
| `findMany({ select: { id: true, name: true, posts: { select: { id: true } } } })` | Column trim + IN-load | Don't omit nested `select` → returns full row |
| `findMany({ include: { _count: { select: { posts: true } } } })` | `SELECT ..., (SELECT COUNT(*) ...) AS _count` | Counting children without fetching them |
| `findMany({ include: { posts: { take, skip, where, orderBy } } })` | Per-child filter applied in second query | `take`/`skip` here is per-parent! |
| `prisma.user.findUnique({...}).posts()` (fluent API) | 2 queries (one user, one posts) | Looks chained but is 2 round-trips |
| `findMany({ relationLoadStrategy: "join" })` (PG only, 5.7+) | Single SQL using LATERAL JOIN | Newer; converts 2 queries into 1 |
| `findMany({ relationLoadStrategy: "query" })` (default) | 2 queries | — |
| Cursor pagination: `findMany({ cursor: { id }, take, skip: 1 })` | Keyset on PK | OFFSET-style `skip` without cursor is slow on deep pages |

Anti-pattern: the **`?` field problem** — Prisma's TS types make all relations optional, encouraging code that does `user.posts?.length ?? 0` which fires lazy queries via fluent API. Docs: <https://www.prisma.io/docs/orm/prisma-client/queries/relation-queries>

### 1.10 Drizzle ORM

Package: `drizzle-orm`.

| API | SQL | Notes |
|---|---|---|
| `db.query.users.findMany({ with: { posts: true } })` | Single SQL (subquery + JSON aggregation) | Relational queries always emit exactly one query |
| `with: { posts: { columns: { id: true }, where, limit, orderBy } }` | Same; filters/limit applied per-parent | Deep nesting compiles to nested aggregates |
| Core: `db.select().from(users).leftJoin(posts, eq(users.id, posts.userId))` | Plain LEFT JOIN | Manual hydration needed for collections |
| `relations` export from schema | Declarative graph used by `query` API | Forgetting it → `with` is unavailable |
| RQB v2: `r` callback for `one`, `many`, `through` | — | v1 → v2 migration |

Docs: <https://orm.drizzle.team/docs/rqb-v2>

### 1.11 Mongoose

Package: `mongoose`.

| API | Effect |
|---|---|
| `Query.populate(path)` | Issues one secondary `find({ _id: { $in: [...] } })` per path |
| `Query.populate({ path, select, match, options })` | Same with field trim/filter/sort |
| `Query.populate({ path, populate: { path: 'inner' } })` | Deep populate — +1 query per depth |
| `Query.populate(virtualPath)` | Populates a virtual ref defined via `Schema.virtual().populate(...)` |
| `Query.lean()` | Returns POJO, skips Mongoose hydration; **drops virtuals & getters** |
| `Query.lean({ getters: true, virtuals: true })` | (with `mongoose-lean-virtuals` plugin) |
| `mongoose-autopopulate` plugin | Always populates configured paths | Hidden N+1 |

Anti-pattern: serializing a non-`lean` doc graph → triggers virtuals that lazy-populate. Docs: <https://mongoosejs.com/docs/populate.html>

### 1.12 EF Core

Namespace: `Microsoft.EntityFrameworkCore`.

| API | Effect |
|---|---|
| `IQueryable.Include(x => x.Posts)` | LEFT JOIN |
| `IQueryable.ThenInclude(x => x.Author)` | Continue from previous Include |
| `IQueryable.AsSplitQuery()` | One SELECT per Include using `IN` (avoids cartesian) |
| `IQueryable.AsSingleQuery()` | Force single JOIN query (default) |
| `IQueryable.Select(x => new UserDto { ... })` | Projection — bypasses entity tracking |
| `IQueryable.AsNoTracking()` | Disable change tracker — required for streaming |
| `IQueryable.AsNoTrackingWithIdentityResolution()` | No tracking but dedup graph |
| `DbContext.Set<T>().FromSqlInterpolated(...)` | Raw SQL — composable |
| `EF.CompileQuery(...)` / `EF.CompileAsyncQuery(...)` | Pre-compiled LINQ |
| Owned types / `OwnsOne` / `OwnsMany` | Always JOINed |

Anti-pattern: multiple `.Include` on collections without `AsSplitQuery` → **cartesian explosion**; `AsSplitQuery` with `Select` projecting a collection throws. Docs: <https://learn.microsoft.com/en-us/ef/core/querying/single-split-queries>

### 1.13 GORM

Package: `gorm.io/gorm`.

| API | SQL |
|---|---|
| `db.Preload("Posts").Find(&users)` | Two queries: parents + `WHERE user_id IN (...)` |
| `db.Preload("Posts.Comments")` | Nested — +1 query per depth |
| `db.Preload("Posts", "published = ?", true)` | Filtered preload |
| `db.Preload("Posts", func(db *gorm.DB) *gorm.DB { return db.Order("id desc") })` | Functional preload |
| `db.Joins("Author").Find(&posts)` | LEFT JOIN — only for `belongs to` / `has one`, populates struct |
| `db.Joins("JOIN posts ON ...").Find(...)` | Manual join — does not populate associations |
| `db.Model(&user).Association("Posts").Find(&posts)` | Lazy load by id |
| `db.Select("id", "name").Find(...)` | Column trim |

Anti-pattern: `db.Joins("Posts")` on a `has many` does NOT populate (Joins is for to-one only). Docs: <https://gorm.io/docs/preload.html>

### 1.14 ent

Module: `entgo.io/ent`.

| API | SQL |
|---|---|
| `client.User.Query().WithPosts().All(ctx)` | Original + secondary IN-load |
| `client.User.Query().WithPosts(func(q *ent.PostQuery) { q.Where(...).Limit(5) })` | Same with inner filter |
| `client.User.Query().Select(user.FieldName).Strings(ctx)` | Single-column projection |
| `client.User.Query().Select(...).Scan(ctx, &dst)` | DTO projection |
| `OnlySelect` / `OnlyX` | Single-row variant |

ent cannot combine multiple eager loads into a single JOIN — each `With<Edge>` becomes a separate SELECT. Docs: <https://entgo.io/docs/eager-load/>

### 1.15 ActiveRecord (Rails)

Module: `ActiveRecord::Relation`.

| Method | SQL |
|---|---|
| `Model.includes(:posts)` | Smart: `preload` if no condition references posts, `eager_load` if it does |
| `Model.preload(:posts)` | Always two queries (parent + IN) |
| `Model.eager_load(:posts)` | Always single LEFT OUTER JOIN |
| `Model.joins(:posts)` | INNER JOIN, does NOT load associations |
| `Model.references(:posts)` | Tells `includes` to use eager_load even when conditions are string SQL |
| `Model.select(:id, :name)` | Column trim — returns partial models |
| `Model.pluck(:name)` | `SELECT name` returning array of values; no model hydration |
| `Model.picks(:name)` (Rails 7+) | `pluck` for a single record |
| `Model.find_each(batch_size: 1000)` | `find_in_batches` + each — ordered by id ascending |
| `Model.find_in_batches(batch_size:)` | Yields arrays of records |
| `Model.in_batches(of:)` | Yields relations (chainable) |

Anti-pattern: `includes` + `where("posts.x = ?")` without `references(:posts)` raises `MissingAttributeError` in current versions or silently fails. Docs: <https://guides.rubyonrails.org/active_record_querying.html>

### 1.16 Eloquent (Laravel)

Namespace: `Illuminate\Database\Eloquent`.

| API | SQL |
|---|---|
| `Model::with('posts')->get()` | Two queries (parent + IN) |
| `Model::with(['posts' => fn($q) => $q->where(...)])` | Filtered eager load |
| `$user->load('posts')` | Lazy eager load on already-loaded model |
| `$user->loadMissing('posts')` | Only load if not yet loaded |
| `Model::withCount('posts')` | Adds `(SELECT COUNT(*) ...) AS posts_count` |
| `$user->loadCount('posts')` | Same, post-load |
| `Model::withMax('posts','created_at')` / `withMin`/`withAvg`/`withSum`/`withExists` | Aggregate subqueries |
| `Model::with(['commentable' => fn($morphTo) => $morphTo->morphWith([...])])` | Polymorphic eager load |
| `$model->loadMorph('commentable', [...])` | Same, post-load |
| `Model::preventLazyLoading()` (in `AppServiceProvider::boot`) | Throws `LazyLoadingViolationException` on any lazy access | Production guard |
| `Model::select(['id','name'])->get()` | Column trim |
| `Model::pluck('name')` | Returns `Collection` of values |

Anti-pattern: calling `$user->posts->count()` instead of `withCount('posts')` or `$user->posts()->count()`. Docs: <https://laracasts.com/discuss/channels/eloquent/withcount-loadcount-problem>

### 1.17 Doctrine ORM (PHP)

Namespace: `Doctrine\ORM`.

| API | SQL |
|---|---|
| `@ManyToOne(fetch="EAGER"|"LAZY"|"EXTRA_LAZY")` | EAGER = JOIN at every load; EXTRA_LAZY = collection methods (`count`, `contains`, slice) execute targeted SQL without full load |
| DQL `JOIN ... WITH ...` | INNER JOIN with extra condition |
| DQL `LEFT JOIN FETCH u.orders o` (fetch join) | Single SELECT including child cols |
| `Query::HINT_FORCE_PARTIAL_LOAD` | Allows partial entity hydration | Deprecated in 3.x; use DTO projections |
| DQL `SELECT u.id, u.name FROM User u` | Scalar/array projection |
| DQL `SELECT NEW App\Dto\UserDto(u.id, u.name) FROM User u` | Constructor DTO |
| `Doctrine\ORM\AbstractQuery::toIterable()` | Stream results (3.x); replaces deprecated `iterate()` |

Anti-pattern: `EAGER` on a `OneToMany` triggers cartesian on every related load; partial loading is a footgun (use DTOs). Docs: <https://www.doctrine-project.org/projects/doctrine-orm/en/3.6/reference/dql-doctrine-query-language.html>

### 1.18 NHibernate

Namespace: `NHibernate.Linq`, `NHibernate.FetchExtensions`.

| API | SQL |
|---|---|
| `query.Fetch(x => x.Author)` | LEFT JOIN to-one |
| `query.FetchMany(x => x.Posts)` | LEFT JOIN to-many |
| `query.FetchMany(...).ThenFetch(...)` / `.ThenFetchMany(...)` | Chained |
| `[BatchSize(N)]` mapping attribute | Batch lazy SELECTs |
| `BatchFetchStyle.LEGACY|PADDED|DYNAMIC` | Strategy for batching IN-lists |
| `query.ToFuture()` / `.ToFutureValue()` | Defer execution, batch multiple queries into one round-trip |
| `Criteria.SetFetchMode("Posts", FetchMode.Eager|Lazy|Join)` | Programmatic |

Anti-pattern: `FetchMany(...).FetchMany(...)` on the same root → cartesian. Docs: <https://nhibernate.info/doc/nhibernate-reference/performance.html>

---

## PART 2 — DTO / Projection Anti-Patterns

The detector rule: any code that loads a full entity solely to read a small subset of fields, count, exists-check, or serialize is a projection violation.

### 2.1 Hibernate / JPA

| Mechanism | Snippet |
|---|---|
| JPQL constructor expression | `SELECT NEW com.x.UserDto(u.id, u.name) FROM User u` |
| `EntityManager.createQuery(jpql, UserDto.class)` | typed return |
| Java records: `record UserDto(Long id, String name) {}` + constructor expression (or Hypersistence `ClassImportIntegrator`) | — |
| Tuple queries: `EntityManager.createQuery(cb.createTupleQuery())` | per-row `Tuple` |
| Hibernate-native `setResultTransformer(Transformers.aliasToBean(UserDto.class))` | (deprecated in Hibernate 6 in favor of `setTupleTransformer`) |
| Spring Data interface projection | `interface UserName { String getName(); }` |
| Spring Data dynamic projection | `<T> List<T> findBy(..., Class<T>)` |

Anti-patterns to flag:
- `em.find(User.class, id)` followed only by `.getId()` / `.getName()` access
- `repository.findAll().stream().map(...).collect(toList())` where the lambda touches ≤2 fields
- Spring Data **open** projection (uses SpEL like `@Value("#{target.x + target.y}")`) — defeats projection optimization, hydrates full entity
- DTOs returning collections via JPQL → multiplied rows

Docs: <https://thorben-janssen.com/dto-projections/>

### 2.2 SQLAlchemy

| Mechanism | Snippet |
|---|---|
| Core-style ORM select with columns | `select(User.id, User.name)` returns `Row`-like tuples |
| `Bundle("u", User.id, User.name)` | Namespaced columns |
| `load_only(User.id, User.name)` | Still hydrates entity; deferred-attribute access N+1 risk |
| `Query.with_entities(User.id, User.name)` (1.x) | Same |
| `cls.__table__.select().where(...)` | Pure Core, no ORM identity map |

Anti-patterns:
- `session.scalars(select(User)).all()` to compute `len()` or `[u.id for u in ...]` — use `session.scalars(select(func.count(User.id)))` or `session.scalars(select(User.id))`
- `jsonify(user.to_dict())` triggering lazy-loaded relationships during serialization

### 2.3 Django

| Mechanism | Snippet |
|---|---|
| `.values('id','name')` | Returns `QuerySet[dict]` |
| `.values_list('id', flat=True)` | List of pks |
| `.only('id','name')` | Returns model with deferred fields |
| `.defer('big_text')` | Inverse |
| `.annotate(Count(...))` | Aggregate as column |
| `Model.objects.in_bulk([...])` | Dict by pk |

Anti-patterns:
- `User.objects.all().count()` is fine; **`len(User.objects.all())`** loads them
- `serializer = UserSerializer(user); data = serializer.data` while user has unprefetched relations
- DRF: nested serializer fields without `prefetch_related` ⇒ N+1 inside `.data`

### 2.4 Sequelize

| Mechanism | Snippet |
|---|---|
| `attributes: ['id','name']` | Column trim |
| `attributes: { include: [[fn('COUNT', col('posts.id')), 'postCount']], exclude: [...] }` | Compound |
| `raw: true` | Plain object — no instance methods, no virtuals |
| `nest: true` (with `raw`) | Plain nested object |

Anti-patterns: serializing model instances with `JSON.stringify(user)` triggers `toJSON` for every nested association.

### 2.5 TypeORM

| Mechanism | Snippet |
|---|---|
| `repo.find({ select: ['id','name'] })` | Column trim |
| QB `.select(['user.id','user.name'])` | Same |
| QB `.getRawMany()` | Skip entity hydration |
| `qb.select('user.id', 'id').addSelect('COUNT(*)', 'count').groupBy('user.id').getRawMany()` | Pure raw projection |

### 2.6 Prisma

| Mechanism | Snippet |
|---|---|
| `select: { id: true, name: true }` | Canonical projection |
| Mixing `include` + `select` at same level | Error |
| Nested `select` inside `include` | Allowed |

Anti-pattern: `findMany()` returning full entities to do `.length` — use `prisma.user.count()`.

### 2.7 EF Core

| Mechanism | Snippet |
|---|---|
| `Select(x => new UserDto { Id = x.Id, Name = x.Name })` | Projection — no tracking needed |
| `Select(x => new { x.Id, x.Name })` | Anonymous type |
| `AsNoTracking()` + `Select` | Most efficient pattern |

Anti-pattern: `_ctx.Users.Include(u => u.Orders).ToList()` followed by `users.Count()` — use `_ctx.Users.CountAsync()`.

### 2.8 Eloquent

| Mechanism | Snippet |
|---|---|
| `Model::select(['id','name'])->get()` | Partial models |
| `Model::pluck('name')` | Returns `Collection<string>` (no model hydration — fastest) |
| `Model::pluck('name','id')` | keyed collection |
| `DB::table('users')->select(...)` | Query builder, no model overhead |

Anti-pattern: `User::all()->count()` (loads everything) vs `User::count()` (SQL COUNT).

### 2.9 Doctrine

| Mechanism | Snippet |
|---|---|
| DQL `SELECT u.id, u.name FROM User u` | Array hydration |
| DQL `SELECT NEW UserDto(...)` | DTO |
| `query->getArrayResult()` / `getScalarResult()` | Bypass entity hydration |
| Partial entities: `SELECT PARTIAL u.{id, name}` | Deprecated in 3.x |

### 2.10 ActiveRecord

| Mechanism | Snippet |
|---|---|
| `User.pluck(:name)` | Array of values |
| `User.pluck(:id, :name)` | Array of arrays |
| `User.select(:id, :name)` | Partial models — accessing other attrs raises `ActiveModel::MissingAttributeError` |
| `User.picks(:name)` (Rails 7+) | First row scalar |
| `User.ids` | Shortcut for `pluck(:id)` |

Anti-pattern: `User.all.map(&:id)` instead of `User.pluck(:id)`.

Universal anti-patterns to detect across all ORMs:
1. **Count-by-length**: `.length` / `.size` / `.count` on a hydrated collection instead of SQL `COUNT`
2. **Exists-by-find**: `findOne(...)` then null check instead of `exists`/`EXISTS`
3. **Serialize-full-graph**: `toJSON` / `to_dict` / `Jackson@JsonProperty` on an entity with lazy relations
4. **Hydrate-to-pluck**: fetch entities to read one column
5. **Eager-loaded count**: `withCount`/`include _count` is the right call, not `with` + `.length`

---

## PART 3 — ORM-Level Caching Layers

### 3.1 Hibernate L2

Providers and import paths:

| Provider | Maven coords / class | Notes |
|---|---|---|
| Ehcache 3 (via JCache) | `org.hibernate.orm:hibernate-jcache` + `org.ehcache:ehcache` | JSR-107 adapter; Hibernate 6 way |
| Caffeine (via JCache) | `com.github.ben-manes.caffeine:jcache` | In-process, no eviction surprises |
| Infinispan | `org.infinispan:infinispan-hibernate-cache-v62` | Distributed; transactional region factory |
| Hazelcast | `com.hazelcast:hazelcast-hibernate53` | Distributed |
| Redisson | `org.redisson:redisson-hibernate-6X` | Redis-backed |

API:

| Annotation / API | FQN | Effect |
|---|---|---|
| `@Cacheable` | `jakarta.persistence.Cacheable` | Marks entity cacheable |
| `@Cache(usage=CacheConcurrencyStrategy.READ_ONLY|NONSTRICT_READ_WRITE|READ_WRITE|TRANSACTIONAL, region="...")` | `org.hibernate.annotations.Cache` | Concurrency + region |
| `@Cache` on a `Collection` field | same | Collection cache (stores child PKs) |
| `hibernate.cache.use_query_cache=true` | property | Enable query cache |
| `Query.setCacheable(true).setCacheRegion("name")` | `org.hibernate.query.Query` | Per-query opt-in |
| `SessionFactory.getCache().evictEntityRegion(Class)` | `org.hibernate.Cache` | Programmatic eviction |

Anti-patterns:
- `READ_WRITE` on a hot-write entity = lock contention
- Query cache **without** entity cache for joined entities → query-cache hit triggers per-id `EntityNotFoundException` storm
- Caching mutable graphs that bypass Hibernate (raw SQL writes) → stale reads
- Forgetting region size config → cache thrashing
- Cache key collisions via region naming reuse across entity classes

Docs: <https://www.baeldung.com/hibernate-second-level-cache>

### 3.2 SQLAlchemy

Module: `dogpile.cache`.

| API | FQN |
|---|---|
| `dogpile.cache.make_region()` | region factory |
| `FromCache("region", cache_key="...")` (SQLAlchemy example pattern) | per-query option |
| `RelationshipCache(relationship_attr, "region")` | for lazy loads |
| Custom `_generate_cache_key(stmt, params)` | leverages SQL compile-cache key |
| `cache_ok=True` | on custom `TypeDecorator` — declares safe to use in SQL compile cache |

Anti-patterns: cache key not including session-level filters (multi-tenant footgun); `cache_ok=True` on a non-deterministic custom type → wrong row returned. Docs: <https://docs.sqlalchemy.org/en/20/_modules/examples/dogpile_caching/caching_query.html>

### 3.3 Doctrine

| API | FQN |
|---|---|
| `Doctrine\ORM\Configuration::setQueryCache(CacheItemPoolInterface)` | PSR-6 |
| `setResultCache(CacheItemPoolInterface)` | PSR-6 |
| `setMetadataCache(CacheItemPoolInterface)` | PSR-6 |
| `Query::enableResultCache(int $lifetime, string $cacheId)` | per-query |
| Second-level cache config: `Doctrine\ORM\Cache\CacheConfiguration` | `enable_second_level_cache` flag |
| `EntityManager::getCache()->evictEntity(Class, id)` | `Doctrine\ORM\Cache` API |
| `EntityManager::getCache()->evictEntityRegion(Class)` | — |
| `Doctrine\ORM\Mapping\Cache` attribute on entity | `#[ORM\Cache(usage: 'READ_ONLY', region: '...')]` |

Anti-patterns: enabling result cache without metadata cache → cache key generation is slow; result-cache on queries with current-time params → cache pollution.

Docs: <https://www.doctrine-project.org/projects/doctrine-orm/en/3.6/reference/caching.html>

### 3.4 EF Core

| API | FQN |
|---|---|
| `IMemoryCache` | `Microsoft.Extensions.Caching.Memory.IMemoryCache` (manual, not ORM-level) |
| `EFCoreSecondLevelCacheInterceptor` | NuGet `EFCoreSecondLevelCacheInterceptor` |
| `services.AddEFSecondLevelCache(options => options.UseMemoryCacheProvider())` | DI registration |
| Provider variants: `.UseMemoryCacheProvider()`, `.UseEasyCachingCoreProvider()`, `.UseStackExchangeRedisCacheProvider()` | |
| `.Cacheable(timeout, CacheExpirationMode.Absolute|Sliding)` | extension method on `IQueryable` |
| `.NotCacheable()` | opt-out |

Anti-pattern: `ExecuteUpdate` / `ExecuteDelete` bypass the interceptor — cache becomes stale. Docs: <https://github.com/VahidN/EFCoreSecondLevelCacheInterceptor>

### 3.5 Mongoose

Third-party plugins (Mongoose has no built-in):

| Plugin | npm name | API |
|---|---|---|
| cachegoose | `cachegoose` | `Query.cache(seconds, customKey)` |
| recachegoose | `recachegoose` | same |
| ts-cache-mongoose | `ts-cache-mongoose` | typed; memory or Redis |
| mongoose-redis-cache | `mongoose-redis-cache` | unmaintained since 2014 — flag as risk |
| mongoose-plugin-cache | `mongoose-plugin-cache` | per-model hooks |

Detect via: `require('cachegoose')` or `import 'cachegoose'`; query call ending in `.cache(...)`.

### 3.6 Sequelize

Third-party:

| Plugin | npm | Notes |
|---|---|---|
| `sequelize-transparent-cache` | `sequelize-transparent-cache` + an adapter | Per-model |
| `sequelize-redis-cache` | unmaintained | |

Sequelize has no first-class result cache. Detect via import.

### 3.7 TypeORM (built-in)

| API | Effect |
|---|---|
| DataSource options: `cache: true` | enables cache, default 1s TTL |
| `cache: { type: "database" | "redis" | "ioredis" | "ioredis/cluster", duration, options }` | provider config |
| `.cache(true)` on QueryBuilder | per-query |
| `.cache("key", milliseconds)` | named cache id + TTL |
| `repo.find({ cache: { id: "x", milliseconds: 25000 } })` | repo variant |
| `dataSource.queryResultCache.remove(["key"])` | manual invalidation |

Anti-patterns: default 1s TTL gives false sense of caching; named keys collide if mass-reused (e.g. `"all_users"` re-used across tenants); `database` cache writes to `query-result-cache` table — adds write load. Docs: <https://typeorm.io/docs/query-builder/caching/>

### 3.8 Prisma

| API | Notes |
|---|---|
| `prisma.$extends(withAccelerate())` | requires Accelerate |
| `findMany({ cacheStrategy: { ttl: 60, swr: 600 } })` | TTL + stale-while-revalidate |
| `prisma.$accelerate.invalidate({ tags: [...] })` | on-demand invalidation |
| `cacheStrategy.tags: ["..."]` | tag for targeted invalidation |

Supported only on: `findUnique[OrThrow]`, `findFirst[OrThrow]`, `findMany`, `count`, `aggregate`, `groupBy`. Anti-pattern: caching mutations or assuming auto-invalidation. Docs: <https://www.prisma.io/docs/accelerate/caching>

### 3.9 ActiveRecord (Rails)

| API | Effect |
|---|---|
| ActiveRecord query cache | enabled per controller action automatically; keyed by SQL string |
| `Rails.cache.fetch("key", expires_in:) { Model.find(...) }` | low-level |
| `Rails.cache.fetch_multi(*keys)` | batch |
| `<% cache @user do %>` (fragment) | view-level |
| Russian-doll: nested `cache` blocks with `touch: true` on associations | parent `updated_at` bumps on child save → cache key changes |
| `belongs_to :post, touch: true` | required for Russian doll |

Anti-patterns: caching keyed only by id (no `updated_at` / version → stale); `Rails.cache.fetch` storing AR model instances → loaders explode on deserialization across deployments.

### 3.10 Eloquent

| API | Effect |
|---|---|
| `Model::query()->remember($minutes, $key)` | macro from `laravel-cache-on-demand` (third-party) |
| `Cache::remember("key", $ttl, fn() => Model::all())` | manual |
| `Cache::tags(['users'])->remember(...)` | tag-based invalidation |
| `Cache::flexible("key", [$fresh, $stale], fn() => ...)` | SWR (Laravel 11+) |

Anti-patterns: tag stores limited to `redis`/`memcached`; caching collections via `Cache::put('users', User::all())` serializes Eloquent models → version pin issue.

Universal anti-patterns (Part 3):
- Cache-aside writes without invalidation hooks on `save`/`delete`
- Caching mutable graphs (entities with eager relations whose children mutate)
- Cache stampede on cold start (no probabilistic refresh / no lock)
- Missing or infinite TTL
- Cache keys formed from `id` only (collision across entity types) — always prefix with class FQN
- Stale-on-bulk-update: `ExecuteUpdate` / `update_all` / raw SQL bypass cache invalidation hooks

---

## PART 4 — Streaming / Cursor / Batch APIs

### 4.1 Hibernate / JPA

| API | FQN | Notes |
|---|---|---|
| `org.hibernate.ScrollableResults` | `org.hibernate.ScrollableResults` | Scrollable JDBC cursor; call `close()` |
| `Query.scroll(ScrollMode.FORWARD_ONLY)` | `org.hibernate.query.Query` | Forward-only cursor |
| `Query.stream()` (Hibernate 5.2+) / `Query.getResultStream()` (JPA 2.2) | `org.hibernate.query.Query`, `jakarta.persistence.Query` | Returns `java.util.stream.Stream` — close in try-with-resources |
| `Query.setFetchSize(N)` / hint `org.hibernate.fetchSize` | — | JDBC `setFetchSize` |
| `Session.clear()` between batches | `org.hibernate.Session` | Required to free first-level cache |
| `StatelessSession` | `org.hibernate.StatelessSession` | No first-level cache, no cascades, no listeners — bulk ingestion |

Anti-pattern: forgetting `Session.clear()` in a streaming loop → first-level cache OOM.

### 4.2 Spring Data JPA

| Return type | Effect |
|---|---|
| `Stream<T> findBy...()` | Uses `getResultStream` — must be inside `@Transactional` and try-with-resources |
| `Streamable<T>` | Like `List` but supports `concat`/`filter`/`map` |
| `Slice<T>` | Knows only if there's a next page |
| `Page<T>` | Includes total count — extra COUNT query |
| `@QueryHints(@QueryHint(name="org.hibernate.fetchSize", value="1000"))` | hint |

Anti-pattern: `Page<T>` for forward-only feeds → unnecessary COUNT.

### 4.3 SQLAlchemy

| API | FQN |
|---|---|
| `select(...).execution_options(yield_per=N)` | `sqlalchemy.sql.Select` |
| `connection.execution_options(stream_results=True)` | `sqlalchemy.engine.Connection` |
| `result.partitions(N)` | `sqlalchemy.engine.Result` |
| `session.scalars(stmt).yield_per(N)` | ORM-level |
| `windowed_query` (recipe, not in core) | community pattern using PK windows |

Notes: `yield_per` is **incompatible** with `joinedload` on collections and with `subqueryload`. Docs: <https://docs.sqlalchemy.org/en/20/orm/queryguide/api.html>

### 4.4 Django

| API | Effect |
|---|---|
| `QuerySet.iterator(chunk_size=N)` | Server-side cursor on PG; `LIMIT/OFFSET` on others |
| `Paginator(qs, per_page)` | OFFSET-based — bad for deep pages |
| `qs.filter(id__gt=last_id).order_by('id')[:N]` | Keyset pattern (manual) |

Anti-pattern: `for u in User.objects.all():` materializes everything — must use `.iterator()`.

### 4.5 Sequelize

No first-class streaming. Patterns:
- `findAll({ limit, offset, order: [['id','ASC']] })` — manual chunking
- `sequelize.query(sql, { raw: true })` then iterate — manual
- Community: `sequelize-cursor-pagination`, raw `pg-cursor` adapter

### 4.6 TypeORM

| API | Effect |
|---|---|
| `qb.stream()` | Returns Node `ReadableStream` of raw rows |
| `repo.find({ skip, take, order })` | OFFSET pagination |
| `qb.where("id > :last", { last }).orderBy("id").limit(N)` | Keyset |

Anti-pattern: `.stream()` returns raw rows, not entities — no hydration.

### 4.7 Prisma

No built-in streaming. Patterns:

```ts
let cursor;
while (true) {
  const batch = await prisma.user.findMany({
    take: 1000,
    ...(cursor && { skip: 1, cursor: { id: cursor } }),
    orderBy: { id: "asc" },
  });
  if (batch.length === 0) break;
  cursor = batch[batch.length - 1].id;
}
```

Anti-pattern: `findMany({ skip: 100000, take: 100 })` on deep page.

### 4.8 Mongoose

| API | Effect |
|---|---|
| `Query.cursor()` / `.cursor({ batchSize: N })` | Returns `QueryCursor` (Node streams3) |
| `cursor.eachAsync(async (doc) => {...}, { parallel: N })` | Concurrency-controlled iteration |
| `Query.stream()` | Deprecated alias |
| `Aggregate.cursor({ batchSize })` | Aggregation cursor |

Anti-pattern: `Model.find().populate('x').cursor()` — populate de-batches the cursor back to one-by-one population. Docs: <https://mongoosejs.com/docs/api/querycursor.html>

### 4.9 EF Core

| API | Effect |
|---|---|
| `IQueryable.AsAsyncEnumerable()` | Stream rows asynchronously |
| `IQueryable.AsNoTracking().AsAsyncEnumerable()` | Required for streaming large sets (otherwise change tracker accumulates) |
| `await foreach (var x in q.AsAsyncEnumerable())` | C# 8 async iteration |
| `IQueryable.AsSplitQuery()` + `AsAsyncEnumerable()` | Streaming with split queries |

Anti-pattern: streaming without `AsNoTracking` — tracker holds every entity → memory leak.

### 4.10 GORM

| API | Effect |
|---|---|
| `db.FindInBatches(&dst, 100, func(tx *gorm.DB, batch int) error {...})` | Batch processing |
| `rows, err := db.Model(&User{}).Rows()` then `db.ScanRows(rows, &user)` | Manual cursor |
| `db.Limit(N).Offset(M)` | OFFSET pagination |

Anti-pattern: `FindInBatches` with default ordering on a frequently-inserted table — duplicates/missed rows; use `id` keyset.

### 4.11 ent

ent has no streaming primitive. Pattern: keyset on `id` field via `Where(user.IDGT(last))`. Docs: <https://entgo.io/docs/>

### 4.12 ActiveRecord

| API | Effect |
|---|---|
| `Model.find_each(batch_size: N)` | Yields records; orders by id ASC |
| `Model.find_in_batches(batch_size:)` | Yields arrays |
| `Model.in_batches(of: N)` | Yields `ActiveRecord::Relation` (chainable: `.update_all`, `.destroy_all`) |
| `Model.in_batches(of: N).each_record { ... }` | Per-record |

Anti-patterns:
- `find_each` with custom `order` → raises (can't order)
- `find_each` followed by `record.update(...)` when `in_batches.update_all(...)` would do
- `find_each` with concurrent inserts in id order → missed rows; use `id_asc_start` cursor or keyset

### 4.13 Eloquent

| API | Effect |
|---|---|
| `Model::chunk($size, fn ($rows) => ...)` | Orders by primary key by default; **breaks if you update the keying column** |
| `Model::chunkById($size, fn ($rows) => ...)` | Safe variant — re-queries `WHERE id > last` |
| `Model::cursor()` | PHP generator using PDO `fetch()`; single statement, no buffering |
| `Model::lazy()` | LazyCollection wrapper around chunking — better memory than `cursor` for huge sets |
| `Model::lazyById($size)` | LazyCollection over `chunkById` |
| `Model::lazyByIdDesc($size)` | DESC variant |
| `Model::each(fn ($row) => ...)` | Chunks under the hood |

Anti-pattern: `chunk()` on a result set where you `update($keyColumn)` — rows get skipped because the cursor moves under you. Use `chunkById`. Docs: <https://janostlund.com/2021-12-26/eloquent-cursor-vs-chunk>

### 4.14 Doctrine

| API | FQN |
|---|---|
| `AbstractQuery::toIterable()` | `Doctrine\ORM\AbstractQuery` (3.x) |
| `AbstractQuery::iterate()` | Deprecated in 3.x |
| `EntityManager::clear()` between batches | Required to free identity map |
| `EntityManager::flush(); $em->clear()` after batch writes | Standard batching idiom |

Anti-pattern: `toIterable()` without `$em->clear()` in loop → memory leak via identity map.

### 4.15 Tortoise

| API | Effect |
|---|---|
| `QuerySet.__aiter__` (async iteration) | Materializes all then iterates |
| Manual keyset: `Model.filter(id__gt=last).limit(N)` | Required; no built-in cursor |

### 4.16 NHibernate

| API | Effect |
|---|---|
| `IQuery.Enumerable<T>()` | IEnumerable lazy fetch |
| `IQuery.Future<T>()` | Batches deferred queries |
| `IStatelessSession` | No first-level cache |
| `Criteria.SetFetchSize(N)` | JDBC-style fetch size |

Universal Part-4 anti-patterns:
- `.all()` / `.toList()` / `.get()` on a query that can return millions
- OFFSET pagination on deep pages (use **keyset / seek**)
- Per-row UPDATE in a loop instead of `update_all` / `ExecuteUpdate` / `bulk_update`
- Loading collection to compute `count()` / `exists()`
- Streaming without disabling identity map / change tracker
- `chunk` on a mutating key column (use `chunkById`/`find_each`/keyset)
- Forgetting `Session.clear()` / `em.clear()` / `Session.flush()` in batches → memory blow-up

---

## Detector tuple format (reference)

Each entry above slots into a Rust + tree-sitter rule as:

```rust
DetectorRule {
  id: "DJANGO-N1-001",
  framework_import: &["django.db.models"],
  receiver_pattern: AstPattern::QuerySetCall,
  method_names: &["all", "filter", "exclude"],
  followed_by_iteration_accessing: &["{related}.all", "{related}.count"],
  fix: "Use select_related/prefetch_related",
  severity: Warn,
}
```

The catalog above gives the literal method-name sets, import paths, fully-qualified class names, and SQL/anti-pattern semantics needed to populate ~200 such tuples.

---

## Sources

- [Django QuerySet API reference](https://docs.djangoproject.com/en/6.0/ref/models/querysets/)
- [Django ticket #29984 – prefetch_related with iterator()](https://code.djangoproject.com/ticket/29984)
- [Django Query Expressions (Subquery/OuterRef)](https://docs.djangoproject.com/en/4.1/ref/models/expressions/)
- [SQLAlchemy 2.0 Relationship Loading Techniques](https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html)
- [SQLAlchemy 2.1 ORM API Features for Querying](https://docs.sqlalchemy.org/en/21/orm/queryguide/api.html)
- [SQLAlchemy 1.4 Relationship Loading Techniques](https://docs.sqlalchemy.org/en/14/orm/loading_relationships.html)
- [SQLAlchemy 1.4 Loading Columns](https://docs.sqlalchemy.org/en/14/orm/loading_columns.html)
- [SQLAlchemy dogpile.cache example](https://docs.sqlalchemy.org/en/20/_modules/examples/dogpile_caching/caching_query.html)
- [dogpile.cache repo](https://github.com/sqlalchemy/dogpile.cache)
- [Tortoise ORM Query API](https://tortoise.github.io/query.html)
- [Hibernate FetchMode Javadoc](https://docs.hibernate.org/orm/current/javadocs/org/hibernate/annotations/FetchMode.html)
- [Baeldung – FetchMode in Hibernate](https://www.baeldung.com/hibernate-fetchmode)
- [Baeldung – JPA Entity Graph](https://www.baeldung.com/jpa-entity-graph)
- [Baeldung – Hibernate Second-Level Cache](https://www.baeldung.com/hibernate-second-level-cache)
- [Spring Data JPA Projections](https://docs.spring.io/spring-data/jpa/reference/repositories/projections.html)
- [Spring Data JPA EntityGraph API](https://docs.spring.io/spring-data/jpa/docs/current/api/org/springframework/data/jpa/repository/EntityGraph.html)
- [Thorben Janssen – DTO projections with JPA/Hibernate](https://thorben-janssen.com/dto-projections/)
- [Thorben Janssen – JPA 2.2 getResultStream](https://thorben-janssen.com/jpa-2-2s-new-stream-method-and-how-you-should-not-use-it/)
- [Vlad Mihalcea – Hibernate performance tuning](https://vladmihalcea.com/hibernate-performance-tuning-tips/)
- [Vlad Mihalcea – JPA 2.2 stream](https://vladmihalcea.com/whats-new-in-jpa-2-2-stream-the-result-of-a-query-execution/)
- [Vlad Mihalcea – DTO projection mapping](https://vladmihalcea.com/the-best-way-to-map-a-projection-query-to-a-dto-with-jpa-and-hibernate/)
- [jOOQ MULTISET blog](https://blog.jooq.org/jooq-3-15s-new-multiset-operator-will-change-how-you-think-about-sql/)
- [jOOQ MULTISET_AGG manual](https://www.jooq.org/doc/latest/manual/sql-building/column-expressions/aggregate-functions/multiset-agg-function/)
- [jOOQ – No more MultipleBagFetchException](https://blog.jooq.org/no-more-multiplebagfetchexception-thanks-to-multiset-nested-collections/)
- [Sequelize SELECT in depth](https://sequelize.org/docs/v7/querying/select-in-depth/)
- [Sequelize v6 Eager Loading](https://sequelize.org/docs/v6/advanced-association-concepts/eager-loading/)
- [Sequelize v7 IncludeOptions](https://sequelize.org/api/v7/interfaces/_sequelize_core.index.includeoptions)
- [TypeORM Relations FAQ](https://typeorm.io/docs/relations/relations-faq/)
- [TypeORM Caching queries](https://typeorm.io/docs/query-builder/caching/)
- [Prisma Relation queries](https://www.prisma.io/docs/orm/prisma-client/queries/relation-queries)
- [Prisma Query optimization](https://www.prisma.io/docs/orm/prisma-client/queries/query-optimization-performance)
- [Prisma Accelerate caching](https://www.prisma.io/docs/accelerate/caching)
- [Prisma Pagination](https://www.prisma.io/docs/orm/prisma-client/queries/pagination)
- [Drizzle Relational Queries v2](https://orm.drizzle.team/docs/rqb-v2)
- [Drizzle Relations v2](https://orm.drizzle.team/docs/relations-v2)
- [Mongoose Populate](https://mongoosejs.com/docs/populate.html)
- [Mongoose Lean tutorial](https://mongoosejs.com/docs/tutorials/lean.html)
- [Mongoose QueryCursor API](https://mongoosejs.com/docs/api/querycursor.html)
- [cachegoose npm](https://www.npmjs.com/package/cachegoose)
- [ts-cache-mongoose](https://github.com/ilovepixelart/ts-cache-mongoose)
- [EF Core Single vs Split Queries](https://learn.microsoft.com/en-us/ef/core/querying/single-split-queries)
- [EFCoreSecondLevelCacheInterceptor](https://github.com/VahidN/EFCoreSecondLevelCacheInterceptor)
- [GORM Preloading docs](https://gorm.io/docs/preload.html)
- [GORM Query docs](https://gorm.io/docs/query.html)
- [ent Eager Loading](https://entgo.io/docs/eager-load/)
- [Rails Active Record Querying guide](https://guides.rubyonrails.org/active_record_querying.html)
- [Rails Caching guide](https://guides.rubyonrails.org/caching_with_rails.html)
- [AppSignal – Russian doll caching](https://blog.appsignal.com/2018/04/03/russian-doll-caching-in-rails.html)
- [Saeloun – ActiveRecord N+1 includes/preload/eager_load](https://blog.saeloun.com/2020/01/08/activerecord-database-performance-n-1-includes-preload-eager-load-pluck/)
- [Eloquent get/chunk/lazy/cursor benchmark](https://janostlund.com/2021-12-26/eloquent-cursor-vs-chunk)
- [Doctrine Caching docs](https://www.doctrine-project.org/projects/doctrine-orm/en/3.6/reference/caching.html)
- [Doctrine Second Level Cache](https://www.doctrine-project.org/projects/doctrine-orm/en/3.2/reference/second-level-cache.html)
- [Doctrine DQL reference](https://www.doctrine-project.org/projects/doctrine-orm/en/3.6/reference/dql-doctrine-query-language.html)
- [Tideways – Doctrine performance traps](https://tideways.com/profiler/blog/5-doctrine-orm-performance-traps-you-should-avoid)
- [NHibernate Performance ch.21](https://nhibernate.info/doc/nhibernate-reference/performance.html)
- [NHibernate BatchFetchStyle Javadoc](https://docs.jboss.org/hibernate/orm/5.0/javadocs/org/hibernate/loader/BatchFetchStyle.html)agentId: aa4657473585fa546 (use SendMessage with to: 'aa4657473585fa546' to continue this agent)
<usage>total_tokens: 95474
tool_uses: 38
duration_ms: 490253</usage>