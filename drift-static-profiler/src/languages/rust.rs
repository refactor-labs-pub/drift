//! Rust — `tree-sitter-rust` 0.24 binding + tags query.
//!
//! `impl T { fn m() {} }` puts the `function_item` inside an
//! `impl_item`, so containment sets the function's parent to `T` —
//! receiver-based resolution then works the same way it does for
//! Java/TS classes.
//!
//! Call shapes (verified against tree-sitter-rust 0.24):
//!   - `foo()`              → `call_expression(function: identifier)`
//!   - `Type::assoc()`      → `call_expression(function: scoped_identifier)`
//!   - `mod::sub::foo()`    → `call_expression(function: scoped_identifier)`
//!   - `obj.method()`       → `call_expression(function: field_expression)`
//!
//! All four are `call_expression` at the top — there is NO
//! `method_call_expression` node in this grammar (that name appears in
//! some older docs but isn't in tree-sitter-rust 0.24's grammar.js).
//! Macros (`println!()`) are deliberately skipped — they're noisy and
//! rarely lead to interesting call edges.
//!
//! Turbofish: `foo::<T>()` and `obj.collect::<Vec<_>>()` wrap the
//! function in a `generic_function` node, so the plain identifier and
//! field_expression patterns above don't fire — we add explicit
//! `generic_function` patterns.
//!
//! `use` declarations: cover the common forms. `use a::b::{c, d}`
//! (use_list) is intentionally not enumerated.
//!
//! SQL-sink coverage: `sqlx::query!`, `sqlx::query_as!`,
//! `sqlx::query_scalar!` macros (the canonical Rust embedded-SQL
//! shape) plus tokio-postgres `client.query`/`execute` method calls.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
(function_item
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(impl_item
  type: (type_identifier) @def.name) @def.class

(struct_item
  name: (type_identifier) @def.name) @def.class

(trait_item
  name: (type_identifier) @def.name) @def.class

(enum_item
  name: (type_identifier) @def.name) @def.class

(call_expression
  function: (identifier) @ref.name) @ref.call

(call_expression
  function: (scoped_identifier
    path: (_) @ref.receiver
    name: (identifier) @ref.name)) @ref.call

(call_expression
  function: (field_expression
    value: (_) @ref.receiver
    field: (field_identifier) @ref.name)) @ref.call

; Turbofish-qualified calls. `foo::<T>()` and `obj.collect::<Vec<_>>()` wrap
; the function in a `generic_function` node, so the plain identifier and
; field_expression patterns above don't fire. Both shapes are common enough
; in real Rust code that missing them produces visible call-graph holes.
(call_expression
  function: (generic_function
    function: (identifier) @ref.name)) @ref.call

(call_expression
  function: (generic_function
    function: (field_expression
      value: (_) @ref.receiver
      field: (field_identifier) @ref.name))) @ref.call

(use_declaration
  argument: (scoped_identifier) @import.module)

(use_declaration
  argument: (use_as_clause
    path: (_) @import.module
    alias: (identifier) @import.alias))

; SQL sinks: sqlx::query!, query_as!, query_scalar! macros — the
; canonical Rust embedded-SQL shape. Macros are tree-sitter
; `macro_invocation` nodes whose token_tree contains the raw arg list.
; We capture the macro name + the first string literal inside the
; token_tree. token_tree's grammar is permissive so we use a `string_literal`
; descendant match.
(macro_invocation
  macro: (scoped_identifier
    path: (identifier) @ref.receiver
    name: (identifier) @ref.name)
  (#eq? @ref.receiver "sqlx")
  (#match? @ref.name "^(query|query_as|query_scalar|query_unchecked|query_as_unchecked|query_file|query_file_as)$")
  (token_tree (string_literal) @ref.sql_literal)) @ref.call

; Unqualified macro form when `use sqlx::query;` is in scope:
(macro_invocation
  macro: (identifier) @ref.name
  (#match? @ref.name "^(query|query_as|query_scalar|query_unchecked|query_as_unchecked|query_file|query_file_as)$")
  (token_tree (string_literal) @ref.sql_literal)) @ref.call

; tokio-postgres / deadpool-postgres / postgres method calls:
;   client.query("SELECT ...", &[...]).await
;   txn.execute("UPDATE ...", &[]).await
(call_expression
  function: (field_expression
    value: (_) @ref.receiver
    field: (field_identifier) @ref.name)
  (#match? @ref.name "^(query|query_one|query_opt|query_raw|execute|batch_execute|simple_query|prepare|fetch_all|fetch_one|fetch_optional)$")
  arguments: (arguments . (string_literal) @ref.sql_literal)) @ref.call
"#;
