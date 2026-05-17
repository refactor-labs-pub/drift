//! Python — `tree-sitter-python` 0.23 binding + tags query.
//!
//! Notable grammar shapes consumed by `TAGS_QUERY`:
//!   - `function_definition` / `class_definition` for symbols
//!   - `call` with `attribute` (method) or bare `identifier` (function)
//!   - `import_statement` / `import_from_statement` for imports
//!   - SQL-sink predicate on `attribute.attribute` against the regex
//!     `^(execute|executemany|raw|executescript)$` — covers psycopg,
//!     pymysql, sqlite3, asyncpg, motor, SQLAlchemy session.execute,
//!     Django Manager.raw.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_python::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
(function_definition
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(class_definition
  name: (identifier) @def.name
  body: (_) @def.body) @def.class

(call function: (identifier) @ref.name) @ref.call

(call function: (attribute
  object: (_) @ref.receiver
  attribute: (identifier) @ref.name)) @ref.call

(import_statement
  name: (dotted_name) @import.module)

(import_statement
  name: (aliased_import
    name: (dotted_name) @import.module
    alias: (identifier) @import.alias))

(import_from_statement
  module_name: (dotted_name) @import.module
  name: (dotted_name) @import.name)

(import_from_statement
  module_name: (dotted_name) @import.module
  name: (aliased_import
    name: (dotted_name) @import.name
    alias: (identifier) @import.alias))

; SQL sinks: receiver.method("SQL"). Covers DB-API drivers
; (psycopg, pymysql, sqlite3, asyncpg, motor wrapping aside), SQLAlchemy
; session.execute / connection.execute, Django Manager.raw, and the
; common .executemany variant. Predicate-filtered so non-SQL "execute"
; methods on unrelated objects don't pollute the capture.
(call function: (attribute
  object: (_) @ref.receiver
  attribute: (identifier) @ref.name
  (#match? @ref.name "^(execute|executemany|raw|executescript)$"))
  arguments: (argument_list . (string) @ref.sql_literal)) @ref.call

; SQLAlchemy text("SELECT ...") wrapper — bare function call with a
; string literal as the first argument. Tier C / D classification on
; the receiver-less call still fires; this just adds the SQL string.
(call function: (identifier) @ref.name
  (#eq? @ref.name "text")
  arguments: (argument_list . (string) @ref.sql_literal)) @ref.call
"#;
