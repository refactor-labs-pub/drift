//! Backward-compat shim: the canonical home of per-language tree-sitter
//! bindings + tags queries is now [`crate::languages`]. This file
//! re-exports the dispatcher functions so existing call sites
//! (`crate::parser::language_for`, `crate::parser::tags_query`) keep
//! compiling unchanged.
//!
//! New code should depend on `crate::languages::*` directly.

pub use crate::languages::{language_for, tags_query};
