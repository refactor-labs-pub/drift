//! TypeScript — `tree-sitter-typescript` 0.23 (LANGUAGE_TYPESCRIPT
//! variant; the same crate also exposes a TSX variant we don't pick
//! here since drift's tags-query is identical) + tags query.
//!
//! Notable grammar shapes:
//!   - `function_declaration`, `method_definition`, `class_declaration`
//!     all decorated with `type_identifier` for the class name (vs JS
//!     using plain `identifier`).
//!   - `call_expression` with `member_expression` or bare `identifier`
//!   - `new_expression` for constructors
//!   - SQL-sink regex covers pg/node-postgres `client.query`, mysql2
//!     `.query` / `.execute`, Knex `.raw`, Prisma `$queryRaw` /
//!     `$executeRaw`, TypeORM `dataSource.query`.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

pub const TAGS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(method_definition
  name: (property_identifier) @def.name
  body: (_) @def.body) @def.method

(class_declaration
  name: (type_identifier) @def.name
  body: (_) @def.body) @def.class

(call_expression
  function: (identifier) @ref.name) @ref.call

(call_expression
  function: (member_expression
    object: (_) @ref.receiver
    property: (property_identifier) @ref.name)) @ref.call

(new_expression
  constructor: (identifier) @ref.name) @ref.call

(import_statement
  source: (string (string_fragment) @import.module))

(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @import.name)))
  source: (string (string_fragment) @import.module))

(import_statement
  (import_clause
    (namespace_import (identifier) @import.alias))
  source: (string (string_fragment) @import.module))

; SQL sinks: pg/node-postgres client.query, mysql2 .query/.execute,
; Knex .raw, Prisma $queryRaw/$executeRaw, TypeORM dataSource.query.
; Restrict to a known set so generic .query() / .execute() on unrelated
; objects doesn't pollute the capture. Predicate scoped to the
; member_expression parent so it filters the method-name match cleanly.
(call_expression
  function: (member_expression
    object: (_) @ref.receiver
    property: (property_identifier) @ref.name
    (#match? @ref.name "^(query|queryRaw|queryRawUnsafe|executeRaw|executeRawUnsafe|raw|execute)$"))
  arguments: (arguments (string) @ref.sql_literal)) @ref.call
"#;
