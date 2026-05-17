I have comprehensive data. Now let me write up the final report.

---

# State-of-the-Art Static Detection: Niche & Second-Tier ORMs / Query Builders / ODMs

Audience: drift-static-profiler rule-catalog authors. Every entry below is structured for the same rule shape you already use: an import set, a receiver/call signature to match, a loop-body shape, and (where relevant) a counter-pattern (the fix). All license info refers to OSS detector tools, not the ORMs themselves.

---

## 1. Python (beyond Django / SQLAlchemy / Tortoise)

### 1.1 SQLModel (Tiangolo — SQLA + Pydantic)
SQLModel inherits everything from SQLAlchemy 2.x, so the entire `selectinload` / `joinedload` / `subqueryload` / `contains_eager` / `raiseload` matrix applies verbatim. The N+1 footgun is unique though: developers commonly write `session.exec(select(Hero)).all()` and then iterate `hero.team.name` — relationships are lazy by default. The fix is `select(Hero).options(selectinload(Hero.team))`.
- **N+1 detect API**: `lazy="raise"` on the `Relationship(sa_relationship_kwargs={"lazy": "raise"})`, or `raiseload(Hero.team)` per-query.
- **Eager primitives**: `selectinload`, `joinedload`, `subqueryload`, `contains_eager`, `with_loader_criteria`. All imported from `sqlalchemy.orm`.
- **Bulk writes**: `session.add_all([...])`, `session.exec(insert(Hero).values([...]))`, `Session.bulk_insert_mappings`.
- **Tree-sitter shape**: `for x in <result>: ... x.<rel>...` where `<result>` came from `Session.exec` or `session.execute`. Import set: `from sqlmodel import` / `from sqlalchemy.orm import`.
- **OSS detector**: `kolypto/py-nplus1loader` (MIT) is the closest thing — a runtime SQLA loader that flags N+1. ([SQLModel Read Relationships](https://sqlmodel.tiangolo.com/tutorial/relationship-attributes/read-relationships/), [SQLA loading techniques](https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html), [py-nplus1loader](https://github.com/kolypto/py-nplus1loader))

### 1.2 Peewee
- **Headline anti-patterns**: "select-and-iterate-FK", and mixing `aggregate_rows()` with `LIMIT/OFFSET/get()` which silently returns wrong counts.
- **N+1 detect**: none built-in. Peewee debug logger only.
- **Eager primitives**: `prefetch(query, Tweet)` (subquery-style, one query per table); `.join(Author).select(Tweet, Author)` for many-to-one; `aggregate_rows()` (legacy, dangerous).
- **Bulk**: `Model.insert_many(rows).execute()`, `Model.bulk_create(list_of_instances, batch_size=N)`, `bulk_update`.
- **Tree-sitter shape**: import `import peewee` or `from peewee import`. Look for `.select()` results iterated where loop body accesses a `ForeignKeyField` attribute. Counter-pattern: presence of `prefetch(` in same scope.
- ([Peewee querying](https://docs.peewee-orm.com/en/latest/peewee/querying.html), [Peewee relationships](https://docs.peewee-orm.com/en/latest/peewee/relationships.html))

### 1.3 Pony ORM
- **Headline anti-pattern**: list comprehension instead of generator expression inside `select()` — Pony will throw at runtime; static catch is a freebie.
- **Eager primitives**: `Query.prefetch(Entity, Entity.attr, ...)`; `lazy=True` on `Required`/`Optional` for selective deferral; `db_session` boundary controls.
- **N+1 detect**: nothing first-party.
- **Bulk**: `db.execute()` raw; no `bulk_create`. Pony's identity map deduplicates in memory.
- **Tree-sitter shape**: `select(x for x in Entity)` (call to `select` with a generator-exp argument). Detect missing `.prefetch(...)` chained before iteration. Imports: `from pony.orm import`.
- ([Pony API reference](https://docs.ponyorm.org/api_reference.html), [Pony prefetch issue #172](https://github.com/ponyorm/pony/issues/172))

### 1.4 Beanie (Pydantic + Motor MongoDB ODM)
- **Headline anti-pattern**: iterating documents whose fields are typed `Link[Other]` without `fetch_links=True` — accessing `.x.field` issues a separate `find_one`. There's also a runaway-fetch trap when `nesting_depth` defaults are left high on cyclic graphs.
- **Eager primitives**: `Model.find(..., fetch_links=True, nesting_depth=N)`, `nesting_depths_per_field={"field": 1}`, `document.fetch_link(field)` for one-off resolves, `document.fetch_all_links()`.
- **N+1 detect**: none.
- **Bulk**: `Model.insert_many([docs])`, `await Model.find(...).update(...)`, `Model.delete_all`, `BulkWriter` context manager.
- **Tree-sitter shape**: `Model.find(...)` / `find_one(...)` without `fetch_links=` kwarg, iterated in a loop touching a `Link`-typed attribute. Imports: `from beanie import Document, Link`.
- ([Beanie relations docs](https://github.com/BeanieODM/beanie/blob/main/docs/tutorial/relations.md), [Beanie query API](https://beanie-odm.dev/api-documentation/query/))

### 1.5 MongoEngine
- **Headline**: classic "dereference on access" N+1; `select_related()` only goes 1 level deep by default.
- **Eager primitives**: `.select_related(max_depth=N)`, the inverse `no_dereference()` context manager, and `QuerySet.no_dereference()`.
- **Bulk**: `Model.objects.insert(list, load_bulk=False)`, `bulk_write` via PyMongo escape hatch.
- **Tree-sitter shape**: `Model.objects(...)` (or `.objects.filter(...)`) iterated where body touches a `ReferenceField`-named attr. Import: `from mongoengine import`.
- ([MongoEngine querying](https://mongoengine-odm.readthedocs.io/guide/querying.html))

### 1.6 Edgy (Tarsil, async, SQLA-fork-style)
Edgy mirrors ormar's API: `Model.query.select_related("user").all()`, `Model.query.prefetch_related("books")`. Anti-patterns and detection signatures are the same as ormar (1.7) since both target FastAPI shops. Import path: `from edgy import`.

### 1.7 ormar + encode/databases
- **Headline anti-pattern**: calling `.all()` then awaiting attribute access on a relation — relations are `None` unless eager-loaded. Also: combining `iterate()` with `prefetch_related()` raises `QueryDefinitionError`.
- **Eager primitives**: `Model.objects.select_related("author")` (JOIN, FK + M2M); `prefetch_related("comments")` (one extra query per relation); nested dot syntax `select_related("author__books")`.
- **Bulk**: `Model.objects.bulk_create([...])`, `bulk_update`.
- **Tree-sitter shape**: `await Model.objects.all()` / `.get()` / `.first()` without preceding `.select_related(` / `.prefetch_related(`, iterated touching a relation attribute. Import: `import ormar`.
- ([ormar queries](https://collerek.github.io/ormar/latest/queries/read/), [ormar prefetch](https://collerek.github.io/ormar/latest/api/queryset/queries/prefetch_query/))

### 1.8 Piccolo (async Postgres ORM)
- **Eager primitives**: `Table.objects(Table.fk).run()` to pass FK columns directly; equivalent `.prefetch(Table.fk)` chain; `all_related()` to grab every FK in one shot.
- **Bulk**: `Table.insert(*rows).run()` (variadic), `MyTable.raw(...)`.
- **Tree-sitter shape**: `await Model.objects().run()` then iterating touching `.fk_field.something`. Import: `from piccolo.table import Table`, `from piccolo.columns import ForeignKey`.
- ([Piccolo objects](https://piccolo-orm.readthedocs.io/en/latest/piccolo/query_types/objects.html))

### 1.9 PynamoDB (DynamoDB)
- **Anti-pattern**: per-item `Model.get(pk)` in a loop instead of `Model.batch_get([(pk1, sk1), ...])`. DynamoDB throughput cost is the killer.
- **Batch primitives**: `Model.batch_get(item_keys)`, `Model.batch_write() as batch:` context manager — PynamoDB auto-paginates the 25-item PutRequest/DeleteRequest limit.
- **Tree-sitter shape**: `for ... in <iterable>: Model.get(...)` or `Model(...).save()` calls in a loop body. Import: `from pynamodb.models import Model`.
- ([PynamoDB batch ops](https://pynamodb.readthedocs.io/en/stable/batch.html))

### 1.10 Redis-OM (Python)
- **Anti-pattern**: per-item `Model.get(pk)` in a loop. No first-party batch; users must drop to a redis pipeline (`client.pipeline()`). Embedded models only work with `JsonModel`, not `HashModel`.
- **Tree-sitter shape**: `Model.get(<var>)` inside a loop without `pipeline()` context. Imports: `from redis_om import HashModel, JsonModel`.
- ([Redis-OM Python models docs](https://github.com/redis/redis-om-python/blob/main/docs/models.md))

---

## 2. Java / Kotlin (beyond Hibernate / Spring Data / jOOQ)

### 2.1 MyBatis / MyBatis-Plus
- **Headline anti-patterns**: `<association select=...>` and `<collection select=...>` with `fetchType="lazy"` (default) trigger one extra SELECT per parent. Forgetting `resultOrdered="true"` on nested-result collections wastes memory.
- **Detect / mitigate**: `lazyLoadingEnabled=true` + `aggressiveLazyLoading=false`; `defaultLazyLoadingBatchSize` setting batches lazy loads via `IN` clauses (N → N/batch).
- **Eager primitives**: nested `<resultMap>` with `<collection>` ofType + a single SELECT JOIN; or annotation `@Many(select="...", fetchType=FetchType.EAGER)`.
- **Bulk**: `<insert>` with `<foreach>`; MyBatis-Plus `IService.saveBatch(list)`, `saveOrUpdateBatch`.
- **Tree-sitter shape**: hard to do purely on Java AST (the XML matters). For Java: detect Mapper interface calls inside `for/while` whose return type has lazy nested fields. For XML, separate tree-sitter-xml pass: find `<association select=` / `<collection select=` without `fetchType="eager"`. Imports: `org.apache.ibatis.annotations.*` / `com.baomidou.mybatisplus.*`.
- ([MyBatis XML mappers](https://mybatis.org/mybatis-3/sqlmap-xml.html), [PingCAP MyBatis N+1](https://www.pingcap.com/article/how-to-efficiently-solve-the-n1-query-problem/))

### 2.2 Ebean ORM (Avaje)
- **Headline**: relies on AutoTune; un-tuned queries silently emit 1 + N/10. The "N+1" is masked into "1 + N/batchSize".
- **Eager primitives**: `query.fetch("orders")`, `fetchLazy()`, `fetchQuery("orders", new FetchConfig().query())`, `setLazyLoadBatchSize(int)`. AutoTune profiler can auto-tune.
- **Bulk**: `DB.saveAll(list)`, `DB.executeBatch`, `Database.update(beanType).set(...).update()`.
- **Tree-sitter shape**: receiver chains on `io.ebean.DB` / `io.ebean.Database` / `io.ebean.Query` without a `.fetch(` or `.fetchQuery(` modifier, followed by iteration over the result list. Imports: `io.ebean.*`.
- ([Ebean N+1 docs](https://ebean.io/docs/query/background/nplus1), [Ebean fetch](https://ebean.io/docs/query/fetch))

### 2.3 EclipseLink
- **Eager primitives**: `@JoinFetch(JoinFetchType.OUTER)`, `@BatchFetch(value=BatchFetchType.IN, size=N)`, JPQL hints `eclipselink.join-fetch`, `eclipselink.batch`, `eclipselink.batch.type` (IN | EXISTS | JOIN), `eclipselink.batch.size`. Default fetch group via `@FetchGroup`.
- **Tree-sitter shape**: identical to Hibernate detection — JPA `EntityManager.find/createQuery` in a loop body that touches a non-fetched lazy field. Annotation lookups on the entity classes (`@OneToMany(fetch=LAZY)`) are the cross-reference.
- ([EclipseLink @BatchFetch](https://eclipse.dev/eclipselink/documentation/2.5/jpa/extensions/a_batchfetch.htm))

### 2.4 OpenJPA
- **Concepts**: `@FetchGroup`/`@FetchAttribute` + `MaxFetchDepth` config; well-documented N+1 with `OneToOne`/`ManyToOne` even with eager fetch (issue OPENJPA-2296).
- **Tree-sitter shape**: same as JPA generally. Annotation detection on entities matters more than call-site shape.
- ([OpenJPA fetch groups](https://openjpa.apache.org/builds/1.2.3/apache-openjpa/docs/ref_guide_fetch.html))

### 2.5 Querydsl (separate from jOOQ)
- **Anti-pattern**: `fetchJoin()` on `OneToMany` returns duplicated rows; naïve fix `distinct()` adds SQL DISTINCT which kills pagination. Set Hibernate hint `hibernate.query.passDistinctThrough=false` since Hibernate 5.2.
- **Detect target**: `JPAQuery<>(em).from(qX).leftJoin(qX.children).fetchJoin().distinct().fetch()` chains. Also catch missing `fetchJoin()` when loop later touches `qX.children`.
- **Imports**: `com.querydsl.jpa.impl.JPAQuery`, `com.querydsl.core.types.*`.
- ([Querydsl + fetchJoin Korean blog](https://madplay.github.io/post/avoid-n+1-problem-in-jpa-using-querydsl-fetchjoin))

### 2.6 Spring Data JPA Specifications (criteria misuse)
- **Anti-pattern**: building `Specification<>` with `root.join(...)` then expecting eager loading — `Join` does **not** trigger fetch. Must use `root.fetch(...)`.
- **Tree-sitter shape**: any `.join(` on `Root`/`From` without a sibling `.fetch(` call in the same Specification body, followed by relationship-attribute access on results. Import: `org.springframework.data.jpa.domain.Specification`.

### 2.7 Kotlin Exposed
- **Anti-pattern**: DAO entities are always lazy. Iterating `Users.all()` then touching `.histories` issues N queries.
- **Eager primitives**: `entity.load(Entity::relation)` for single, `entities.with(Entity::rel1, Entity::rel2)` extension function for collections. No global batch_size; explicit `batchInsert`.
- **Tree-sitter shape**: `for x in Users.all()` (or `.find { }`) body accesses a `referencedOn`/`backReferencedOn`/`referrersOn`-defined property, without preceding `.with(` call. Imports: `org.jetbrains.exposed.dao.*`.
- ([Exposed guide](https://www.jetbrains.com/help/exposed/about.html), [Hibernate vs Exposed](https://softwaremill.com/hibernate-vs-exposed-choosing-kotlins-best-persistence-tool-round-3/))

### 2.8 Ktorm
- **Anti-pattern**: relying on `references()` auto-join can produce unwanted JOINs every query. Conversely, dropping `refs` access loses the JOIN and forces follow-up queries.
- **Mitigations**: explicit `database.from(Employees).leftJoin(Departments, on=...).select()` plus `it.refs` access.
- **Imports**: `org.ktorm.dsl.*`, `org.ktorm.entity.*`.
- ([Ktorm joining](https://www.ktorm.org/en/joining.html))

### 2.9 Komapper (Kotlin, JDBC + R2DBC)
- **Primitives**: `QueryDsl.from(d).innerJoin(e) { d.id eq e.departmentId }.include(d, e)` or `.includeAll()`. Then `store[d]`, `department.employees(store)` navigate.
- **Anti-pattern**: not specifying `include` returns flat join rows but the Association API helpers won't populate.
- **Imports**: `org.komapper.core.dsl.*`, generated `_<Entity>Def`.
- ([Komapper association](https://www.komapper.org/docs/reference/association/))

### 2.10 Spring Data R2DBC, Vert.x SQL Client, Hibernate Reactive
- **Hibernate Reactive (Mutiny)**: `Mutiny.Session` does **not** support transparent lazy fetch — accessing a non-fetched association throws. Use `session.fetch(entity.getThing())`, HQL `join fetch`, or an `EntityGraph`. Imports: `org.hibernate.reactive.mutiny.Mutiny.*`.
- **Spring R2DBC**: relationships are not first-class — there is *no* lazy loading at all; you write the join. The anti-pattern is concurrent `.flatMap` that issues N queries: detect `.flatMap(x -> repo.findById(x.fk))` shape on `Flux`.
- **Vert.x SQL Client**: `client.preparedQuery(sql).executeBatch(tuples)` is the batch primitive. Anti-pattern: `for (...) client.preparedQuery(sql).execute(Tuple.of(id))`.
- ([Hibernate Reactive 1.1 ref](https://hibernate.org/reactive/documentation/1.1/reference/html_single/), [Spring R2DBC](https://docs.spring.io/spring-framework/reference/data-access/r2dbc.html))

### 2.11 Speedment
- Streams-of-tables ORM. `JoinComponent` joins up to 10 tables in a type-safe builder. N+1 manifests when devs `.forEach` a stream and call `manager.byId(...)` per element. Imports: `com.speedment.runtime.join.*`. ([Speedment joins](https://speedment.github.io/speedment-doc/join.html))

### 2.12 Detector tooling for the JVM family
- `yannbriancon/spring-hibernate-query-utils` (MIT) — runtime N+1 detection via proxy.
- `vladmihalcea/hypersistence-utils` (Apache-2.0) — ships `SQLStatementCountValidator` for test-time N+1 assertions. ([list](https://github.com/yannbriancon/spring-hibernate-query-utils))

---

## 3. Node / TypeScript (beyond Sequelize/TypeORM/Prisma/Drizzle/Mongoose)

### 3.1 Knex.js
- **Anti-pattern**: `await knex('users').then(rows => rows.map(u => knex('posts').where({user_id: u.id})))`. Pure query builder so no relations layer to "fix" it.
- **Mitigations**: `knex('posts').whereIn('user_id', ids)` + group in JS; `knex.raw` with CTEs; `knex.batchInsert(table, rows, chunkSize)` for writes.
- **Tree-sitter shape**: receiver pattern `knex(<string>)` or `db.<table>` inside any loop body. Imports: `import knex from 'knex'`.
- ([Knex query builder](https://knexjs.org/guide/query-builder.html))

### 3.2 Objection.js
- **Primitives**: `Model.query().withGraphFetched('[parts.sub]')` (multi-query, per relation), `withGraphJoined('[parts.sub]')` (single SQL with JOINs). Deprecated alias: `.eager(...)`.
- **Anti-pattern**: `withGraphJoined` on M-N causes Cartesian explosion; `withGraphFetched` is usually safer.
- **Bulk**: `Model.query().insert([list])`, `insertGraph(graph)`, `upsertGraph(graph)` — atomic multi-table inserts.
- **Tree-sitter shape**: `Model.query().<find/findById/where>...await` whose result is iterated touching a relation getter without preceding `.withGraph(F|J)etched(`. Imports: `import { Model } from 'objection'`.
- ([Objection eager methods](https://vincit.github.io/objection.js/api/query-builder/eager-methods.html))

### 3.3 MikroORM
- **Anti-pattern**: relationships are `Reference<T>` / `Collection<T>` proxies. Touching `.title` on a non-initialized `Reference` returns undefined; touching `.getItems()` on a non-init collection in v5+ throws (when in strict mode).
- **Primitives**: `em.find(Author, {}, { populate: ['books', 'books.tags'] })`, `wrap(entity).init()` for single, `em.populate(authors, ['books'])` for batch hydrate. `populateWhere: 'all'|'infer'` and v5's `loadStrategy: 'select-in'|'joined'`.
- **Detect**: `validateRequired` and `forceUndefined` flags; `strict: true` in v5 makes uninitialized access loud.
- **Bulk**: `em.upsertMany(EntityClass, [list])`, `em.insertMany`, `nativeUpdate`. EM batches all `em.persist` calls into one flush thanks to Identity Map + UoW.
- **Tree-sitter shape**: `em.find(...)` / `em.findOne(...)` without `populate:` in the options object literal, followed by `.<relation>.get()` access. Imports: `import { EntityManager } from '@mikro-orm/core'`.
- ([MikroORM 5 announcement](https://dev.to/b4nan/mikroorm-5-stricter-safer-smarter-l4g), [MikroORM IdMap](https://mikro-orm.io/docs/identity-map))

### 3.4 Bookshelf.js (legacy, still seen)
- **Primitives**: `Model.fetchAll({withRelated: ['author', 'tags']})`, `.fetch({withRelated: 'author'})`, `.load(['author'])` on collections.
- **Tree-sitter shape**: `Model.collection().fetchAll(` with no `withRelated`, or `.fetch(` without it, where result then accesses `.related('x')`. Imports: `require('bookshelf')`.
- ([Bookshelf API](https://bookshelfjs.org/api.html))

### 3.5 Waterline (Sails)
- **Primitive**: `.populate('comments', {where:{...}, limit:N, sort:'createdAt DESC'})`. Cross-datastore populate is unsafe; deep populate unsupported.
- **Tree-sitter shape**: `await Model.find(...)` without chained `.populate(`, iterating with access to an association field. Imports: `sails.models.<x>`, `Model.find`.
- ([Waterline populate](https://sailsjs.com/documentation/reference/waterline-orm/queries/populate))

### 3.6 Kysely
- Not an ORM — no relations concept. Idiomatic N+1 mitigation is the helpers `jsonObjectFrom` / `jsonArrayFrom` from `kysely/helpers/postgres` (or `mysql`/`sqlite`).
- **Anti-pattern**: `const users = await db.selectFrom('user').selectAll().execute(); for (const u of users) { ... await db.selectFrom('post').where('user_id','=',u.id) }`.
- **Tree-sitter shape**: `await db.selectFrom(...)` followed by `for/map` whose body calls `db.selectFrom(...)`. Counter pattern: `jsonArrayFrom(eb.selectFrom(`. Imports: `from 'kysely'`, `from 'kysely/helpers/postgres'`.
- ([Kysely relations recipe](https://kysely.dev/docs/recipes/relations))

### 3.7 Mongoose (deeper than already covered)
- **Anti-patterns**: chained `populate('a').populate('b').populate('c')` on cold cache; populating after `lean()` without `mongoose-lean-virtuals`; nested `populate({path:'a', populate:{path:'b'}})` is fine but `Document#populate()` does not chain — only `Query#populate` does. `refPath` populates require the foreign collection to be derivable; bad refPath returns nulls silently.
- **Detection**: enable `mongoose.set('debug', true)`; the community plugin `mongoose-autopopulate` is its own anti-pattern (hidden N+1).
- **Bulk**: `Model.insertMany([docs], {ordered:false})`, `Model.bulkWrite([{insertOne}, {updateOne}, ...])`.
- **Tree-sitter shape (new)**: `await Model.find(...).lean()` followed by `Model.findById(x.refId)` in a loop. Or virtual access `.get('myVirtual')` after `.lean()` without `mongoose-lean-virtuals`. Imports: `import { Schema, model } from 'mongoose'`.
- ([Mongoose populate](https://mongoosejs.com/docs/populate.html), [Mongoose lean](https://mongoosejs.com/docs/tutorials/lean.html))

### 3.8 DataLoader (the canonical fix layer)
- **Pattern**: `new DataLoader(async (keys) => batchFn(keys))` then `await loader.load(id)`. Batches all `.load()` calls in same tick.
- **Static detection angle**: presence of DataLoader is the *fix* signature. Two useful rules:
  1. GraphQL resolver file (`*.resolver.ts`, `Query/Mutation/Subscription` exports) where a field resolver `await`s a non-DataLoader fetch (e.g., a Prisma `findUnique` per parent) → N+1 risk.
  2. Loader instantiated in module scope rather than per-request — caches stale data across requests.
- Imports: `import DataLoader from 'dataloader'`.
- ([DataLoader repo](https://github.com/graphql/dataloader))

### 3.9 Edge / Cloudflare D1
- **Pattern**: `db.batch([stmt1, stmt2, ...])` for atomic batches. Avoid `for await { await stmt.bind(id).run() }`.
- **Tree-sitter shape**: receiver `env.DB.prepare(...)` inside loop body. Counter: presence of `env.DB.batch(`. Imports: `D1Database` binding type from `@cloudflare/workers-types`.
- ([Cloudflare D1 binding API](https://developers.cloudflare.com/d1/worker-api/d1-database/))

---

## 4. Go (beyond GORM / ent / sqlc / sqlx)

### 4.1 SQLBoiler
- **Anti-pattern**: `for _, j := range jets { j.Pilot().OneP(ctx, db) }` (or `One()`) — the docs literally warn about this. Fix: `models.Jets(qm.Load("Pilot")).All(ctx, db)`.
- **Primitives**: `qm.Load("Pilot")`, type-safe `qm.Load(models.JetRels.Pilot)`, nested `qm.Load("Pilot.Languages")`, filter-during-load `qm.Load("Pilot", Where("active=?", true))`.
- **Bulk**: `boil.SetDB`; insert via `for ... Insert()` — boiler is per-row. Use `pgx`/`sqlx`'s `CopyFrom` for true bulk.
- **Tree-sitter shape**: receiver `<RelName>().One(...)`/`...All(...)` or `<RelName>P(...)` inside `range` over models. Imports: `github.com/volatiletech/sqlboiler/v4/boil`.
- ([SQLBoiler eager_load.go](https://github.com/volatiletech/sqlboiler/blob/master/queries/eager_load.go))

### 4.2 Beego ORM
- **Primitives**: `o.LoadRelated(&user, "Posts")` (one entity), `qs.RelatedSel("Profile").All(&users)` (eager-load FK), `PrepareInsert` for batch.
- **Anti-pattern**: looping `o.Read(&user); o.LoadRelated(&user, "Posts")` — equivalent of N+1 but explicit.
- **Imports**: `github.com/beego/beego/v2/client/orm`.

### 4.3 Bun (uptrace)
- **Primitive**: `db.NewSelect().Model(&users).Relation("Posts").Scan(ctx)` — explicit eager. Closure variant: `Relation("Posts", func(q *bun.SelectQuery) *bun.SelectQuery { return q.Where("published") })` pushes filter into SQL.
- **Anti-pattern**: forgetting `Relation()` and accessing `users[i].Posts` (nil); or per-row `Model(&u).WherePK().Relation(...).Scan()` in a loop.
- **Bulk**: `db.NewInsert().Model(&users).Exec(ctx)` works on slices.
- **Tree-sitter shape**: `db.NewSelect().Model(&<slice>).Scan(` without `.Relation(`, followed by range loop touching a relation field. Imports: `github.com/uptrace/bun`.
- ([Bun relations](https://bun.uptrace.dev/guide/relations.html))

### 4.4 go-pg (deprecated upstream; still in production code)
- **Primitive**: `db.Model(&users).Relation("Profile").Select()` — same shape as Bun (Bun is its successor).
- **Imports**: `github.com/go-pg/pg/v10/orm`.

### 4.5 xorm
- **Primitive**: `Join("LEFT", "user", "user.id = post.user_id")` + `Find(&result)` — no relation system, joins are manual. `Iterate` for big streams.
- **Anti-pattern**: `engine.ID(id).Get(&user)` per row in loop, instead of `engine.In("id", ids).Find(&users)`.
- **Imports**: `xorm.io/xorm`.

### 4.6 upper/db
- No relations layer. N+1 detection requires recognizing per-row `Find(...)`/`One(...)` calls inside `for/range`. Imports: `github.com/upper/db/v4`.

### 4.7 pgx (jackc, low-level)
- **Anti-pattern**: per-row `conn.Query("SELECT ... WHERE id=$1", id)` in loop.
- **Fix primitives**: `pgx.CollectRows(rows, pgx.RowToStructByName[T])` and `Batch{}` + `Queue(sql, args...)` + `conn.SendBatch(ctx, &b)`.
- **Tree-sitter shape**: `conn.Query(` or `conn.QueryRow(` inside `for`/`range`. Counter: presence of `pgx.Batch{}` or `conn.SendBatch(`. Imports: `github.com/jackc/pgx/v5`.
- ([pgx package docs](https://pkg.go.dev/github.com/jackc/pgx/v5))

### 4.8 Pop (Buffalo)
- **Modes**: `tx.Eager()` (default — N queries per association, "EagerDefault"), `tx.EagerPreload()` (single query per association — fewer DB hits, more memory). Issue #46 is the canonical reference.
- **Tree-sitter shape**: `tx.All(&list)` without preceding `.Eager()` / `.EagerPreload()` followed by struct-field access on association. Imports: `github.com/gobuffalo/pop/v6`.
- ([Pop N+1 PR #146](https://github.com/gobuffalo/pop/pull/146), [Buffalo relations](https://gobuffalo.io/documentation/database/relations/))

---

## 5. Rust

### 5.1 Diesel (sync)
- **Anti-pattern**: `books.load::<Book>(conn)?` then `pages = pages.filter(book_id.eq(book.id)).load(conn)` in a `for` loop. Diesel makes the fix explicit and the bug obvious.
- **Fix primitives**: `Page::belonging_to(&books).load::<Page>(conn)?.grouped_by(&books)` — IN-clause loader + grouped pairing. Zip with parents to produce `Vec<(Book, Vec<Page>)>`.
- **Bulk**: `diesel::insert_into(table).values(&vec_of_inserts).execute(conn)` — Diesel batches.
- **Tree-sitter shape**: `BelongsTo::belonging_to(&<entity>)` is the *fix* marker. The anti-pattern is calling `<assoc_table>.filter(<fk>.eq(parent.id)).load(` inside `for x in <parents>`. Imports: `use diesel::prelude::*;`, `use diesel::associations::*;`.
- ([Diesel relations](https://diesel.rs/guides/relations.html))

### 5.2 SeaORM
- **Primitives**: `Entity::find().find_with_related(Other)` (1-N or M-N, single tuple result); `find_also_related(Other)` (1-1); `Entity::find().all(db).await?` then `parents.load_many(OtherEntity, db).await?` (data-loader pattern).
- **Detection trick**: SeaORM cannot eager-load more than 2 entities together; deeper graphs require chained `load_many` / `load_one` calls. The docs explicitly recommend Seaography for deep nesting.
- **Anti-pattern**: `for parent in parents { parent.find_related(Other).all(db).await? }`.
- **Bulk**: `Entity::insert_many(models).exec(db).await?`.
- **Tree-sitter shape**: `.find_related(` or `.find_linked(` inside `for` loops. Imports: `use sea_orm::*;`.
- ([SeaORM data loader](https://www.sea-ql.org/SeaORM/docs/relation/data-loader/), [Entity loader](https://www.sea-ql.org/SeaORM/docs/relation/entity-loader/))

### 5.3 sqlx (macro deep-dive)
- **Anti-pattern**: per-row `sqlx::query_as!(Child, "SELECT ... WHERE parent_id = $1", p.id).fetch_one(pool).await?` inside `for`. Often hidden behind `try_join_all(parents.iter().map(|p| async { ... }))`.
- **Fix**: `WHERE parent_id = ANY($1::bigint[])` plus `Vec<i64>` of parent ids.
- **Tree-sitter shape**: `sqlx::query` / `query_as` / `query!` / `query_as!` macro invocation inside `for`/`while`/`map+future` chain. Imports: `use sqlx::`.

### 5.4 SeaORM-based Loco (Rails-like)
- Re-exports SeaORM. Same detection signatures; just the import paths include `loco_rs::`.

### 5.5 diesel-async
- Identical to Diesel semantics; the only difference is `.await` on every query. Same `belonging_to` / `grouped_by` fix.

### 5.6 ormlite (kurtbuilds)
- **Primitive**: `Person::select().join(Person::organization()).fetch_all(&pool).await?`. M-to-1 only; M-to-N and 1-to-N planned.
- **Anti-pattern**: per-row `Person::fetch_one(...)` in a stream.
- **Imports**: `use ormlite::*;`. ([ormlite crate](https://docs.rs/ormlite))

### 5.7 Welds
- Similar to ormlite: explicit `.include(...)` API. Detector should treat as ormlite for tree-sitter purposes.

### 5.8 rbatis
- MyBatis-style XML mappers + Rust macros. Same XML detection rules as MyBatis (Section 2.1) apply: `<select>` with nested `<association>`/`<collection>` of lazy fetchType. Imports: `use rbatis::*;`, `#[html_sql(...)]`, `#[py_sql(...)]`. ([rbatis repo](https://github.com/rbatis/rbatis))

---

## 6. PHP (beyond Eloquent / Doctrine)

### 6.1 Eloquent strict mode (Laravel 9+)
- **First-party N+1 prevention**: `Model::preventLazyLoading(! $this->app->isProduction())` in `AppServiceProvider::boot`. Throws `Illuminate\Database\LazyLoadingViolationException`. Production override via `Model::handleLazyLoadingViolationUsing(fn($model, $rel) => Log::warning(...))`.
- Also: `preventSilentlyDiscardingAttributes`, `preventAccessingMissingAttributes`. Useful sister rules.
- **Tree-sitter shape**: presence of `preventLazyLoading(` in any provider/bootstrap file = strict mode is on; treat lazy-relation access in that codebase as a hard error to surface. Imports: `use Illuminate\Database\Eloquent\Model;`.
- ([Laravel News on strict](https://laravel-news.com/disable-eloquent-lazy-loading-during-development))

### 6.2 Doctrine ORM (deeper)
- **Headline antipatterns**: `fetch="EAGER"` on `OneToMany`/`ManyToMany` still produces N+1 in DQL because Doctrine cannot JOIN-fetch lazily-mapped collections from one query without `JOIN FETCH`. `partial` objects are deprecated. **Lazy ghosts** (PHP 8.4 native lazy proxies) replaced classic proxies via `enable_lazy_ghost_objects=true`.
- **Fix primitives**: DQL `SELECT u, a FROM User u JOIN FETCH u.address a`; `QueryBuilder->leftJoin('u.posts','p')->addSelect('p')`; `EXTRA_LAZY` for huge collections (count/contains skip hydration); DTO via `SELECT NEW App\Dto\X(u.id, u.name) FROM User u`.
- **Tree-sitter shape**: QueryBuilder calls with `->leftJoin(` or `->innerJoin(` but missing matching `->addSelect(` for the joined alias = N+1 risk if those relations are touched. Imports: `use Doctrine\ORM\QueryBuilder;`.
- ([Tideways performance traps](https://tideways.com/profiler/blog/5-doctrine-orm-performance-traps-you-should-avoid))

### 6.3 Cycle ORM (Spiral)
- **Primitives**: `->load('tags')`, `->load('tags.posts')` (dot for nesting), `->load('tags', ['where' => ['active' => true], 'orderBy' => ['name' => 'ASC']])` for sub-criteria; `->with('tags')` for inner-join filtering. Attribute-level: `#[ManyToMany(target: Tag::class, through: UserTag::class, load: 'eager')]`.
- **Imports**: `use Cycle\ORM\Select;`, `use Cycle\Annotated\Annotation\Relation\*`.
- ([Cycle eager loading docs](https://spiral.dev/docs/orm-loading/1.0/en))

### 6.4 RedBeanPHP
- **Pattern**: zero-config; lazy own-list access `$user->ownPost` triggers query. No batched eager primitive — `R::preload($users, 'ownPost')` exists but is for convenience.
- **Tree-sitter shape**: receiver `->own<Pascal>List` / `->shared<Pascal>List` / `->fetchAs(...)` inside `foreach` loop. Imports: `use RedBeanPHP\R;`.
- ([RedBean eager loading](http://www.redbeanphp.com/manual3_0/eager_loading))

### 6.5 Atlas.Orm
- Aura/Atlas pattern: `$mapper->select()->with(['orders' => function ($s) { ... }])->fetchRecordSet()`. Per-record `getOrders()` lazy-fires; presence of `->with([` is the fix.
- Imports: `use Atlas\Orm\Atlas;`.

### 6.6 Propel
- **Primitive**: `BookQuery::create()->joinWithAuthor()` (generated per-relation), `joinWith('Book.Author')` (string form), nested `joinWith('Review.Book')->joinWith('Book.Author')->joinWith('Book.Publisher')`.
- **Gotcha**: `with()` + `limit()` on one-to-many returns wrong related counts (documented).
- **Bulk**: `BookQuery::create()->update([...])`; Propel does not have explicit bulkCreate — emits multi-INSERT batched at PDO level.
- **Imports**: generated `<Entity>Query` classes; base `Propel\Runtime\ActiveQuery\Criteria`. ([Propel relationships](https://propelorm.org/documentation/04-relationships.html))

### 6.7 Pomm
- Postgres-native; explicit SQL. Same N+1 detection as raw SQL — per-row `$model->findByPk($id)` in `foreach`. Imports: `use PommProject\ModelManager\*`.

### 6.8 Yii2 ActiveRecord
- **Primitives**: `Customer::find()->with('orders')` (separate query, eager), `joinWith('orders')` (JOIN; second arg `false` disables eager and only joins for filtering), `innerJoinWith('books', false)`.
- **Tree-sitter shape**: `<Model>::find()` chained without `->with(` and `->all()` results iterated for relation property access. Receiver pattern in PHP tree-sitter is fairly clean. Imports: `use yii\db\ActiveRecord;`. ([Yii AR](https://www.yiiframework.com/doc/guide/2.0/en/db-active-record))

### 6.9 CodeIgniter Query Builder
- No relations layer. Same as Knex: detect `$this->db->get(...)` or `$this->db->where(...)->get()` inside `foreach`. Imports: `$this->db`, `\CodeIgniter\Database\BaseBuilder`.

---

## 7. Ruby (beyond ActiveRecord)

### 7.1 Sequel (Jeremy Evans)
- **Primitives**: `Artist.eager(albums: {tracks: :lyric})` (4 queries, no JOIN duplication), `Artist.eager_graph(albums: {tracks: :lyric})` (single query w/ JOINs — supports filtering on associated columns).
- **Plugins**: `forbid_lazy_load` — turns any lazy-load attempt into an exception (the Sequel equivalent of Eloquent's strict mode). `eager_graph_eager` — chain `.eager` onto `eager_graph` loads.
- **Bulk**: `DB[:items].multi_insert(rows)`, `import` (alias), `Model.dataset.update(...)`.
- **Tree-sitter shape**: `<Model>.all` / `.first` / `.where(...).all` without preceding `.eager(` or `.eager_graph(`, iterated touching an association. Imports: `require 'sequel'`.
- ([Sequel plugins](https://sequel.jeremyevans.net/plugins.html), [Sequel associations](https://sequel.jeremyevans.net/rdoc/files/doc/association_basics_rdoc.html))

### 7.2 ROM-rb
- **Primitive**: `user_repo.all.combine(:tasks).combine_parents(:account)`, `task_repo.aggregate(:user)`. Combine = explicit eager. Lazy loading is **not supported by design** — there is no `.tasks` on a struct unless combined.
- **Tree-sitter shape**: any `.aggregate(` / `.combine(` indicates correct path. The anti-pattern in ROM is rarer because the type system pushes you toward combine. Detect: per-call `repo.find(...)` inside `each` block. Imports: `require 'rom'`, `require 'rom-sql'`.

### 7.3 Hanami (model 1.x and 2.x)
- 2.x is ROM-backed. 1.x: repositories must define `find_with_examples` etc. manually — no auto-eager. Detection: any `repo.find(id)` in a loop body, where loop is over a parent collection.
- Imports: `require 'hanami/model'` (1.x), `Hanami::Repository`.

### 7.4 Mongoid
- **Primitive**: `Band.includes(:albums).each { |b| b.albums }` — single eager queue. Limitations: only direct relations; polymorphic `belongs_to` cannot be includes'd; works only on `Mongoid::Criteria`, not arrays.
- **Workaround**: `mongoid_includes` gem (`ElMassimo/mongoid_includes`, MIT) supports nested + polymorphic.
- **Bulk**: `Band.collection.insert_many([docs])` (Mongo driver passthrough). Mongoid `.create` / `.save` per-instance.
- **Tree-sitter shape**: `<Model>.where(...).each` or `.all.each` body touching `has_many`/`embeds_many`/`belongs_to` field without `.includes(`. Imports: `require 'mongoid'`.
- ([Mongoid optimization analysis](https://alexbevi.com/blog/2021/03/26/analysis-and-optimization-of-an-n-plus-1-scenario-in-mongoid/))

### 7.5 DataMapper (historical)
- `Model.all(:fields => [:id, :name])` had implicit lazy-load; `User.all.posts` triggered N+1. Most rules from ActiveRecord apply.

---

## 8. Scala

### 8.1 Slick
- No relation model — only joins. N+1 pattern: `posts.result` then `Await.result(db.run(users.filter(_.id === p.userId).result))` in a `map`. Fix: `posts.joinLeft(users).on(_.userId === _.id).result`.
- **Tree-sitter shape**: `Await.result(db.run(...))` or `db.run(...).map(... => db.run(...))` chains. Imports: `import slick.jdbc.PostgresProfile.api._`.

### 8.2 Quill
- Compile-time AST → SQL. There is no relations system either. Anti-pattern: a `quote { query[Post].filter(...) }` inside a `users.foreach` traversal — Quill happily compiles per-row. Fix: a single `quote { for (u <- query[User]; p <- query[Post] if p.userId == u.id) yield (u, p) }`.
- Imports: `import io.getquill._`.

### 8.3 Doobie
- Pure SQL + cats-effect. N+1 appears as `parents.traverse(p => sql"select * from child where parent_id=$p.id".query[Child].to[List].transact(xa))`. Fix: a single `IN (...)` query + group in Scala using `cats.implicits._`.
- **Tree-sitter shape**: `.traverse(` over a collection where lambda body contains `sql"..."` interp or `.query[`. Imports: `import doobie._`, `import doobie.implicits._`.

### 8.4 Skunk
- Postgres-native, FP, session-based. Same anti-pattern shape as Doobie (per-row `session.prepare(...)` inside `traverse`). Skunk has first-class **prepared statements** with cached plans — encourage `prepare` outside the loop. Imports: `import skunk._`.

### 8.5 ScalikeJDBC
- DSL + raw SQL. Anti-pattern: `members.map(m => sql"... WHERE id=${m.groupId}".map(Group(_)).single.apply())`. Fix: `joinAlias` query or `OneToManies` mapper. Imports: `import scalikejdbc._`.

---

## 9. Elixir (Ecto deep-dive)

- **Primitives**: `Repo.preload(parents, [:assoc])`, `Repo.preload(parents, assoc: [:nested])`, query-form `from p in Parent, preload: [:assoc]`. Joined-preload via `join: a in assoc(p, :assoc), preload: [assoc: a]` — single SQL.
- **Anti-pattern 1**: per-row `Repo.preload` inside `Enum.map` (each call hits DB once).
- **Anti-pattern 2**: `:in_parallel` only works **outside** transactions and with multiple preloads — it's silently ignored inside `Repo.transaction` / `Ecto.Multi`.
- **Multi**: `Ecto.Multi.new() |> Multi.insert(:user, changeset) |> Multi.run(:notify, fn _, %{user: u} -> ... end) |> Repo.transaction()` — single round-trip transactional pipeline. Anti-pattern: chained `Repo.insert / Repo.update` outside Multi inside loops.
- **Bulk**: `Repo.insert_all(Model, list, on_conflict: :nothing, conflict_target: :id)`, `Repo.update_all`, `Repo.delete_all`.
- **Tree-sitter shape**: `Repo.all(query)` or `Repo.get(...)` followed by `Enum.map` whose lambda calls `Repo.preload(` or `Repo.get(`. Imports: `alias MyApp.Repo`, `import Ecto.Query`.
- ([Ecto.Repo](https://hexdocs.pm/ecto/Ecto.Repo.html), [Preloader source](https://github.com/elixir-ecto/ecto/blob/master/lib/ecto/repo/preloader.ex))

---

## 10. Crystal

### 10.1 Granite (Amber framework)
- Per-association method generation: `Post.find(id).user`. Lazy — anti-pattern is iterating `Post.all` and calling `.user`. No first-class preload — community forks added `eager_loading_n` plugins. Imports: `require "granite"`.

### 10.2 Avram (Lucky framework)
- **First-party N+1 prevention**: config `Avram.settings.lazy_load_enabled = false` → raises if a relation is accessed without preload. (Equivalent of Eloquent strict / Sequel forbid_lazy_load.)
- **Primitive**: `UserQuery.new.preload_posts.results`. Generated `preload_<assoc>` per association.
- **Imports**: `require "avram"`. ([Avram readme](https://github.com/luckyframework/avram))

---

## 11. Dart / Flutter

### 11.1 Drift (formerly Moor) — yes, our project name collision
- No relations layer in the Active Record sense. You write SQL JOINs in `joinBuilder`/`select().join([...])`. N+1 appears as `final users = await db.select(db.users).get(); for (u in users) await (db.select(db.posts)..where(p => p.userId.equals(u.id))).get()`.
- Counter-pattern: single `select().join([leftOuterJoin(posts, posts.userId.equalsExp(users.id))])`.
- Imports: `import 'package:drift/drift.dart';`.

### 11.2 Floor (Room-style annotation processor)
- Relations require manual `@Query` joins; no annotation-level `@Relation` until recently. Anti-pattern: per-row `dao.findById(id)` in `forEach`. Imports: `package:floor/floor.dart`.

### 11.3 ObjectBox (Dart)
- **Primitives**: `query.eager(Customer_.orders)`, `ToMany`/`ToOne` proxies lazy by default. One-level eager only; deeper needs query filters.
- **Bulk**: `box.putMany([list])`, `box.removeMany`.
- **Tree-sitter shape**: `query.find()` then loop body access to a `ToOne`/`ToMany`-typed field. Counter: `.eager(`. Imports: `package:objectbox/objectbox.dart`.
- ([ObjectBox relations](https://docs.objectbox.io/relations))

---

## 12. Swift

### 12.1 Core Data
- **Faulting** is the lazy mechanism. N+1 appears as `let emps = try ctx.fetch(req); for e in emps { print(e.department.name) }`.
- **Fixes**: `fetchRequest.relationshipKeyPathsForPrefetching = ["department"]`, `fetchRequest.fetchBatchSize = 100`, predicate with `IN`.
- **Tree-sitter shape**: `NSFetchRequest` constructed without `relationshipKeyPathsForPrefetching` set, results then iterated accessing relationship. Imports: `import CoreData`.
- ([Apple CoreData Performance](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreData/Performance.html))

### 12.2 GRDB
- **Primitives**: `Author.including(all: Author.books)`, `including(required: Author.country)`, `including(optional: ...)`, `forKey("alias")` for aliasing, `annotated(with:)` for aggregates.
- Decoder pulls all data from one fetched row tree, no per-row queries.
- **Bulk**: `try dbQueue.write { db in for x in xs { try x.insert(db) } }` is batched in a single transaction.
- **Tree-sitter shape**: `Author.fetchAll(db)` not chained with `.including(`, then loop accesses an association. Imports: `import GRDB`. ([GRDB associations](https://github.com/groue/GRDB.swift/blob/master/Documentation/AssociationsBasics.md))

### 12.3 Vapor Fluent
- **Primitives**: `Planet.query(on: db).with(\.$star) { $0.with(\.$galaxy) }`, `.with(\.$pivot)` for M2M pivot, `.join(...)` for SQL joins, `.alsoDecode(_:)`.
- **Tree-sitter shape**: `Model.query(on: db).all()` without `.with(\.` preceding it; loop body accesses `model.$relation.value`. Imports: `import Fluent`, `import FluentKit`. ([Fluent relations](https://docs.vapor.codes/fluent/relations/))

### 12.4 SwiftData (iOS 17+)
- **Primitive**: `FetchDescriptor<Movie>(predicate:..., sortBy:...)`; set `descriptor.relationshipKeyPathsForPrefetching = [\.director, \.actors]` to batch.
- Lazy by default — accessing a non-prefetched relation triggers a fetch.
- **Tree-sitter shape**: `FetchDescriptor(` constructed and used in `modelContext.fetch(...)`, iterated, relation accessed, **and** descriptor has no assignment to `relationshipKeyPathsForPrefetching`. Imports: `import SwiftData`.
- ([SwiftData relationshipKeyPathsForPrefetching](https://developer.apple.com/documentation/swiftdata/fetchdescriptor/relationshipkeypathsforprefetching))

---

## 13. .NET (beyond EF Core)

### 13.1 NHibernate
- **Primitives**: HQL `LEFT JOIN FETCH`, Criteria `SetFetchMode("Children", FetchMode.Eager/Join/Subselect)`, `batch-size="N"` on `<class>` or `<set>`, global `default_batch_fetch_size`. The famous Ayende warning: forgetting to set batch-size on collections.
- **Fix mechanic**: `batch-size=25` collapses N selects to N/25.
- **Anti-pattern**: `cats.ToList().ForEach(c => Console.WriteLine(c.Owner.Name))` with no fetch mode set.
- **Tree-sitter shape**: `session.Query<T>()` / `session.CreateCriteria` chains without `.Fetch(x => x.<rel>)` (NH-LINQ) or `SetFetchMode`. Imports: `using NHibernate;`, `using NHibernate.Linq;`.
- ([NHibernate performance](https://nhibernate.info/doc/nhibernate-reference/performance.html))

### 13.2 Dapper
- **Primitive**: multi-map `connection.Query<Post, Author, Post>(sql, (p, a) => { p.Author = a; return p; }, splitOn: "AuthorId")`. `QueryMultiple` for grids.
- **Anti-pattern**: `posts.Select(p => conn.QueryFirst<Author>("SELECT * FROM Authors WHERE Id=@Id", new {p.AuthorId}))`.
- **Bulk**: Dapper has no first-party bulk insert; use `connection.Execute(sql, list)` (auto-batches) or Dapper.Plus (commercial) / SqlBulkCopy.
- **Tree-sitter shape**: `conn.Query<` / `conn.QueryFirst<` / `conn.Execute<` inside `for/foreach`. Counter: `splitOn:` literal or `QueryMultiple`. Imports: `using Dapper;`.
- ([Learn Dapper relationships](https://www.learndapper.com/relationships))

### 13.3 ServiceStack OrmLite
- **Primitive**: `[Reference]` attribute on parent property + `db.LoadSelect<Order>(x => x.Id == 1)` to populate child collections. `db.LoadSingleById<T>(id)` analog.
- **Anti-pattern**: `db.Select<Order>().Each(o => o.Items = db.Select<OrderItem>(...))`.
- **Tree-sitter shape**: `db.Select<` / `db.Single<` results iterated touching a `[Reference]`-marked property, without prior `db.LoadSelect<` / `db.LoadSingleById<`. Imports: `using ServiceStack.OrmLite;`.

### 13.4 LINQ to DB (linq2db)
- **Primitives**: `LoadWith(u => u.Accounts).ThenLoad(a => a.Transactions)`. Known issues: deep nesting >3 levels can silently emit multiple queries; cache invalidation around `LoadWith` was fixed in 3.x.
- **Tree-sitter shape**: `db.GetTable<>()` queries materialized to list with `.ToList()` / `.ToArrayAsync()` without `.LoadWith(` followed by relation access. Imports: `using LinqToDB;`.
- ([linq2db LoadWith #214](https://github.com/linq2db/linq2db/issues/214))

### 13.5 RepoDB
- Hybrid micro/full ORM. `connection.QueryAll<Person>()` then per-row queries = N+1. Bulk: `BulkInsert`, `BulkMerge` (real `SqlBulkCopy` underneath). Imports: `using RepoDb;`.

### 13.6 LINQ to SQL (legacy)
- `DataLoadOptions.LoadWith<Order>(o => o.OrderDetails)` is the equivalent of EF Include. Same shape rules as EF Core but receiver is `DataContext`.

---

## 14. Static-Detection Feasibility Matrix

| Pattern class | Tree-sitter friendly? | Notes |
|---|---|---|
| Import-set check (ORM presence) | YES | Cheap gate before any AST walk |
| Receiver-pattern call site (e.g., `Model.find(`) | YES | Needs alias tracking for `from x import Y as Z` (Python) and `import {Y} from` (TS) |
| Loop body shape (`for ... in <expr>: <method-call>`) | YES | Already what drift does for Django |
| Missing eager-load chain on a query | MOSTLY | Need to track receiver fluency: `.find().with(...).all()`. Tree-sitter handles `.x.y.z()` chains, but rebinding via `let q = Model.find(); q = q.where(...); q.all()` requires intraprocedural dataflow |
| Attribute access *on what relation* | LIMITED | Detecting that `user.posts` is a relation needs schema/annotation parsing (e.g., `@OneToMany` in Java, `relationship()` in SQLA). Drift can ship per-ORM allow-lists of *known relation suffixes* (`_set`, `s`, `Posts`) and per-ORM annotation lookup |
| Bulk-write detection (loop of `.save()`/`.insert()`) | YES | Mechanically same shape as N+1 |
| XML mapper anti-patterns (MyBatis, rbatis) | YES (with tree-sitter-xml) | Needs separate XML parser pass |
| Strict-mode presence (Eloquent, Avram, SQLA raiseload, Sequel `forbid_lazy_load`) | YES | Detect the *enabler* call in app bootstrap files; if absent in a project that uses the ORM, surface as low-severity finding |
| Cartesian explosion from `withGraphJoined` / `JOIN FETCH` on M-N | NO (needs schema graph) | Skip unless schema is in scope |

---

## 15. Recommended drift Rule-Pack Additions (concrete)

A spreadsheet's worth of import-set + signature pairs you can drop in:

| ORM | Import grep | Anti-pattern method | Counter signature |
|---|---|---|---|
| SQLModel | `from sqlmodel`, `from sqlalchemy.orm` | `session.exec(select(`+iter+`.<rel>` | `selectinload(`, `joinedload(`, `raiseload(` |
| Peewee | `from peewee` / `import peewee` | `.select()`+iter+`.<fk_attr>` | `prefetch(` |
| Pony | `from pony.orm` | `select(... for ... in <Entity>)`+iter+`.<rel>` | `.prefetch(` |
| Beanie | `from beanie` | `Model.find(`/`find_one(` w/o `fetch_links=True` | `fetch_links=True` |
| MongoEngine | `from mongoengine` | `.objects(`+iter+`.<ref>` | `.select_related(` / `.no_dereference(` |
| ormar | `import ormar` | `await Model.objects.all()`+iter+`.<rel>` | `.select_related(` / `.prefetch_related(` |
| Piccolo | `from piccolo` | `.run()`+iter+`.<fk>` | `.objects(Table.fk)` / `.prefetch(` / `all_related()` |
| PynamoDB | `from pynamodb.models` | `Model.get(`+inside-loop | `batch_get([` / `batch_write()` |
| Redis-OM | `from redis_om` | `Model.get(`+inside-loop | `pipeline()` |
| MyBatis | XML `<collection select=` / `@Many(fetchType=LAZY)` | repeated mapper method calls inside loop | `<collection><...> ofType=` w/ nested resultMap; `defaultLazyLoadingBatchSize` |
| Ebean | `io.ebean` | `DB.find(X.class).findList()` w/o `.fetch(` | `.fetch(`, `.fetchQuery(`, `setLazyLoadBatchSize(` |
| EclipseLink | `org.eclipse.persistence` / JPA | JPQL `from X` w/o `JOIN FETCH` | `@BatchFetch`, hint `eclipselink.join-fetch` |
| Querydsl | `com.querydsl.jpa` | `.leftJoin(...).fetchJoin().distinct()` on M-N | use `distinct(true)` with `passDistinctThrough=false` |
| Exposed (Kotlin) | `org.jetbrains.exposed.dao` | `<Entity>.all().forEach`+`.<rel>` | `.with(<Entity>::rel)`, `.load(` |
| Ktorm | `org.ktorm` | `.findAll()` w/o explicit join | `database.from(...).leftJoin(...).select()` |
| Komapper | `org.komapper.core.dsl` | query w/o `.include(...)` then `store[...].rel(store)` | `.include(`, `.includeAll()` |
| Knex | `import knex from 'knex'` | `knex(<str>)` inside loop | `whereIn(`, `batchInsert(` |
| Objection | `from 'objection'` | `.query()...await` w/o `.withGraph(F\|J)etched(` | `.withGraphFetched(`, `insertGraph(`, `upsertGraph(` |
| MikroORM | `@mikro-orm/core` | `em.find(...)` w/o `populate:` | `populate: [`, `em.populate(`, `loadStrategy:` |
| Bookshelf | `require('bookshelf')` | `.fetch{All}(` w/o `withRelated` | `withRelated:` literal |
| Waterline | `sails.models.` / `Model.find` | `.find(...)` w/o `.populate(` | `.populate(` |
| Kysely | `from 'kysely'` | `db.selectFrom(...)` inside `map`/`for` | `jsonObjectFrom(`, `jsonArrayFrom(` |
| Mongoose | `from 'mongoose'` | `.find().lean()` + per-row `findById` | `.populate(`, `bulkWrite(`, `insertMany(` |
| DataLoader | `from 'dataloader'` | absence in GraphQL resolver file | `new DataLoader(`, `loader.load(` |
| D1 | `D1Database` type | `prepare().run()` inside loop | `db.batch([` |
| SQLBoiler | `volatiletech/sqlboiler` | `Rel().One{,P}(` inside `range` | `qm.Load(` |
| Bun | `uptrace/bun` | `Model(&list).Scan(` w/o `.Relation(` | `.Relation(` |
| pgx | `jackc/pgx` | `conn.Query{Row,}(` inside loop | `pgx.Batch{}`, `SendBatch(` |
| Pop | `gobuffalo/pop` | `tx.All(&)` w/o `.Eager{,Preload}()` | `tx.Eager(`, `tx.EagerPreload(` |
| Diesel | `use diesel::` | per-row `<Child>.filter(<fk>.eq(parent.id)).load(` in `for` | `belonging_to(&parents)` + `grouped_by(&parents)` |
| SeaORM | `use sea_orm::` | `find_related(`/`find_linked(` inside `for` | `find_with_related(`, `load_many(` |
| sqlx | `use sqlx::` | macro inside loop | `WHERE … = ANY($1)` + single fetch |
| ormlite (Rust) | `use ormlite::` | per-row fetch | `.join(Self::organization())` |
| rbatis | `use rbatis::` | XML mapper repeated in loop | XML batch tags |
| Eloquent strict | `Illuminate\Database\Eloquent` | absence of `preventLazyLoading(` | presence of `preventLazyLoading(` |
| Doctrine | `Doctrine\ORM\QueryBuilder` | `->leftJoin(` w/o matching `->addSelect(` | `JOIN FETCH` (DQL), `addSelect(` |
| Cycle | `Cycle\ORM\Select` | iterate w/o `->load(` | `->load(`, `->with(` |
| RedBean | `RedBeanPHP\R` | `->own<X>List` inside `foreach` | `R::preload(` |
| Propel | gen'd `<X>Query` | `<X>Query::create()` w/o `joinWith` | `joinWith('A.B')` |
| Yii AR | `yii\db\ActiveRecord` | `find()->all()` w/o `with(` | `->with(`, `->joinWith(` |
| Sequel | `require 'sequel'` | `.all` w/o `.eager`/`.eager_graph` | `.eager(`, `.eager_graph(`, plugin `forbid_lazy_load` |
| ROM-rb | `require 'rom'` | per-row `repo.find` in `each` | `.combine(`, `.aggregate(` |
| Mongoid | `require 'mongoid'` | `.each`+access ref w/o `.includes` | `.includes(` |
| Hanami | `Hanami::Repository` | `repo.find(id)` in loop | repo-defined `find_with_x` aggregator |
| Slick | `slick.jdbc` | `db.run(...)` inside `map`/`foreach` | `joinLeft`/`join` query |
| Quill | `io.getquill` | `quote { query[Child] }` inside `foreach` | single comprehension w/ filter |
| Doobie | `import doobie` | `.traverse(` w/ `sql"..."` lambda | single `IN (...)` query |
| Skunk | `import skunk` | per-row `session.prepare`/`execute` in `traverse` | `prepare` once + `stream` |
| Ecto | `alias <App>.Repo` / `import Ecto.Query` | `Enum.map`+`Repo.preload(` or `Repo.get(` | `Repo.preload([:a, :b])`, `join: a in assoc(p, :a), preload: [a: a]` |
| Granite | `require "granite"` | `.all` + `.user` in iteration | (manual joins) |
| Avram | `require "avram"` | query w/o `.preload_<assoc>` | `.preload_<assoc>`, `lazy_load_enabled = false` |
| Drift (Dart) | `package:drift/drift.dart` | `select().get()` + per-row `select()` in `for` | `select().join([leftOuterJoin(`)] |
| Floor | `package:floor/floor.dart` | per-row `dao.findById` in `forEach` | `@Query` with JOIN |
| ObjectBox (Dart) | `package:objectbox/objectbox.dart` | `query.find()` + `ToOne.target`/`ToMany` in loop | `query.eager(` |
| Core Data | `import CoreData` | `fetch(req)` + `.relationship` in `for` | `relationshipKeyPathsForPrefetching =` |
| GRDB | `import GRDB` | `.fetchAll(db)` w/o `.including(` | `.including(all:`, `.including(required:` |
| Vapor Fluent | `import Fluent` | `.query(on:).all()` w/o `.with(\.$` | `.with(\.$` |
| SwiftData | `import SwiftData` | `FetchDescriptor(` w/o `relationshipKeyPathsForPrefetching` | `relationshipKeyPathsForPrefetching =` |
| NHibernate | `using NHibernate` | LINQ `session.Query<T>()` w/o `.Fetch(` | `.Fetch(`, `SetFetchMode(`, `batch-size=` |
| Dapper | `using Dapper` | per-row `conn.Query<` in `foreach` | `splitOn:`, `QueryMultiple` |
| OrmLite (SS) | `using ServiceStack.OrmLite` | `db.Select<>()` iterate + per-row `db.Select<>` | `LoadSelect<`, `[Reference]` |
| linq2db | `using LinqToDB` | `db.GetTable<>().ToList()` w/o `.LoadWith(` | `.LoadWith(`, `.ThenLoad(` |
| RepoDB | `using RepoDb` | `QueryAll<>()` + per-row queries | `BulkInsert(`, `BulkMerge(` |

---

## 16. OSS detector tools by language (licenses)

- **Python**: `kolypto/py-nplus1loader` (MIT) — runtime SQLA. No mature static linter for SQLA/SQLModel; this is drift's gap.
- **Java/Kotlin**: `yannbriancon/spring-hibernate-query-utils` (MIT), `vladmihalcea/hypersistence-utils` (Apache-2.0). Both runtime-assertion-style.
- **Node**: GraphQL DataLoader (`graphql/dataloader`, BSD-3-Clause) is the canonical fix-layer to recognize, not a detector. No widely-used static N+1 detector for Knex/Objection/MikroORM — drift's opportunity.
- **Go**: nothing canonical. `bombsimon/sqlcommenter`-style observability libs are not detectors.
- **Rust**: nothing.
- **PHP**: `beyondcode/laravel-query-detector` (MIT, runtime). Doctrine has the official query logger; no static linter.
- **Ruby**: `bullet` gem (MIT) — runtime ActiveRecord N+1 detector, the de facto reference; for Sequel use `forbid_lazy_load` plugin (Sequel's own, MIT).
- **Scala**: nothing.
- **Elixir**: `surgeventures/lemon_meringue` and similar test helpers; Telemetry-based observability dominates.
- **Swift**: nothing.
- **.NET**: `EFCore.BulkExtensions` (commercial), `Z.EntityFramework.Plus` (commercial), `MiniProfiler` (MIT, runtime).

---

## Sources

- [SQLAlchemy 2.x loading techniques](https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html)
- [SQLModel Read Relationships](https://sqlmodel.tiangolo.com/tutorial/relationship-attributes/read-relationships/)
- [Peewee querying](https://docs.peewee-orm.com/en/latest/peewee/querying.html), [Peewee relationships](https://docs.peewee-orm.com/en/latest/peewee/relationships.html)
- [Pony ORM API](https://docs.ponyorm.org/api_reference.html), [Pony prefetch issue #172](https://github.com/ponyorm/pony/issues/172)
- [Beanie relations](https://github.com/BeanieODM/beanie/blob/main/docs/tutorial/relations.md), [Beanie query API](https://beanie-odm.dev/api-documentation/query/)
- [MongoEngine querying](https://mongoengine-odm.readthedocs.io/guide/querying.html)
- [ormar queries](https://collerek.github.io/ormar/latest/queries/read/), [prefetch](https://collerek.github.io/ormar/latest/api/queryset/queries/prefetch_query/)
- [Piccolo objects](https://piccolo-orm.readthedocs.io/en/latest/piccolo/query_types/objects.html)
- [PynamoDB batch](https://pynamodb.readthedocs.io/en/stable/batch.html)
- [Redis-OM Python models](https://github.com/redis/redis-om-python/blob/main/docs/models.md)
- [MyBatis XML mappers](https://mybatis.org/mybatis-3/sqlmap-xml.html), [N+1 article](https://www.pingcap.com/article/how-to-efficiently-solve-the-n1-query-problem/)
- [Ebean N+1](https://ebean.io/docs/query/background/nplus1), [Ebean fetch](https://ebean.io/docs/query/fetch)
- [EclipseLink @BatchFetch](https://eclipse.dev/eclipselink/documentation/2.5/jpa/extensions/a_batchfetch.htm)
- [OpenJPA fetch groups](https://openjpa.apache.org/builds/1.2.3/apache-openjpa/docs/ref_guide_fetch.html)
- [Querydsl + fetchJoin](https://madplay.github.io/post/avoid-n+1-problem-in-jpa-using-querydsl-fetchjoin)
- [Exposed guide](https://www.jetbrains.com/help/exposed/about.html), [Exposed vs Hibernate](https://softwaremill.com/hibernate-vs-exposed-choosing-kotlins-best-persistence-tool-round-3/)
- [Ktorm joining](https://www.ktorm.org/en/joining.html)
- [Komapper association API](https://www.komapper.org/docs/reference/association/)
- [Spring R2DBC docs](https://docs.spring.io/spring-framework/reference/data-access/r2dbc.html)
- [Hibernate Reactive 1.1 ref](https://hibernate.org/reactive/documentation/1.1/reference/html_single/)
- [Speedment join ops](https://speedment.github.io/speedment-doc/join.html)
- [Knex query builder](https://knexjs.org/guide/query-builder.html)
- [Objection eager methods](https://vincit.github.io/objection.js/api/query-builder/eager-methods.html)
- [MikroORM Identity Map](https://mikro-orm.io/docs/identity-map), [MikroORM 5 changes](https://dev.to/b4nan/mikroorm-5-stricter-safer-smarter-l4g)
- [Bookshelf API](https://bookshelfjs.org/api.html)
- [Waterline populate](https://sailsjs.com/documentation/reference/waterline-orm/queries/populate)
- [Kysely relations recipe](https://kysely.dev/docs/recipes/relations)
- [Mongoose populate](https://mongoosejs.com/docs/populate.html), [Mongoose lean](https://mongoosejs.com/docs/tutorials/lean.html)
- [DataLoader repo](https://github.com/graphql/dataloader)
- [Cloudflare D1 binding API](https://developers.cloudflare.com/d1/worker-api/d1-database/)
- [SQLBoiler eager_load.go](https://github.com/volatiletech/sqlboiler/blob/master/queries/eager_load.go)
- [Bun relations](https://bun.uptrace.dev/guide/relations.html)
- [pgx package](https://pkg.go.dev/github.com/jackc/pgx/v5)
- [Pop N+1 PR #146](https://github.com/gobuffalo/pop/pull/146), [Buffalo relations](https://gobuffalo.io/documentation/database/relations/)
- [Diesel relations](https://diesel.rs/guides/relations.html)
- [SeaORM data loader](https://www.sea-ql.org/SeaORM/docs/relation/data-loader/), [Entity loader](https://www.sea-ql.org/SeaORM/docs/relation/entity-loader/)
- [ormlite (Rust)](https://docs.rs/ormlite)
- [rbatis](https://github.com/rbatis/rbatis)
- [Laravel News on preventLazyLoading](https://laravel-news.com/disable-eloquent-lazy-loading-during-development)
- [Doctrine performance traps](https://tideways.com/profiler/blog/5-doctrine-orm-performance-traps-you-should-avoid)
- [Cycle eager loading](https://spiral.dev/docs/orm-loading/1.0/en)
- [RedBean eager loading](http://www.redbeanphp.com/manual3_0/eager_loading)
- [Propel relationships](https://propelorm.org/documentation/04-relationships.html)
- [Yii AR](https://www.yiiframework.com/doc/guide/2.0/en/db-active-record)
- [Sequel plugins](https://sequel.jeremyevans.net/plugins.html), [Sequel associations](https://sequel.jeremyevans.net/rdoc/files/doc/association_basics_rdoc.html)
- [Mongoid optimization](https://alexbevi.com/blog/2021/03/26/analysis-and-optimization-of-an-n-plus-1-scenario-in-mongoid/)
- [Avram (Crystal/Lucky)](https://github.com/luckyframework/avram)
- [Ecto.Repo](https://hexdocs.pm/ecto/Ecto.Repo.html), [Ecto preloader source](https://github.com/elixir-ecto/ecto/blob/master/lib/ecto/repo/preloader.ex)
- [ObjectBox relations](https://docs.objectbox.io/relations)
- [Apple Core Data Performance](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreData/Performance.html)
- [GRDB associations](https://github.com/groue/GRDB.swift/blob/master/Documentation/AssociationsBasics.md)
- [Vapor Fluent relations](https://docs.vapor.codes/fluent/relations/)
- [SwiftData relationshipKeyPathsForPrefetching](https://developer.apple.com/documentation/swiftdata/fetchdescriptor/relationshipkeypathsforprefetching)
- [NHibernate performance](https://nhibernate.info/doc/nhibernate-reference/performance.html)
- [Learn Dapper relationships](https://www.learndapper.com/relationships)
- [linq2db LoadWith #214](https://github.com/linq2db/linq2db/issues/214)
- [Spring Hibernate Query Utils](https://github.com/yannbriancon/spring-hibernate-query-utils) (MIT)
- [py-nplus1loader](https://github.com/kolypto/py-nplus1loader) (MIT)agentId: a47139721d5250dd4 (use SendMessage with to: 'a47139721d5250dd4' to continue this agent)
<usage>total_tokens: 136384
tool_uses: 70
duration_ms: 735237</usage>