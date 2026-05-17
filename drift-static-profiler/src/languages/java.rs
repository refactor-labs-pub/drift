//! Java — `tree-sitter-java` 0.23 binding + tags query.
//!
//! Notable grammar shapes:
//!   - `method_declaration` / `class_declaration` / `interface_declaration`
//!     all map to def.method / def.class
//!   - `method_invocation` with or without `object:` field
//!   - `object_creation_expression` for `new Foo()`
//!   - SQL-sink regex covers JDBC Statement.execute*, EntityManager
//!     createNativeQuery / createQuery, Spring JdbcTemplate
//!     query / queryForObject / queryForList / update / execute /
//!     batchUpdate.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_java::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
(method_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.method

(class_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.class

(interface_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.class

(method_invocation
  object: (_) @ref.receiver
  name: (identifier) @ref.name) @ref.call

(method_invocation
  name: (identifier) @ref.name
  !object) @ref.call

(object_creation_expression
  type: (type_identifier) @ref.name) @ref.call

(import_declaration
  (scoped_identifier) @import.module)

; SQL sinks: JDBC Statement.execute*, EntityManager.createNativeQuery /
; createQuery, Spring JdbcTemplate query/queryForObject/queryForList/
; update / execute. Predicate restricts to known SQL-bearing names so
; unrelated `execute()` methods on Runnables/Futures don't match.
(method_invocation
  object: (_) @ref.receiver
  name: (identifier) @ref.name
  (#match? @ref.name "^(executeQuery|executeUpdate|createNativeQuery|createQuery|queryForObject|queryForList|queryForMap|queryForRowSet|update|query|execute|batchUpdate)$")
  arguments: (argument_list . (string_literal) @ref.sql_literal)) @ref.call
"#;
