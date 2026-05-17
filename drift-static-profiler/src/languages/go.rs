//! Go — `tree-sitter-go` 0.25 binding + tags query.
//!
//! Go has no classes. Methods are top-level `func (recv T) M() {}`
//! declarations; the receiver type would naturally be the "parent"
//! for receiver-based dispatch, but containment-based parent
//! resolution can't find it (there's no enclosing node), so Go method
//! symbols come out with `parent=None`. The call graph still works
//! because resolution is by name.
//!
//! Call shapes:
//!   - `foo()`     → `call_expression(function: identifier)`
//!   - `pkg.Foo()` → `call_expression(function: selector_expression)`
//!   - `r.M()`     → `call_expression(function: selector_expression)`
//!
//! Both selector forms collapse to the same capture pattern.
//!
//! Imports: `import "fmt"` and `import f "fmt"`. The string path lives
//! in an `interpreted_string_literal` whose text includes the
//! surrounding quotes — `tags.rs` strips those before storing
//! `module_path`.
//!
//! SQL-sink coverage: database/sql `Query`/`QueryRow`/`Exec` + Context
//! variants, sqlx `Select`/`Get`/`NamedExec`/`NamedQuery`/`Queryx`,
//! pgx `Query`/`Exec`/`Batch`, GORM `Raw`/`Exec`. Both quoted
//! (`"…"`) and raw-string (`` `…` ``) forms captured — raw-strings
//! are the canonical Go multi-line SQL shape.

use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_go::LANGUAGE.into()
}

pub const TAGS_QUERY: &str = r#"
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

; SQL sinks: database/sql Query/QueryRow/Exec + Context variants,
; sqlx Select/Get/NamedExec/NamedQuery/Queryx, pgx Query/Exec/Batch,
; GORM Raw/Exec. Restrict to a known set to keep noise out.
; Both quoted forms ("...") and raw strings (`...`) are captured —
; the latter is the canonical Go multi-line SQL shape.
(call_expression
  function: (selector_expression
    operand: (_) @ref.receiver
    field: (field_identifier) @ref.name)
  (#match? @ref.name "^(Query|QueryRow|QueryContext|QueryRowContext|Exec|ExecContext|Select|Get|NamedExec|NamedQuery|Queryx|QueryxContext|QueryRowx|QueryRowxContext|Raw)$")
  arguments: (argument_list . (interpreted_string_literal) @ref.sql_literal)) @ref.call

(call_expression
  function: (selector_expression
    operand: (_) @ref.receiver
    field: (field_identifier) @ref.name)
  (#match? @ref.name "^(Query|QueryRow|QueryContext|QueryRowContext|Exec|ExecContext|Select|Get|NamedExec|NamedQuery|Queryx|QueryxContext|QueryRowx|QueryRowxContext|Raw)$")
  arguments: (argument_list . (raw_string_literal) @ref.sql_literal)) @ref.call
"#;
