//! Scala — `tree-sitter-scala` 0.26 binding + tags query.
//!
//! Scala mirrors Java's class/method structure, plus `object`
//! (singletons) and `trait` (interfaces). All three are treated as
//! `def.class` so methods inside them inherit the right parent via
//! containment.
//!
//! Call shapes:
//!   - `foo()`          → `call_expression(function: identifier)`
//!   - `obj.method()`   → `call_expression(function: field_expression)`
//!   - `obj method arg` → `infix_expression` (Scala-specific; not yet captured)
//!
//! Infix calls and method calls without parens are common in Scala
//! but skipped for v1 — the cost is some missed call edges, not
//! incorrect ones.
//!
//! No SQL-sink pattern (Scala embedded SQL via Slick/Doobie/Quill is
//! usually a typed DSL, not a string literal; covered by ORM-lint
//! later, not by the inline-SQL pipeline).

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_scala::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
(function_definition
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(class_definition
  name: (identifier) @def.name) @def.class

(object_definition
  name: (identifier) @def.name) @def.class

(trait_definition
  name: (identifier) @def.name) @def.class

(call_expression
  function: (identifier) @ref.name) @ref.call

(call_expression
  function: (field_expression
    value: (_) @ref.receiver
    field: (identifier) @ref.name)) @ref.call

(import_declaration
  path: (_) @import.module)
"#;
