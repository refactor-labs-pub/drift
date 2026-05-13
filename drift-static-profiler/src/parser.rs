use crate::Language;
use tree_sitter::Language as TsLanguage;

pub fn language_for(lang: Language) -> TsLanguage {
    match lang {
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Scala => tree_sitter_scala::LANGUAGE.into(),
        Language::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
    }
}

pub fn tags_query(lang: Language) -> &'static str {
    match lang {
        Language::Python => PYTHON_QUERY,
        Language::Java => JAVA_QUERY,
        Language::TypeScript => TYPESCRIPT_QUERY,
        Language::JavaScript => JAVASCRIPT_QUERY,
        Language::Go => GO_QUERY,
        Language::Rust => RUST_QUERY,
        Language::Scala => SCALA_QUERY,
        Language::Kotlin => KOTLIN_QUERY,
    }
}

// Capture name conventions used by all language queries:
//   @def.name / @def.function / @def.method / @def.class
//   @ref.name             — method/function being called
//   @ref.receiver         — the object/scope before the call (optional)
//   @ref.call             — the whole call site (for byte range / scope)
//   @import.module        — module path string node
//   @import.name          — imported identifier (None = whole-module)
//   @import.alias         — local binding name when aliased

const PYTHON_QUERY: &str = r#"
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
"#;

const JAVA_QUERY: &str = r#"
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
"#;

const TYPESCRIPT_QUERY: &str = r#"
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
"#;

// JavaScript is similar to TypeScript without type identifiers
const JAVASCRIPT_QUERY: &str = r#"
(function_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(method_definition
  name: (property_identifier) @def.name
  body: (_) @def.body) @def.method

(class_declaration
  name: (identifier) @def.name
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

(variable_declarator
  name: (identifier) @import.alias
  value: (call_expression
    function: (identifier) @_require_fn
    arguments: (arguments (string (string_fragment) @import.module)))
  (#eq? @_require_fn "require"))
"#;

// ── Go ───────────────────────────────────────────────────────────────────
// Go has no classes. Methods are top-level `func (recv T) M() {}` declarations;
// the receiver type would naturally be the "parent" for receiver-based dispatch,
// but containment-based parent resolution can't find it (there's no enclosing
// node), so Go method symbols come out with parent=None. The call graph still
// works because resolution is by name.
//
// Call shapes:
//   foo()         → call_expression(function: identifier)
//   pkg.Foo()     → call_expression(function: selector_expression)
//   r.M()         → call_expression(function: selector_expression)
// Both selector forms collapse to the same capture pattern.
//
// Imports: `import "fmt"` and `import f "fmt"`. The string path lives in an
// `interpreted_string_literal` whose text includes the surrounding quotes —
// tags.rs strips those before storing module_path.
const GO_QUERY: &str = r#"
(function_declaration
  name: (identifier) @def.name
  body: (_) @def.body) @def.function

(method_declaration
  name: (field_identifier) @def.name
  body: (_) @def.body) @def.method

(call_expression
  function: (identifier) @ref.name) @ref.call

(call_expression
  function: (selector_expression
    operand: (_) @ref.receiver
    field: (field_identifier) @ref.name)) @ref.call

(import_spec
  path: (interpreted_string_literal) @import.module)

(import_spec
  name: (package_identifier) @import.alias
  path: (interpreted_string_literal) @import.module)
"#;

// ── Rust ─────────────────────────────────────────────────────────────────
// `impl T { fn m() {} }` puts the function_item inside an impl_item, so
// containment sets the function's parent to "T" — receiver-based resolution
// then works the same way it does for Java/TS classes.
//
// Call shapes (verified against tree-sitter-rust 0.24):
//   foo()                → call_expression(function: identifier)
//   Type::assoc()        → call_expression(function: scoped_identifier)
//   mod::sub::foo()      → call_expression(function: scoped_identifier)
//   obj.method()         → call_expression(function: field_expression)
// All four are call_expression at the top — there is NO `method_call_expression`
// node in this grammar (that name appears in some older docs but isn't in
// tree-sitter-rust 0.24's grammar.js). Macros (`println!()`) are deliberately
// skipped — they're noisy and rarely lead to interesting call edges.
//
// `use` declarations: cover the common forms. `use a::b::{c, d}` (use_list)
// is intentionally not enumerated — the imports module path is still captured
// via the surrounding scoped_identifier when present, and per-name spec
// resolution is a step further than the existing language queries provide.
const RUST_QUERY: &str = r#"
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
"#;

// ── Scala ────────────────────────────────────────────────────────────────
// Scala mirrors Java's class/method structure, plus `object` (singletons) and
// `trait` (interfaces). All three are treated as def.class so methods inside
// them inherit the right parent via containment.
//
// Call shapes:
//   foo()              → call_expression(function: identifier)
//   obj.method()       → call_expression(function: field_expression)
//   obj method arg     → infix_expression (Scala-specific; not yet captured)
// Infix calls and method calls without parens are common in Scala but skipped
// for v1 — the cost is some missed call edges, not incorrect ones.
const SCALA_QUERY: &str = r#"
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

// ── Kotlin ───────────────────────────────────────────────────────────────
// tree-sitter-kotlin-ng (the actively-maintained fork) collapses
// classes, interfaces, and enums into a single `class_declaration` node
// distinguished only by an unnamed `class`/`interface`/`enum` keyword
// token — so one capture handles all three. Singletons (`object Foo`)
// live under a separate `object_declaration` node and are captured the
// same way so methods inside them get the right parent.
//
// Call shapes:
//   foo()         → call_expression with an `identifier` child
//   obj.foo()     → call_expression with `navigation_expression`(receiver, name)
//   Type(...)     → call_expression with `identifier` (constructor invocation
//                   syntactically identical to a function call in Kotlin)
// The navigation_expression has positional `(expression, identifier)`
// children — first is the receiver, second is the method name.
//
// Imports: `import a.b.c`, `import a.b.c.*`, and `import a.b.c as d`. The
// wildcard form has no separate marker — the trailing `.*` is unnamed
// tokens. Aliased form adds a trailing `identifier`.
const KOTLIN_QUERY: &str = r#"
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
