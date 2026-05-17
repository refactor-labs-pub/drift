//! Kotlin — `tree-sitter-kotlin-ng` 1.1 binding + tags query.
//!
//! `tree-sitter-kotlin-ng` (the actively-maintained fork) collapses
//! classes, interfaces, and enums into a single `class_declaration`
//! node distinguished only by an unnamed `class`/`interface`/`enum`
//! keyword token — so one capture handles all three. Singletons
//! (`object Foo`) live under a separate `object_declaration` node
//! and are captured the same way so methods inside them get the
//! right parent.
//!
//! Call shapes:
//!   - `foo()`     → `call_expression` with an `identifier` child
//!   - `obj.foo()` → `call_expression` with `navigation_expression(receiver, name)`
//!   - `Type(...)` → `call_expression` with `identifier` (constructor
//!                   invocation syntactically identical to a function
//!                   call in Kotlin)
//!
//! The `navigation_expression` has positional `(expression,
//! identifier)` children — first is the receiver, second is the
//! method name.
//!
//! Imports: `import a.b.c`, `import a.b.c.*`, and `import a.b.c as d`.
//! The wildcard form has no separate marker — the trailing `.*` is
//! unnamed tokens. Aliased form adds a trailing `identifier`.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @def.name) @def.function

(class_declaration
  name: (identifier) @def.name) @def.class

(object_declaration
  name: (identifier) @def.name) @def.class

(call_expression
  (identifier) @ref.name) @ref.call

(call_expression
  (navigation_expression
    (_) @ref.receiver
    (identifier) @ref.name)) @ref.call

(import
  (qualified_identifier) @import.module)

(import
  (qualified_identifier) @import.module
  (identifier) @import.alias)
"#;
