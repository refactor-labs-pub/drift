use crate::Language;
use tree_sitter::Language as TsLanguage;

pub fn language_for(lang: Language) -> TsLanguage {
    match lang {
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
    }
}

pub fn tags_query(lang: Language) -> &'static str {
    match lang {
        Language::Python => PYTHON_QUERY,
        Language::Java => JAVA_QUERY,
        Language::TypeScript => TYPESCRIPT_QUERY,
        Language::JavaScript => JAVASCRIPT_QUERY,
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
