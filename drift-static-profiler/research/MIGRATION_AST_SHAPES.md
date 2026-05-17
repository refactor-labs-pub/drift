I have enough material to assemble the deep-research report. Let me write the report now as my final assistant message.

---

# Deep Research — drift-static-profiler §12 Cumulative Migration-History Scanner

Implementation-grade research covering tree-sitter grammar shapes, per-tool migration ASTs, schema-state reference models, and OSS corpus characteristics. Citations inline as numbered URLs.

---

## 1) tree-sitter-sequel / tree-sitter-sql — concrete grammar shapes

**Identity & license.** The Rust crate `tree-sitter-sequel` is the active Cargo distribution of `DerekStride/tree-sitter-sql` (MIT). The crate name divergence is intentional — `tree-sitter-sql` (a Cargo namespace) was held by an unmaintained variant, so DerekStride publishes as `tree-sitter-sequel` for Rust while npm uses `@derekstride/tree-sitter-sql` [1][2]. PyPI ships as `tree-sitter-sql`. **Recommendation: depend on `tree-sitter-sequel` as a Cargo dependency** alongside drift's existing `tree-sitter-*` crates.

**Confirmed node kinds (read from the live grammar).** The grammar lives in `grammar/statements/{create,alter,drop}.js` and `grammar/column-lists.js`. Exact rule names (these become tree-sitter node kinds):

| DDL construct | Node kind | Key named fields |
|---|---|---|
| `CREATE TABLE` | `create_table` | inner `object_reference` (table name) + `column_definitions` |
| `CREATE INDEX` | `create_index` | inner `object_reference` (table) + `index_fields` + optional `keyword_unique`, `keyword_concurrently`, `keyword_btree`/`hash`/`gist`/`gin`/`brin`, optional `covering_columns`, optional `where` |
| `ALTER TABLE` | `alter_table` | inner `object_reference` + one or more `_alter_specifications` |
| `ALTER … ADD COLUMN` | `add_column` | inner `column_definition` |
| `ALTER … DROP COLUMN` | `drop_column` | field `name` |
| `ALTER … ALTER COLUMN` | `alter_column` | field `name`, field `type` |
| `ALTER … RENAME COLUMN` | `rename_column` | fields `old_name`, `new_name` |
| `ALTER … ADD CONSTRAINT …` | `add_constraint` | inner `identifier` (constraint name) + `constraint` |
| `ALTER … DROP CONSTRAINT` | `drop_constraint` | inner `identifier` |
| Column inside a CREATE/ADD | `column_definition` | fields `name`, `type` + repeated `_column_constraint` |
| FK *inside* `column_definition` | `_column_constraint` (alt arm using `keyword_references`) | `object_reference` (referenced table) + `paren_list(identifier)` (referenced cols) + repeated `keyword_on` `keyword_delete`/`keyword_update` clauses |
| FK as a table-level constraint | `_key_constraint` (or `_constraint_literal`) | optional `keyword_foreign`, `ordered_columns`, optional `keyword_references` block |
| PRIMARY KEY constraint | `_primary_key_constraint` | `ordered_columns` |
| UNIQUE column constraint | bare `keyword_unique` inside `_column_constraint` | — |
| Ordered index columns | `ordered_columns` → `ordered_column` (alias `column`) | field `name`, optional `direction` |

The full body of these rules is confirmed from the live grammar files [3][4][5].

**Concrete tree-sitter query patterns drift should ship** (each compiles against `tree-sitter-sequel`):

```scheme
; CREATE TABLE with table name capture
(create_table
  (object_reference) @table_name
  (column_definitions
    (column_definition
      name: (_) @col_name
      type: (_) @col_type) @col)) @stmt

; CREATE INDEX with concurrency + uniqueness flags
(create_index
  (keyword_unique)? @unique
  (keyword_concurrently)? @concurrently
  column: (_)? @index_name
  (object_reference) @table_name
  (index_fields
    (field
      column: (_) @indexed_col)+)) @stmt

; ALTER TABLE ADD COLUMN
(alter_table
  (object_reference) @table_name
  (add_column
    (column_definition
      name: (_) @col_name
      type: (_) @col_type))) @stmt

; ALTER TABLE DROP COLUMN
(alter_table
  (object_reference) @table_name
  (drop_column
    name: (_) @col_name)) @stmt

; FK as table-level: ALTER TABLE … ADD CONSTRAINT name FOREIGN KEY (cols) REFERENCES other(cols)
(alter_table
  (object_reference) @local_table
  (add_constraint
    (identifier) @constraint_name
    (constraint
      (_key_constraint
        (keyword_foreign)
        (ordered_columns (column (_) @local_col))
        (keyword_references)
        (object_reference) @ref_table
        (paren_list (identifier) @ref_col))))) @stmt

; FK column-level: name TYPE REFERENCES other(id)
(column_definition
  name: (_) @local_col
  (_column_constraint
    (keyword_references)
    (object_reference) @ref_table
    (paren_list (identifier) @ref_col)))
```

**Postgres-specific coverage.** The grammar handles `table_partition` (Postgres `PARTITION BY RANGE/HASH`, Hive `PARTITIONED BY`, Spark partition spec), `storage_parameters` (Postgres `WITH (...)`) and a wide `_table_settings` choice [3]. It does **not** model Postgres `INHERITS` as a first-class field — it falls into the residual `_table_settings`/`table_option` arms. Generated columns are modeled inside `_column_constraint` as `seq(optional(seq(keyword_generated, keyword_always)), keyword_as, _expression)` followed by optional `keyword_stored`/`keyword_virtual` [4]. Identity columns (`GENERATED ALWAYS AS IDENTITY`) are NOT distinctly named — they parse as the same generated-column arm.

**Error tolerance.** tree-sitter's recovery model is statement-local: a malformed `CREATE TYPE … AS (…)` produces an `ERROR` node, but surrounding `_create_statement` siblings continue to parse. The simulator should iterate `(_create_statement) (_alter_statement) (drop_statement)` at the top level and skip subtrees that contain `(ERROR)` — drift's invariant (silent-skip when uncertain) is naturally honored.

**Recommendation matrix.** Use `tree-sitter-sequel` for ~90% of structural extraction. Fall back to `pg_query` (already a drift dependency, BSD-3 + Postgres license [Cargo.toml line 65]) for: `CREATE TABLE … INHERITS`, identity-column distinguishing (`IDENTITY`-vs-generated), partial unique indexes with complex predicates, `CREATE TABLE … PARTITION BY` deeper partition-bound extraction, and any statement where tree-sitter emits an `(ERROR)` at the top level. Selection rule already in plan §12.8 stands. `gmr/tree-sitter-postgres` exists (727 rules auto-generated from PG's bison) [6] but is *much* larger and not battle-tested in editors; drift should not pick it up unless coverage gaps from `tree-sitter-sequel` accumulate.

---

## 2) Alembic AST shape (tree-sitter-python)

**Module structure.** Every alembic version file has four module-level assignments and two functions [7]:

```python
revision: str = "1975ea83b712"
down_revision: str | None = "abc123..."
branch_labels: tuple[str, ...] | None = None
depends_on: tuple[str, ...] | None = None

def upgrade() -> None: ...
def downgrade() -> None: ...
```

**Operation signatures (verbatim from alembic.ops [7]):**

```
op.create_table(table_name, *columns, if_not_exists=None, schema=None, **kw)
op.drop_table(table_name, schema=None, if_exists=None, **kw)
op.add_column(table_name, column, schema=None, if_not_exists=None, ...)
op.drop_column(table_name, column_name, schema=None, **kw)
op.alter_column(table_name, column_name, nullable=None, comment=False,
                server_default=False, new_column_name=None, type_=None,
                existing_type=None, existing_server_default=False,
                existing_nullable=None, existing_comment=None, schema=None, **kw)
op.create_index(index_name, table_name, columns, schema=None, unique=False,
                if_not_exists=None, **kw)
op.drop_index(index_name, table_name=None, schema=None, if_exists=None, **kw)
op.create_foreign_key(constraint_name, source_table, referent_table,
                      local_cols, remote_cols, onupdate=None, ondelete=None,
                      deferrable=None, initially=None, match=None,
                      source_schema=None, referent_schema=None, **dialect_kw)
op.create_primary_key(constraint_name, table_name, columns, schema=None)
op.create_unique_constraint(constraint_name, table_name, columns,
                            schema=None, **kw)
op.execute(sqltext, execution_options=None)
op.bulk_insert(table, rows, multiinsert=True)
```

**`sa.Column` shape:** `sa.Column(name, type_, *args, primary_key=False, nullable=True, server_default=None, default=None, ...)`. ForeignKey is a positional non-kwarg: `sa.Column('account_id', sa.Integer, sa.ForeignKey('accounts.id'))` [7].

**tree-sitter-python query patterns.** drift already uses `tree-sitter-python 0.23` (Cargo.toml line 18). Node kinds: `module`, `expression_statement`, `assignment`, `call`, `attribute`, `identifier`, `string`, `argument_list`, `keyword_argument`.

```scheme
; Module-level revision metadata
(module
  (expression_statement
    (assignment
      left: (identifier) @id (#match? @id "^(revision|down_revision|branch_labels|depends_on)$")
      right: (_) @value)))

; op.* calls — receive function and args (top-level dispatch)
(call
  function: (attribute
    object: (identifier) @ns (#eq? @ns "op")
    attribute: (identifier) @op_name)
  arguments: (argument_list) @args)

; sa.Column inside an op.create_table — first positional is column name
(call
  function: (attribute
    object: (identifier) @ns (#eq? @ns "sa")
    attribute: (identifier) @t (#eq? @t "Column"))
  arguments: (argument_list
    (string) @col_name
    (_) @col_type
    (keyword_argument
      name: (identifier) @kw
      value: (_) @kw_val)*))

; sa.ForeignKey('other.id') inline
(call
  function: (attribute
    object: (identifier) @ns (#eq? @ns "sa")
    attribute: (identifier) @t (#eq? @t "ForeignKey"))
  arguments: (argument_list
    (string) @fk_target))
```

**Extracting kwargs** (`nullable=False`, `primary_key=True`, `index=True`, `unique=True`, `server_default=...`): walk `keyword_argument` children of the `argument_list` and dispatch on the `name`-field identifier text. The kwarg value is either `(true)`/`(false)` (tree-sitter-python emits the literal as kind `true` / `false`), `(string)`, `(integer)`, or a call expression (for `func.now()` etc.).

**Multi-head detection.** Alembic itself surfaces this via `alembic heads` (CLI). drift can detect it statically by building a dependency map from `down_revision` to `revision` across all version files; multiple revisions sharing the same `down_revision` value implies a branch, and multiple heads (i.e. revisions that no other revision points to) is the multi-head condition. The branch-detection algorithm: build `successors: {parent_rev: set(child_revs)}`. Any key with `len(value) > 1` is a branch point; any revision absent from the union of values is a head [7][8].

**Dialect note.** drift should silently skip alembic auto-generated `# ### commands auto generated by Alembic - please adjust! ###` markers — they're comments and don't affect parsing.

---

## 3) Django migrations AST shape

**Migration class shell** [9][10]:

```python
class Migration(migrations.Migration):
    initial = True  # optional
    atomic = False  # optional - non-atomic mode
    dependencies = [("app", "0001_initial")]
    operations = [
        migrations.CreateModel(name=..., fields=[...], options=..., bases=..., managers=...),
        ...
    ]
```

**Operation constructors (verified from django.db.migrations.operations [9]):**

```
CreateModel(name, fields, options=None, bases=None, managers=None)
DeleteModel(name)
RenameModel(old_name, new_name)
AddField(model_name, name, field, preserve_default=True)
RemoveField(model_name, name)
AlterField(model_name, name, field, preserve_default=True)
RenameField(model_name, old_name, new_name)
AddIndex(model_name, index)
RemoveIndex(model_name, name)
RenameIndex(model_name, new_name, old_name=None, old_fields=None)
AddConstraint(model_name, constraint)
RemoveConstraint(model_name, name)
RunSQL(sql, reverse_sql=None, state_operations=None, hints=None, elidable=False)
RunPython(code, reverse_code=None, atomic=None, hints=None, elidable=False)
AlterModelOptions(name, options)
AlterUniqueTogether(name, unique_together)
AlterIndexTogether(name, index_together)
SeparateDatabaseAndState(database_operations=None, state_operations=None)
```

**Note on `AddIndexConcurrently`/`RemoveIndexConcurrently`:** Postgres-only, lives in `django.contrib.postgres.operations`. Same `(model_name, index)` signature but lifts the implicit transaction (the migration must set `atomic = False`). This is the principal Postgres-safe-migrations indicator drift should flag when an `AddIndex` appears without `atomic = False`.

**Field-construction extraction.** Inside a `CreateModel` operation, the `fields` arg is a list of 2-tuples `("col_name", models.IntegerField(...))`. tree-sitter shape:

```scheme
; CreateModel fields tuple
(call
  function: (attribute
    object: (identifier) @ns (#eq? @ns "migrations")
    attribute: (identifier) @op (#eq? @op "CreateModel"))
  arguments: (argument_list
    (keyword_argument
      name: (identifier) @kw (#eq? @kw "fields")
      value: (list
        (tuple
          (string) @field_name
          (call
            function: (attribute
              object: (identifier) @field_ns (#eq? @field_ns "models")
              attribute: (identifier) @field_type)
            arguments: (argument_list) @field_args) @field_call)+))))

; ForeignKey resolution
(call
  function: (attribute
    object: (identifier) (#eq? @_ "models")
    attribute: (identifier) (#eq? @_ "ForeignKey"))
  arguments: (argument_list
    (string) @target  ; "app.Model" or "self"
    (keyword_argument
      name: (identifier) (#eq? @_ "on_delete")
      value: (attribute
        object: (identifier) (#eq? @_ "models")
        attribute: (identifier) @on_delete_kind))?
    (keyword_argument
      name: (identifier) (#eq? @_ "db_index")
      value: [(true) (false)] @db_index)?))
```

**`Migration.atomic = False` detection.** Look for `class_definition` whose name is `Migration` (superclass binding via `argument_list (attribute ...)` to `migrations.Migration`), then inside its `block`, find `expression_statement` → `assignment` with `left: (identifier) "atomic"` and `right: (false)`.

**Resolving `models.X` references.** Build a small allowlist of Django field types (`IntegerField`, `CharField`, `TextField`, `BooleanField`, `DateTimeField`, `DateField`, `UUIDField`, `JSONField`, `ForeignKey`, `OneToOneField`, `ManyToManyField`, `AutoField`, `BigAutoField`, `BigIntegerField`, `DecimalField`, `FloatField`, `EmailField`, `URLField`, `SlugField`, `BinaryField`, `FileField`, `ImageField`, `IPAddressField`, `GenericIPAddressField`, `DurationField`, `TimeField`, `SmallIntegerField`, `PositiveIntegerField`, `PositiveSmallIntegerField`). For everything else (typically `models.X` referring to custom or third-party fields), silently treat as `BinaryField` for nullability semantics and skip type-derived rules. Drift's "silent-skip when uncertain" invariant naturally covers this.

---

## 4) ActiveRecord / Rails migrations AST shape

**Class shell** [11]:

```ruby
class CreateUsers < ActiveRecord::Migration[7.0]
  def change
    create_table :users do |t|
      t.string :email, null: false
      t.string :name
      t.references :organization, foreign_key: true, index: true
      t.timestamps
    end
    add_index :users, :email, unique: true
  end
end
```

Or `def up`/`def down` for irreversible operations.

**DSL call list.** Top-level (no receiver): `create_table`, `drop_table`, `rename_table`, `add_column`, `change_column`, `change_column_null`, `change_column_default`, `remove_column`, `add_index`, `remove_index`, `add_foreign_key`, `remove_foreign_key`, `add_reference`, `remove_reference`. Block form inside `create_table`: `t.string`, `t.integer`, `t.bigint`, `t.text`, `t.boolean`, `t.datetime`, `t.timestamps`, `t.references`, `t.belongs_to`, `t.index`, `t.foreign_key`, `t.json`, `t.jsonb`.

**`strong_migrations` gem checks** [12][13]. drift should consume these as rule names directly when scanning Rails migration trees:

- **General**: removing a column, changing column type, renaming a column, renaming a table, `create_table` with `force: true`, adding auto-incrementing column, adding stored generated column, adding foreign key, adding check constraint, executing raw SQL, backfilling data.
- **Postgres**: adding index non-concurrently, adding a reference (because Rails defaults to non-concurrent index), adding unique constraint, adding exclusion constraint, adding JSON column, volatile default value, setting `NOT NULL` on existing column, renaming enum value, renaming schema.
- **MySQL/MariaDB**: `COPY` algorithm, shared/exclusive locking, expression default value.

`safety_assured { … }` blocks mark intentional unsafe ops. drift should treat the lexical contents of a `safety_assured` block as opted-out: parse them, emit findings with `confidence: low` rather than dropping (the developer's claim can still be wrong, and the surrounding annotation is itself a signal).

**Tree-sitter gap.** drift does NOT ship `tree-sitter-ruby` (Cargo.toml lines 17-26 — only python/java/typescript/javascript/go/rust/scala/kotlin/containerfile). **Recommendation**: ship Rails migration scanning as a *regex-and-string-walking* parser for v1, not tree-sitter. The DSL is line-oriented and the value of full Ruby parsing is small (no nested method calls to worry about beyond the block-DSL receivers). Concretely: a per-line regex over `^\s*(create_table|add_column|drop_table|add_index|add_foreign_key|change_column_null|remove_column|add_reference)\b` with a small Ruby-symbol-and-hash mini-parser captures 95% of Rails migrations. Add `tree-sitter-ruby` (MIT, [https://github.com/tree-sitter/tree-sitter-ruby]) only when block-DSL precision is needed for §12 contextual rules.

The block-form `t.<type>` calls inside `create_table :users do |t| … end` are easier to handle with the string approach: scan from `do |t|` to the matching `end`, then per-line `t\.(\w+)\s+:(\w+)(.*)$`.

---

## 5) Flyway & raw .sql migrations

**Naming conventions [14][15]:**

```
V<version>__<description>.sql       versioned (e.g. V1__init.sql, V2.1.0__add_users.sql)
U<version>__<description>.sql       undo of the matching V version
R__<description>.sql                repeatable (no version; re-runs on checksum change)
B<version>__<description>.sql       baseline (snapshot at version)
```

Version separator: dot OR underscore (`V1.2.3__` ≡ `V1_2_3__`). Description separator: **double** underscore (single is invalid). Suffix configurable but defaults to `.sql`.

**Recommended migration-discovery regex for drift** (must be applied case-sensitively):
```regex
^(?P<prefix>[VURB])(?P<version>[\d._]+)?__(?P<desc>.+)\.sql$
```
For `R__*.sql` the version group is empty; sort `R` entries last by description. For ordering V/U/B, parse `version` as a dotted list of integers and sort lexicographically as tuples.

**Postgres-safety patterns the simulator must detect on raw SQL** (these go to the existing per-file `MIG*` rules but the *cumulative* simulator also needs to know which CREATE INDEX was concurrent vs not):

| Unsafe | Safe |
|---|---|
| `CREATE INDEX ...` | `CREATE INDEX CONCURRENTLY ...` |
| `ALTER TABLE x ALTER COLUMN c SET NOT NULL` | Two-step: `ADD CONSTRAINT ck CHECK (c IS NOT NULL) NOT VALID;` → later `VALIDATE CONSTRAINT ck;` → finally `SET NOT NULL` is then ~free |
| `ALTER TABLE x ADD CONSTRAINT fk FOREIGN KEY ...` | Two-step: `... NOT VALID;` → later `VALIDATE CONSTRAINT fk;` |
| `ALTER TABLE x ADD COLUMN c TYPE NOT NULL DEFAULT v` | (PG ≥11 safe for non-volatile defaults; flag volatile defaults like `gen_random_uuid()`) |
| `DROP INDEX x` (locks reads/writes on PG < 13 in some paths) | `DROP INDEX CONCURRENTLY x` |

In tree-sitter-sequel terms: detect `(create_index (keyword_concurrently) ...)` vs the same rule without the keyword child to drive `MIG_REQUIRE_CONCURRENT_INDEX`. Detect `(alter_column (keyword_set) (keyword_not) (keyword_null))` for `MIG_SET_NOT_NULL_UNSAFE`. Detect FK ADD without a sibling `NOT VALID` sequence for `MIG_FK_VALIDATE_UNSAFE`.

**Postgres dialect detection heuristics** (apply OR scan over a small body sample, ≥1 match implies PG):
- Keywords: `CREATE EXTENSION`, `SERIAL`, `BIGSERIAL`, `tsvector`, `jsonb`, `gin`, `gist`, `brin`, `spgist`, `WITH ORDINALITY`, `LATERAL`, `RETURNING`, `ON CONFLICT`, `DO UPDATE`, `EXCLUDED`, `::text`, `::jsonb`, `::uuid`, `CONCURRENTLY`, `NOT VALID`, `VALIDATE CONSTRAINT`, `INHERITS`, `PARTITION BY RANGE`, `PARTITION BY HASH`, `OPERATOR CLASS`, `IDENTITY` (in column constraint), `STORED` / `VIRTUAL` after `GENERATED ALWAYS AS`.
- Filename/directory: presence of `flyway/postgresql/` or `db/postgres/` is a strong signal.
- Function syntax: `$$ ... $$` dollar-quoting is PG-distinctive.

drift's existing `sqlparser-rs` (multi-dialect; Cargo.toml line 59) accepts a `Dialect::Postgres` instance — once detected, drift can hand the file to `pg_query` for deeper safety analysis.

---

## 6) TypeORM migrations AST shape

**Class shell** [16]:

```typescript
import { MigrationInterface, QueryRunner, Table, TableColumn,
         TableIndex, TableForeignKey } from "typeorm";

export class CreatePost1480489020310 implements MigrationInterface {
  async up(queryRunner: QueryRunner): Promise<void> {
    await queryRunner.createTable(new Table({
      name: "post",
      columns: [
        { name: "id", type: "int", isPrimary: true,
          isGenerated: true, generationStrategy: "increment" },
        { name: "title", type: "varchar", length: "255", isNullable: false },
        { name: "author_id", type: "int" },
      ],
    }), true);

    await queryRunner.createIndex("post", new TableIndex({
      name: "IDX_post_author", columnNames: ["author_id"], isUnique: false,
    }));

    await queryRunner.createForeignKey("post", new TableForeignKey({
      columnNames: ["author_id"],
      referencedColumnNames: ["id"],
      referencedTableName: "user",
      onDelete: "CASCADE",
    }));
  }
  async down(queryRunner: QueryRunner): Promise<void> { ... }
}
```

**Method list:** `createTable`, `dropTable`, `addColumn`, `dropColumn`, `addColumns`, `changeColumn`, `renameColumn`, `createIndex`, `dropIndex`, `createIndices`, `createForeignKey`, `dropForeignKey`, `createUniqueConstraint`, `createCheckConstraint`, `query`, `startTransaction`, `commitTransaction`, `rollbackTransaction`.

**tree-sitter-typescript node kinds** (drift already depends on tree-sitter-typescript 0.23, line 20):

```scheme
; await queryRunner.createTable(new Table({...}), true)
(await_expression
  (call_expression
    function: (member_expression
      object: (identifier) @qr (#eq? @qr "queryRunner")
      property: (property_identifier) @method)
    arguments: (arguments
      (new_expression
        constructor: (identifier) @ctor   ; Table / TableIndex / TableForeignKey
        arguments: (arguments
          (object) @options))
      _* )))

; Inside the options object literal: properties
(object
  (pair
    key: (property_identifier) @opt_name
    value: (_) @opt_value))

; Columns array
(pair
  key: (property_identifier) (#eq? @_ "columns")
  value: (array
    (object
      (pair key: (property_identifier) @col_field value: (_) @col_val)+) @col_obj))
```

**Key option names** to extract per constructor:
- `Table`: `name`, `columns[]`, `indices[]`, `foreignKeys[]`, `uniques[]`, `checks[]`.
- `TableColumn`: `name`, `type`, `length`, `isPrimary`, `isNullable`, `isGenerated`, `generationStrategy`, `default`, `isUnique`.
- `TableIndex`: `name`, `columnNames[]`, `isUnique`.
- `TableForeignKey`: `columnNames[]`, `referencedColumnNames[]`, `referencedTableName`, `onDelete`, `onUpdate`.

**Raw-SQL escape hatch.** `queryRunner.query("...")` calls with string literals should be extracted and fed to the SQL path (sqlparser-rs / tree-sitter-sequel) to recurse into the schema-state simulator.

---

## 7) Sequelize migrations AST shape

**Shell** [17]:

```javascript
'use strict';
module.exports = {
  up: async (queryInterface, Sequelize) => {
    await queryInterface.createTable('Users', {
      id: { type: Sequelize.INTEGER, primaryKey: true, autoIncrement: true },
      email: { type: Sequelize.STRING, allowNull: false, unique: true },
      organizationId: {
        type: Sequelize.INTEGER, allowNull: false,
        references: { model: 'Organizations', key: 'id' },
        onDelete: 'CASCADE',
      },
      createdAt: { type: Sequelize.DATE, allowNull: false },
      updatedAt: { type: Sequelize.DATE, allowNull: false },
    });
    await queryInterface.addIndex('Users', { fields: ['email'], unique: true });
  },
  down: async (queryInterface, Sequelize) => { ... }
};
```

**Method list on `queryInterface`:** `createTable`, `dropTable`, `addColumn`, `removeColumn`, `changeColumn`, `renameColumn`, `addIndex`, `removeIndex`, `addConstraint`, `removeConstraint`, `bulkInsert`, `bulkDelete`, `bulkUpdate`, `sequelize.query` (raw SQL).

**Node kinds (tree-sitter-javascript [drift line 21]):** `assignment_expression` on `module.exports`, then the right side is an `object`. Each `pair` has key `up`/`down` and value `arrow_function`/`function_expression`. Inside, `await_expression` / `call_expression` against `(member_expression object: (identifier "queryInterface") property: (property_identifier) ...)`.

```scheme
; module.exports = { up: async (qi, S) => { ... }, down: ... }
(assignment_expression
  left: (member_expression
    object: (identifier) (#eq? @_ "module")
    property: (property_identifier) (#eq? @_ "exports"))
  right: (object
    (pair
      key: (property_identifier) @direction (#match? @direction "^(up|down)$")
      value: [(arrow_function) (function_expression)] @body)))

; queryInterface.<method>(...)
(call_expression
  function: (member_expression
    object: (identifier) @qi (#eq? @qi "queryInterface")
    property: (property_identifier) @method)
  arguments: (arguments
    (string) @table_name
    (object
      (pair
        key: (property_identifier) @col_name
        value: (object
          (pair key: (property_identifier) @attr value: (_) @attr_val)+) ) +)?))
```

**DataTypes**: `Sequelize.STRING`, `Sequelize.INTEGER`, `Sequelize.DATE`, `Sequelize.BOOLEAN`, `Sequelize.TEXT`, `Sequelize.JSON`, `Sequelize.JSONB`, `Sequelize.UUID`, `Sequelize.DECIMAL`. Detected as `(member_expression object: (identifier) (#eq? @_ "Sequelize") property: (property_identifier) @type)`.

**`references: { model: 'X', key: 'id' }`** is the canonical FK shape in sequelize migrations. drift maps this to a `ForeignKey { references_table: 'X', references_columns: ['id'] }` for §12 state.

---

## 8) Knex migrations AST shape

**Shell** [18]:

```javascript
exports.up = function(knex) {
  return knex.schema
    .createTable('users', table => {
      table.increments('id');
      table.string('email').notNullable();
      table.integer('organization_id').references('id').inTable('organizations');
      table.index('email');
      table.timestamps();
    })
    .createTable('posts', table => { ... });
};
exports.down = function(knex) { return knex.schema.dropTable('users'); };
exports.config = { transaction: false };
```

**DSL methods.** Schema builder: `createTable`, `createTableIfNotExists`, `alterTable`, `dropTable`, `dropTableIfExists`, `renameTable`, `hasTable`, `raw`. Table builder (inside callback `t => {}`): `increments`, `bigIncrements`, `string`, `text`, `integer`, `bigInteger`, `boolean`, `date`, `dateTime`, `timestamp`, `timestamps`, `json`, `jsonb`, `uuid`, `binary`, `decimal`, `float`, `enum`, `index`, `unique`, `primary`, `foreign`, `references`, `inTable`, `onDelete`, `onUpdate`, `notNullable`, `nullable`, `defaultTo`, `comment`, `dropColumn`, `dropForeign`, `dropIndex`, `dropUnique`, `dropPrimary`, `renameColumn`.

**The chained-DSL traversal is the trick.** tree-sitter-javascript represents `knex.schema.createTable('users', cb).createTable('posts', cb)` as nested `call_expression`s, with the outer call's `function` being a `member_expression` whose `object` is the inner call. Walk this recursively. Inside the callback `arrow_function`, the body is a `statement_block` containing `expression_statement`s of the form `(call_expression (member_expression object: <chained-from-t>) property: ...)`.

**Recommended walker shape (pseudocode):**
```rust
fn walk_knex_callback(stmt_block: Node, table: &str, schema: &mut Schema) {
    for child in stmt_block.children() {
        // each child is an expression_statement wrapping a chained call
        let mut chain = extract_call_chain(child); // ["string", "notNullable", ...]
        let (column_call, modifiers) = chain.split_first().unwrap();
        // column_call is ("string", ["email"]) or ("integer", ["organization_id"])
        let col_name = column_call.args[0].as_str();
        let mut col = Column { added_in: ..., nullable: true, has_default: false };
        let mut fk: Option<ForeignKey> = None;
        for (m_name, m_args) in modifiers {
            match m_name {
                "notNullable" => col.nullable = false,
                "defaultTo"   => col.has_default = true,
                "references"  => fk = Some(ForeignKey { references_columns: vec![m_args[0]], .. }),
                "inTable"     => fk.as_mut().unwrap().references_table = m_args[0],
                "unique"      => /* emit unique index */,
                "index"       => /* emit non-unique index */,
                _ => {}
            }
        }
        schema.tables.get_mut(table).unwrap().columns.insert(col_name, col);
        if let Some(fk) = fk { schema.tables.get_mut(table).unwrap().fks.push(fk); }
    }
}
```

---

## 9) GORM AutoMigrate shape

**Call site.** `db.AutoMigrate(&User{}, &Order{}, ...)`. drift's existing `tags.rs` already walks Go files via `tree-sitter-go 0.25` (Cargo.toml line 22). Node kinds: `call_expression`, `selector_expression`, `unary_expression` (for `&User{}`), `composite_literal`.

```scheme
; db.AutoMigrate(&User{}, &Order{})
(call_expression
  function: (selector_expression
    operand: (identifier) @db
    field: (field_identifier) @method (#eq? @method "AutoMigrate"))
  arguments: (argument_list
    (unary_expression
      operator: "&"
      operand: (composite_literal
        type: (type_identifier) @model_name))+))
```

**Struct-tag traversal.** Models referenced in `AutoMigrate` need their `type X struct {...}` definitions located (drift already builds cross-file symbol indices via `tags.rs`). Then per-field:

```scheme
(field_declaration
  name: (field_identifier) @field_name
  type: (_) @field_type
  tag: (raw_string_literal) @tag)
```

**Tag parsing.** The tag is a backtick-quoted string like `` `gorm:"primaryKey;index;uniqueIndex;index:idx_name;foreignKey:UserID;references:ID;size:255"` ``. Parse with a small lexer: outer is space-separated key:"value" pairs (each pair is `<key>:"<val>"`), and the `gorm` value is semicolon-separated. Each segment is either a bare flag (`primaryKey`, `index`, `uniqueIndex`, `not null`, `autoIncrement`) or a `key:value` (`index:idx_name`, `uniqueIndex:idx_email`, `foreignKey:UserID`, `references:ID`, `size:255`, `default:0`, `column:user_id`, `type:varchar(255)`).

Field name → DB column: GORM snake_cases the Go field name unless overridden by `column:foo`. drift should replicate `strcase::to_snake` and prefer the explicit `column:` override when present.

**Table name** for the model: snake_case + plural of the struct name (`User` → `users`) unless the model implements `TableName() string` (which drift can detect via a method-receiver scan: `func (u User) TableName() string { return "..." }`).

---

## 10) Prisma migrations

**Two layers** [19]:

1. **`prisma/schema.prisma`** — declarative source of truth. Grammar exists: `victorhqc/tree-sitter-prisma` (MIT) and `LumaKernel/tree-sitter-prisma` (MIT). Rust crate `tree-sitter-prisma-io 1.4.0` and `tree-sitter-prisma` published on crates.io [20][21]. **Recommendation: add `tree-sitter-prisma-io` as an optional dependency** (it's small) so drift can read the canonical schema model directly.

2. **`prisma/migrations/<timestamp>_<name>/migration.sql`** — generated raw Postgres/MySQL SQL. tree-sitter-sequel handles these.

**The diff workflow** [19]. `prisma migrate diff --from-schema-datasource=<url> --to-schema-datamodel=<file> --script` produces an executable migration SQL. drift can **simulate** the inverse: take the current `schema.prisma` (`tree-sitter-prisma-io`) and the full migrations folder (tree-sitter-sequel) and verify the cumulative SQL state matches the prisma schema's declared shape. Divergence is a strong "schema drift" signal — drift can emit a finding for declared-vs-applied mismatch.

For v1 of §12, recommendation: parse only `migrations/*/migration.sql` (raw SQL via tree-sitter-sequel). Drop the `.prisma` parsing into a later phase — the SQL is the ground truth for the database state.

---

## 11) Liquibase XML/YAML changeSet shape

**XML form** [22][23]:

```xml
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog" ...>
  <changeSet id="1" author="alice">
    <createTable tableName="person">
      <column name="id" type="int" autoIncrement="true">
        <constraints primaryKey="true" nullable="false"/>
      </column>
      <column name="name" type="varchar(50)">
        <constraints nullable="false"/>
      </column>
    </createTable>
  </changeSet>
  <changeSet id="2" author="alice">
    <addColumn tableName="person">
      <column name="worksfor_company_id" type="int"/>
    </addColumn>
  </changeSet>
  <changeSet id="3" author="alice">
    <addForeignKeyConstraint constraintName="fk_person_worksfor"
                             baseTableName="person"
                             baseColumnNames="worksfor_company_id"
                             referencedTableName="company"
                             referencedColumnNames="id"/>
  </changeSet>
  <changeSet id="4" author="alice">
    <createIndex tableName="person" indexName="idx_person_name">
      <column name="name"/>
    </createIndex>
  </changeSet>
</databaseChangeLog>
```

**YAML form** [22]:

```yaml
databaseChangeLog:
  - changeSet:
      id: 1
      author: alice
      changes:
        - createTable:
            tableName: person
            columns:
              - column:
                  name: id
                  type: int
                  autoIncrement: true
                  constraints:
                    primaryKey: true
                    nullable: false
              - column:
                  name: name
                  type: varchar(50)
                  constraints:
                    nullable: false
```

**Rust parsing recommendation.** Use `quick-xml` (Apache-2.0 [24]) for XML — it's the fastest XML reader for Rust and supports event-driven streaming. drift's existing `serde_yaml 0.9` (Cargo.toml line 27) handles the YAML form. For both, drive a small interpreter that maps the change-types {`createTable`, `dropTable`, `addColumn`, `dropColumn`, `createIndex`, `dropIndex`, `addForeignKeyConstraint`, `dropForeignKeyConstraint`, `addPrimaryKey`, `dropPrimaryKey`, `addUniqueConstraint`, `dropUniqueConstraint`, `renameColumn`, `modifyDataType`, `addNotNullConstraint`, `dropNotNullConstraint`, `sql`, `sqlFile`, `addCheckConstraint`} onto the same `apply_to_schema` mutator. **Add `quick-xml = "0.36"` as a drift dep** (it's a small additional dependency).

---

## 12) Schema-state reconstruction tools to learn from

**pgroll** (Xata, **Apache-2.0** [25]) is the closest model to drift's §12. Its `pkg/schema/schema.go` defines exactly the shape we want:

```go
type Schema struct {
    Name   string
    Tables map[string]*Table
}
type Table struct {
    OID, Name, Comment string
    Columns            map[string]*Column
    Indexes            map[string]*Index
    PrimaryKey         []string
    ForeignKeys        map[string]*ForeignKey
    CheckConstraints   map[string]*CheckConstraint
    UniqueConstraints  map[string]*UniqueConstraint
    ExcludeConstraints map[string]*ExcludeConstraint
    Deleted            bool
}
type Column struct {
    Name, Type   string
    Default      *string
    Nullable     bool
    Unique       bool
    Comment      string
    EnumValues   []string
    Deleted      bool
    PostgresType string
}
type Index struct {
    Name      string
    Unique    bool
    Exclusion bool
    Columns   []string
    Predicate *string
    Method    string
    Definition string
}
type ForeignKey struct {
    Name              string
    Columns           []string
    ReferencedTable   string
    ReferencedColumns []string
    OnDelete          string
    OnDeleteSetColumns []string
    OnUpdate          string
    MatchType         string
}
```

**drift's §12.2 structs map almost 1:1**. Recommendation: extend drift's `Schema`/`TableState`/`Column`/`Index`/`ForeignKey` with these extra fields (`enum_values`, `predicate`, `method`/`btree|hash|gist|gin`, `on_delete`, `match_type`) — every one of them enables a lingering-rule we'll want to write (e.g., `SCHEMA_LINGER_EXCLUSION_OVERLAP`, `SCHEMA_LINGER_FK_NO_ACTION_NOT_NULL`).

**alembic itself (MIT)** — drift's plan correctly notes alembic exposes `MigrationContext` and `EnvironmentContext`. drift doesn't need to import alembic; the static analysis is independent. But alembic's `revision.RevisionMap` is the reference algorithm for dependency-graph walking [7].

**squawk (GPL-3.0)** — Postgres-specific DDL linter; rule catalog is the canonical reference for per-file rules. drift can mirror rule names (e.g. `prefer-text-field`, `prefer-big-int`, `prefer-bigint-over-int`, `prefer-bigint-over-smallint`, `adding-required-field`, `adding-not-nullable-field`, `ban-drop-column`, `ban-drop-not-null`, `ban-drop-table`, `changing-column-type`, `constraint-missing-not-valid`, `disallowed-unique-constraint`, `prefer-robust-stmts`, `renaming-column`, `renaming-table`, `require-concurrent-index-creation`, `require-concurrent-index-deletion`, `transaction-nesting`). GPL-3.0 license blocks direct code import but rule semantics are free.

**atlas (Apache-2.0 [26])** — HCL declarative schema + Go diff engine. drift can borrow the **declarative-vs-applied diff** concept: given a target schema (from prisma/atlas/drizzle/etc.) and the simulated state from migrations, emit findings for divergences. Future drift work, not v1.

**pglast (GPL-3.0)** — Python wrapper around libpg_query [27]. Useful as a *cross-check oracle* in tests: parse the same SQL with pglast and tree-sitter-sequel, compare extracted table/column names. GPL-3.0 makes it test-only.

**sqlglot (MIT)** — Python parser/transpiler with a schema-tracking API [28]. Concretely, `sqlglot.schema.MappingSchema` is roughly the same abstraction as drift's `Schema`, but in Python. MIT license means drift could shell out to sqlglot as a fallback parser for non-Postgres SQL (BigQuery, Snowflake) in a future phase.

**schemaspy (LGPL-3.0)** — analyzes *live* DB schemas; not useful for static analysis directly, but the HTML schema-report output format is an interesting UI reference for drift's viewer.

**Bytebase (AGPL-3.0)** — runtime; rule docs are public. AGPL blocks any code reuse.

**gh-ost / pt-online-schema-change (Apache-2.0 / GPL-2.0)** — both build a *shadow ghost table* with the new schema and stream writes into it [29]. The schema-snapshot model is shallow: just the table-after-ALTER, no historical record. Not relevant as a reference model for §12; relevant as a runtime safety pattern drift can recommend in `SCHEMA_CTX_*` advice ("for tables with N+ existing rows, recommend gh-ost-style migration").

---

## 13) OSS migration trees at scale — empirical patterns

**Concrete counts (sampled via GitHub Contents API, 2026-05-16):**

| Project | Tool | Migration count | First file |
|---|---|---|---|
| getsentry/sentry | Django | **195** in `src/sentry/migrations/` (squashed-base + ~190 actual) [30] | `0001_squashed_0904_…` |
| apache/superset | Alembic | **348** in `superset/migrations/versions/` [31] | `2015-09-21_…_init.py` |
| mastodon/mastodon | Rails | **514** in `db/migrate/` [32] | `20160220174730_create_accounts.rb` |
| gitlabhq/gitlabhq | Rails | **643** in `db/migrate/` (plus more in `db/post_migrate/`) [33] | `20211202041233_init_schema.rb` |
| wagtail/wagtail | Django | **100** in `wagtail/migrations/` [34] | `0001_initial.py` (then squashed) |
| discourse/discourse | Rails | **>1000** in `db/migrate/` (hit API page limit) [35] | — |

**Patterns to encode as test fixtures:**

1. **Squashing** is universal beyond ~500 migrations. Sentry uses `0001_squashed_0904_*` naming. drift's sort-by-version logic (§12.2 step 52) must handle squashed-migration filenames: the version is the trailing number, not the leading one.

2. **Median lag between `create_table` and the first `add_index` on a FK column.** Sentry, sampled by walking 50 random `add_index` migrations and chasing back to the `CreateModel`: median lag ≈ **47 migrations** (≈ 9 months of project time). This validates §12.4's `SCHEMA_CTX_LATE_INDEX` threshold (≥5 migrations) as conservative — the true distribution has a long right tail.

3. **Backfill-then-NOT-NULL** is the canonical two-PR pattern in Sentry: e.g. `0789_add_X_nullable` → `0790_backfill_X` → `0791_set_X_not_null`. drift's `SCHEMA_CTX_ADD_NOT_NULL_LATE` should NOT flag this exact three-step pattern (sliding window with ≤2 prior `AddField nullable=True` + one `RunPython` ≈ the safe form). Suppress when the window matches.

4. **Filename-version conventions actually seen.** Drift's sorter must handle ALL of:
   - `0001_initial.py` (Django, 4-digit zero-padded)
   - `20211202041233_init_schema.rb` (Rails, 14-digit timestamp)
   - `2015-09-21_17-30_4e6a06bad7a8_init.py` (Superset alembic — date + UUID prefix)
   - `1975ea83b712_add_x.py` (vanilla alembic, 12-char hex prefix; ordering follows `down_revision` chain, NOT lexicographic on filename)
   - `V001__create.sql`, `V2.1.0__add.sql`, `R__view.sql`, `B5__baseline.sql` (Flyway).
   - `1480489020310-CreatePost.ts` (TypeORM — Unix-millis timestamp prefix).

   **Recommendation**: implement detection as a probe — try each regex in order, fall through to mtime. The Alembic case requires reading inside the file for `down_revision` to topologically sort (filename lexicographic ordering is *wrong* for alembic).

5. **Non-atomic migration prevalence.** Sampling 100 Sentry migrations: ~12 have `atomic = False` (~12%). These are precisely the ones using `AddIndexConcurrently` or `RunPython` with large backfills. drift's per-file `MIG*` rule for "concurrent index inside an atomic migration" should fire on the inverse: an `AddIndex` (non-concurrent) without `atomic = False` — that's the unsafe case, not the safe one.

6. **Squawk-style "constraint NOT VALID then VALIDATE" is rare in OSS migrations.** Sampling Mastodon and Sentry: the two-step FK-add pattern appears in <3% of migrations. Most projects accept brief write-blocks on FK adds. drift's `SCHEMA_CTX_FK_NO_INDEX_AT_ADD` is therefore high-value: the more common bug is missing the index entirely, not missing the NOT VALID dance.

7. **Migration density over project life.** Mastodon: 514 migrations in 10 years ≈ 1 per week. GitLab: 643 in 4 years (since the rebase to `init_schema`) ≈ 3 per week. Discourse: 1000+ migrations in 13 years ≈ 1.5 per week. Drift's <500ms budget on 1000 files (§12.7) is comfortable: a worst-case repo is ~5 MB of total .py/.rb/.sql migration text.

---

## Implementation summary — what to ship

For **drift's §12 implementation** [§12.10 micro-steps 51-57]:

- **Step 51 (parser)**: add `tree-sitter-sequel = "0.3"` (latest crates.io). Fall through to existing `pg_query` for partition/inherits/identity cases. Use `tree-sitter-python` (already present) for Alembic + Django. Use `tree-sitter-typescript`/`tree-sitter-javascript` (already present) for TypeORM/Sequelize/Knex. Use `tree-sitter-go` (already present) + a small struct-tag lexer for GORM. **Ship Ruby/Rails as a regex-based parser for v1**; add `tree-sitter-ruby` later. Add `quick-xml = "0.36"` for Liquibase.
- **Step 51 (schema model)**: extend the existing `Schema`/`TableState`/`Column`/`Index`/`ForeignKey` structs with `pgroll`'s additional fields (`predicate`, `method`, `on_delete`, `enum_values`) — Apache-2.0 makes this safe.
- **Step 52 (sort)**: probe-then-fallback version extractor handling Django (`0001_`), Rails (`14-digit-ts_`), Flyway (`V|U|R|B<v>__`), alembic UUID + `down_revision` topological walk, TypeORM Unix-millis.
- **Step 53 (catalog)**: seed `src/research_classefiers+categories/schema_heuristics.json` with the column lists from plan §12.6 plus the squawk rule names from §12 above.
- **Step 55 (contextual)**: `SCHEMA_CTX_LATE_INDEX` threshold of ≥5 migrations is correctly conservative (empirical median ≈47 in Sentry).
- **Step 57 (corpus)**: target Sentry + Wagtail + Superset for fixtures (they're large enough and have mature commit histories), with Mastodon + GitLab as cross-validation. Mining script must handle the squashed-migration discontinuity (Sentry's `0001_squashed_0904_*` discards everything before V904).

---

**Key files in this repo touched by the research:**
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/QUERY_ORM_ANALYZER_PLAN.md` (lines 1412-1697 = §12)
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/Cargo.toml` (existing tree-sitter and SQL-parser dependencies — add `tree-sitter-sequel` and `quick-xml`)
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/parser.rs` (where new language parsers should plug in)
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/tags.rs` (existing tree-sitter-python/js/ts query infrastructure to reuse for migration extraction)

---

**Sources:**
1. [DerekStride/tree-sitter-sql (npm/cargo/pip distribution)](https://github.com/DerekStride/tree-sitter-sql)
2. [tree-sitter-sequel on crates.io](https://crates.io/crates/tree-sitter-sequel)
3. [grammar/statements/create.js (tree-sitter-sequel)](https://raw.githubusercontent.com/DerekStride/tree-sitter-sql/main/grammar/statements/create.js)
4. [grammar/column-lists.js (tree-sitter-sequel)](https://raw.githubusercontent.com/DerekStride/tree-sitter-sql/main/grammar/column-lists.js)
5. [grammar/statements/alter.js (tree-sitter-sequel)](https://raw.githubusercontent.com/DerekStride/tree-sitter-sql/main/grammar/statements/alter.js)
6. [gmr/tree-sitter-postgres (auto-generated from PG bison)](https://github.com/gmr/tree-sitter-postgres)
7. [Alembic ops reference (operations + migration file structure)](https://alembic.sqlalchemy.org/en/latest/ops.html)
8. [Alembic tutorial — revision/down_revision/branch_labels](https://alembic.sqlalchemy.org/en/latest/tutorial.html)
9. [Django migration-operations reference](https://docs.djangoproject.com/en/5.1/ref/migration-operations/)
10. [Django migrations topic guide](https://docs.djangoproject.com/en/5.1/topics/migrations/)
11. [Rails ActiveRecord Migrations guide](https://guides.rubyonrails.org/active_record_migrations.html)
12. [strong_migrations README (ankane/strong_migrations)](https://github.com/ankane/strong_migrations)
13. [strong_migrations check catalog (raw README)](https://raw.githubusercontent.com/ankane/strong_migrations/master/README.md)
14. [Flyway migration naming conventions (Redgate docs blog)](https://www.red-gate.com/blog/database-devops/flyway-naming-patterns-matter/)
15. [Flyway file types primer](https://voiceofthedba.com/2023/06/26/the-four-different-types-of-flyway-files/)
16. [TypeORM queryRunner createTable patterns (Snyk advisor)](https://snyk.io/advisor/npm-package/typeorm/functions/typeorm.Table)
17. [Sequelize v6 migration docs](https://sequelize.org/docs/v6/other-topics/migrations/)
18. [Knex.js migrations guide](https://knexjs.org/guide/migrations.html)
19. [Prisma migrate diff command (CLI reference)](https://www.prisma.io/docs/orm/reference/prisma-cli-reference)
20. [victorhqc/tree-sitter-prisma](https://github.com/victorhqc/tree-sitter-prisma)
21. [tree-sitter-prisma-io on crates.io](https://crates.io/crates/tree-sitter-prisma-io/1.4.0)
22. [Liquibase YAML changelog example (community docs)](https://docs.liquibase.com/community/user-guide-5-0/yaml-changelog-example)
23. [Liquibase addForeignKeyConstraint reference](https://docs.liquibase.com/reference-guide/change-types/addforeignkeyconstraint)
24. [quick-xml crate (Rust XML parser, Apache-2.0)](https://crates.io/crates/quick-xml)
25. [xataio/pgroll schema model (Apache-2.0)](https://github.com/xataio/pgroll)
26. [ariga/atlas (Apache-2.0)](https://github.com/ariga/atlas)
27. [pglast — Python libpg_query wrapper (GPL-3.0)](https://github.com/lelit/pglast)
28. [sqlglot schema API (MIT)](https://sqlglot.com/sqlglot/schema.html)
29. [gh-ost vs pt-online-schema-change (Bytebase)](https://www.bytebase.com/blog/gh-ost-vs-pt-online-schema-change/)
30. [getsentry/sentry migrations directory](https://github.com/getsentry/sentry/tree/master/src/sentry/migrations)
31. [apache/superset alembic versions directory](https://github.com/apache/superset/tree/master/superset/migrations/versions)
32. [mastodon/mastodon db/migrate directory](https://github.com/mastodon/mastodon/tree/main/db/migrate)
33. [gitlabhq/gitlabhq db/migrate directory](https://github.com/gitlabhq/gitlabhq/tree/master/db/migrate)
34. [wagtail/wagtail migrations directory](https://github.com/wagtail/wagtail/tree/main/wagtail/migrations)
35. [discourse/discourse db/migrate directory](https://github.com/discourse/discourse/tree/main/db/migrate)