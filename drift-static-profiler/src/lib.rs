pub mod api;
pub mod categories;
pub mod diff;
pub mod docker;
pub mod graph;
pub mod manifest;
pub mod insights;
pub mod linguist;
pub mod metrics;
pub mod pagerank;
pub mod parser;
pub mod progress;
pub mod report;
pub mod roots;
pub mod scans_index;
pub mod tags;
pub mod tree;
pub mod walker;

pub use api::{
    analyze, analyze_roots, analyze_roots_with_progress, analyze_with_progress, AnalyzeOptions,
    AnalyzeOutcome,
};
pub use progress::{CliProgress, NullProgress, Progress};
pub use linguist::{compute_language_stats, LanguageStats};
pub use roots::{discover_roots, DiscoverOpts, DiscoveredRoot};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Python,
    Java,
    TypeScript,
    JavaScript,
    Go,
    Rust,
    Scala,
    Kotlin,
}

impl Language {
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        match ext {
            "py" => Some(Self::Python),
            "java" => Some(Self::Java),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "go" => Some(Self::Go),
            "rs" => Some(Self::Rust),
            "scala" | "sc" => Some(Self::Scala),
            // `.kt` for Kotlin source, `.kts` for Kotlin scripts. We treat
            // both the same since the tree-sitter grammar parses script
            // files as well — there's no separate Kotlin-script grammar.
            "kt" | "kts" => Some(Self::Kotlin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file: PathBuf,
    pub line: usize,
    pub line_end: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub parent: Option<String>,
    // ── per-symbol metrics (Phase A) ──
    pub loc: usize,
    pub complexity: usize,
    pub nesting_depth: usize,
    pub parameter_count: usize,
    pub is_async: bool,
    // ── Phase D inputs: byte ranges of loops / awaits inside this symbol ──
    pub loop_ranges: Vec<(usize, usize)>,
    pub await_ranges: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub receiver: Option<String>,
    pub file: PathBuf,
    pub line: usize,
    pub byte_offset: usize,
    pub in_symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecord {
    /// Local binding name (what the file uses to reference this import)
    pub local_name: String,
    /// Fully-qualified module path
    pub module_path: String,
    /// What was imported from the module (None = wildcard / module import)
    pub imported_name: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    /// Local name in scope (e.g. variable, field, parameter)
    pub name: String,
    /// Source type name as written (e.g. "Repository", "Session", "OrderRepository")
    pub type_name: String,
    /// If non-empty, the class's superclasses (for class-extends inheritance)
    pub extends: Vec<String>,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTags {
    pub file: PathBuf,
    pub language: Language,
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    pub imports: Vec<ImportRecord>,
    pub bindings: Vec<Binding>,
}
