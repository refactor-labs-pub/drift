use crate::categories::Category;
use crate::graph::CallGraph;
use crate::linguist::{LanguageBreakdownEntry, LanguageStats};
use crate::tree::CallTreeNode;
use crate::{FileTags, Language};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotPath {
    pub frames: Vec<String>,
    pub depth: usize,
    pub terminal_category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub languages: Vec<String>,
    pub files: usize,
    pub symbols: usize,
    pub edges: usize,
    pub categories: BTreeMap<String, usize>,
    pub top_callers: Vec<TopSymbol>,
    pub top_callees: Vec<TopSymbol>,
    pub hot_paths: Vec<HotPath>,
    // ── Phase B graph-derived rollups ──
    pub dead_code: Vec<TopSymbol>,
    pub pagerank_top: Vec<RankedByScore>,
    pub recursive_symbols: Vec<TopSymbol>,
    // ── Linguist-style language breakdown ──
    /// Per-programming-language byte share of the whole repo (filtered by
    /// the same .gitignore rules used for source discovery). Sorted desc by
    /// bytes. Mirrors GitHub's repo-page language bar.
    pub language_breakdown: Vec<LanguageBreakdownEntry>,
    /// The supported language drift actually profiled — i.e. the
    /// highest-byte language in `language_breakdown` that has a shipped
    /// tree-sitter parser. `None` when no supported language was detected.
    pub profiled_language: Option<String>,
    /// Share of total programming bytes accounted for by `profiled_language`.
    pub profiled_language_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedByScore {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub parent_class: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopSymbol {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub parent_class: Option<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generator {
    pub tool: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: String,
    pub mode: String,
    pub generator: Generator,
    pub summary: Summary,
    pub entries: Vec<CallTreeNode>,
}

impl Report {
    pub fn build(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: Vec<CallTreeNode>,
        language_stats: &LanguageStats,
        source_root: Option<&Path>,
    ) -> Self {
        let summary = Summary::build(all_tags, graph, &entries, language_stats);
        Self {
            schema_version: "1.0".into(),
            mode: "static".into(),
            generator: Generator {
                tool: "drift-static-profiler".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                source_root: source_root.map(|p| p.display().to_string()),
            },
            summary,
            entries,
        }
    }
}

impl Summary {
    pub fn build(
        all_tags: &[FileTags],
        graph: &CallGraph,
        entries: &[CallTreeNode],
        language_stats: &LanguageStats,
    ) -> Self {
        let languages: Vec<String> = {
            let mut s: HashSet<&'static str> = HashSet::new();
            for ft in all_tags {
                s.insert(match ft.language {
                    Language::Python => "python",
                    Language::Java => "java",
                    Language::TypeScript => "typescript",
                    Language::JavaScript => "javascript",
                    Language::Go => "go",
                    Language::Rust => "rust",
                    Language::Scala => "scala",
                });
            }
            let mut v: Vec<String> = s.into_iter().map(|x| x.to_string()).collect();
            v.sort();
            v
        };

        // Categories aggregate across all entries
        let mut categories: BTreeMap<String, usize> = BTreeMap::new();
        for e in entries {
            for (k, v) in &e.categories_reached {
                *categories.entry(k.clone()).or_default() += *v;
            }
        }
        // Ensure every category is represented (zero-valued) for stable UI
        for c in Category::ALL {
            categories.entry(c.as_str().to_string()).or_insert(0);
        }

        // Top callers (most-called symbols across the project)
        let mut callers_rank: Vec<(String, &crate::graph::SymbolId, usize)> = graph
            .callers
            .iter()
            .filter_map(|(id, list)| {
                let sym = graph.symbols.get(id)?;
                Some((sym.name.clone(), id, list.len()))
            })
            .collect();
        callers_rank.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.cmp(&b.0))
                .then_with(|| a.1.0.cmp(&b.1.0))
        });
        let top_callers: Vec<TopSymbol> = callers_rank
            .into_iter()
            .filter(|(_, _, c)| *c > 0)
            .take(10)
            .filter_map(|(name, id, count)| {
                let sym = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name,
                    file: sym.file.display().to_string(),
                    line: sym.line,
                    parent_class: sym.parent.clone(),
                    count,
                })
            })
            .collect();

        // Top callees (symbols with the most fan-out)
        let mut callees_rank: Vec<(String, &crate::graph::SymbolId, usize)> = graph
            .edges
            .iter()
            .filter_map(|(id, list)| {
                let sym = graph.symbols.get(id)?;
                Some((sym.name.clone(), id, list.len()))
            })
            .collect();
        callees_rank.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.cmp(&b.0))
                .then_with(|| a.1.0.cmp(&b.1.0))
        });
        let top_callees: Vec<TopSymbol> = callees_rank
            .into_iter()
            .filter(|(_, _, c)| *c > 0)
            .take(10)
            .filter_map(|(name, id, count)| {
                let sym = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name,
                    file: sym.file.display().to_string(),
                    line: sym.line,
                    parent_class: sym.parent.clone(),
                    count,
                })
            })
            .collect();

        // Hot paths: walk each entry, collect chains ending at nodes with a
        // category_self or external_calls, keep the longest few.
        let mut hot_paths: Vec<HotPath> = Vec::new();
        for e in entries {
            collect_hot_paths(e, &mut Vec::new(), &mut hot_paths);
        }
        hot_paths.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then_with(|| a.terminal_category.cmp(&b.terminal_category))
                .then_with(|| a.frames.cmp(&b.frames))
        });
        hot_paths.truncate(10);

        let edges_count: usize = graph.edges.values().map(|v| v.len()).sum();

        // ── Phase B rollups ──

        // Entry-point IDs (the user-pinned roots) — these are NOT dead even
        // if no caller exists in source (HTTP handlers, main, etc.)
        let entry_ids: std::collections::HashSet<&crate::graph::SymbolId> =
            entries.iter().map(|e| &e.id).collect();

        let mut dead_code: Vec<TopSymbol> = graph
            .callers
            .iter()
            .filter(|(id, callers)| callers.is_empty() && !entry_ids.contains(id))
            .filter_map(|(id, _)| {
                let s = graph.symbols.get(id)?;
                // Classes are often "instantiated" rather than called by name; skip
                // them in dead-code reporting to reduce noise.
                if matches!(s.kind, crate::SymbolKind::Class) {
                    return None;
                }
                Some(TopSymbol {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    count: 0,
                })
            })
            .collect();
        dead_code.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        let mut pagerank_pairs: Vec<(&crate::graph::SymbolId, f64)> =
            graph.pagerank.iter().map(|(id, r)| (id, *r)).collect();
        pagerank_pairs.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.0.cmp(&b.0.0))
        });
        let pagerank_top: Vec<RankedByScore> = pagerank_pairs
            .into_iter()
            .take(10)
            .filter_map(|(id, score)| {
                let s = graph.symbols.get(id)?;
                Some(RankedByScore {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    score,
                })
            })
            .collect();

        let mut recursive_symbols: Vec<TopSymbol> = graph
            .is_recursive
            .iter()
            .filter(|(_, rec)| **rec)
            .filter_map(|(id, _)| {
                let s = graph.symbols.get(id)?;
                Some(TopSymbol {
                    name: s.name.clone(),
                    file: s.file.display().to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                    count: 1,
                })
            })
            .collect();
        recursive_symbols.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        Self {
            languages,
            files: all_tags.len(),
            symbols: graph.symbols.len(),
            edges: edges_count,
            categories,
            top_callers,
            top_callees,
            hot_paths,
            dead_code,
            pagerank_top,
            recursive_symbols,
            language_breakdown: language_stats.breakdown.clone(),
            profiled_language: language_stats.dominant_supported_name.clone(),
            profiled_language_percent: language_stats.dominant_supported_percent,
        }
    }
}

fn collect_hot_paths(
    node: &CallTreeNode,
    stack: &mut Vec<String>,
    out: &mut Vec<HotPath>,
) {
    let label = format!(
        "{}{}",
        node.parent_class
            .as_ref()
            .map(|p| format!("{p}."))
            .unwrap_or_default(),
        node.name
    );
    stack.push(label);

    let terminal_cat = node.category_self.map(|c| c.as_str().to_string()).or_else(|| {
        node.external_calls
            .first()
            .map(|e| e.category.as_str().to_string())
    });

    if let Some(cat) = terminal_cat {
        out.push(HotPath {
            frames: stack.clone(),
            depth: node.depth,
            terminal_category: cat,
        });
    }

    for c in &node.children {
        collect_hot_paths(c, stack, out);
    }
    stack.pop();
}

