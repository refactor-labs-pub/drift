//! Per-language tree-sitter bindings + tags-query strings.
//!
//! # Separation of concerns
//!
//! Each supported language owns its own module file (`python.rs`,
//! `java.rs`, …). Per-language concerns — the tree-sitter grammar
//! binding, the tags-query pattern, the per-language comments
//! explaining grammar quirks (Go's `interpreted_string_literal`,
//! Rust's `generic_function` for turbofish, Kotlin's
//! `navigation_expression`) — live next to the code they describe.
//!
//! # Design (Robert C. Martin / Effective Rust)
//!
//! - **Single Responsibility per module.** One file owns one
//!   language. Changing Python's SQL-sink pattern can't reach into
//!   Java by accident.
//! - **Open/Closed.** Adding a 9th language is: (1) `cargo add
//!   tree-sitter-<lang>`, (2) `src/languages/<lang>.rs`, (3) one
//!   line each in the two dispatchers below. No edits to existing
//!   language modules.
//! - **Interface Segregation.** Each module exposes a tiny two-symbol
//!   surface: `pub fn language() -> tree_sitter::Language` and `pub
//!   const TAGS_QUERY: &str`. Callers depend on the dispatcher, not
//!   the individual modules — Dependency Inversion.
//! - **High cohesion.** All Go-specific grammar idiosyncrasies live
//!   in [`go`]; all Rust ones in [`rust`]; etc.
//!
//! # Capture-name conventions (shared by every query)
//!
//!   @def.name / @def.function / @def.method / @def.class
//!   @ref.name             — method/function being called
//!   @ref.receiver         — the object/scope before the call (optional)
//!   @ref.call             — the whole call site (byte range / scope)
//!   @ref.sql_literal      — captured SQL string for known SQL-sink
//!                           calls (`cursor.execute`, `sqlx::query!`,
//!                           `db.Query`, etc.). `tags.rs` reads the
//!                           raw text including quotes and strips
//!                           them before stamping onto `Reference`.
//!                           Co-occurs with `@ref.name`/`@ref.call`;
//!                           matches dedup by byte-offset so a
//!                           single call site never produces two
//!                           References.
//!   @import.module        — module path string node
//!   @import.name          — imported identifier (None = whole-module)
//!   @import.alias         — local binding name when aliased

use crate::Language;
use tree_sitter::Language as TsLanguage;

pub mod go;
pub mod java;
pub mod javascript;
pub mod kotlin;
pub mod python;
pub mod rust;
pub mod scala;
pub mod typescript;

/// Map a `Language` enum value to its tree-sitter grammar binding.
/// The grammar is owned by each per-language module.
pub fn language_for(lang: Language) -> TsLanguage {
    match lang {
        Language::Python => python::language(),
        Language::Java => java::language(),
        Language::TypeScript => typescript::language(),
        Language::JavaScript => javascript::language(),
        Language::Go => go::language(),
        Language::Rust => rust::language(),
        Language::Scala => scala::language(),
        Language::Kotlin => kotlin::language(),
    }
}

/// Map a `Language` enum value to its tags-query string.
/// The query is owned by each per-language module.
pub fn tags_query(lang: Language) -> &'static str {
    match lang {
        Language::Python => python::TAGS_QUERY,
        Language::Java => java::TAGS_QUERY,
        Language::TypeScript => typescript::TAGS_QUERY,
        Language::JavaScript => javascript::TAGS_QUERY,
        Language::Go => go::TAGS_QUERY,
        Language::Rust => rust::TAGS_QUERY,
        Language::Scala => scala::TAGS_QUERY,
        Language::Kotlin => kotlin::TAGS_QUERY,
    }
}
