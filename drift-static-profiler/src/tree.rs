#![allow(clippy::needless_borrow)]
use crate::categories::Category;
use crate::graph::{CallGraph, ExternalCall, SymbolId};
use crate::insights::{self, Finding};
use crate::SymbolKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerRef {
    pub id: SymbolId,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub parent_class: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallTreeNode {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: usize,
    pub depth: usize,
    pub parent_class: Option<String>,
    pub children: Vec<CallTreeNode>,
    pub truncated_reason: Option<String>,

    // --- profiler-style annotations ---
    pub callers: Vec<CallerRef>,
    pub callers_count: usize,
    pub callees_count: usize,
    pub subtree_size: usize,

    pub category_self: Option<Category>,
    pub categories_reached: BTreeMap<String, usize>,
    pub external_calls: Vec<ExternalCall>,

    // ── Phase A: code-quality metrics ──
    pub complexity: usize,
    pub loc: usize,
    pub nesting_depth: usize,
    pub parameter_count: usize,
    pub is_async: bool,

    // ── Phase B: graph-derived ──
    pub call_site_count: usize,
    pub is_recursive: bool,
    pub pagerank: f64,

    // ── Phase C: tree-derived percentages ──
    pub percent_total: f64,
    pub percent_parent: f64,

    // ── Phase D: risk flags ──
    pub n_plus_one_risk: bool,
    pub blocking_in_async: bool,

    // ── Phase E: structured findings ──
    // Empty in step 1+2; populated by detectors in subsequent steps.
    // The Phase D booleans above stay populated as derived convenience
    // values computed from this list so older consumers keep working.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

pub struct TreeBuilder<'a> {
    pub graph: &'a CallGraph,
    pub root_dir: &'a Path,
    pub max_depth: usize,
    pub skip_accessors: bool,
}

impl<'a> TreeBuilder<'a> {
    pub fn new(graph: &'a CallGraph, root_dir: &'a Path) -> Self {
        Self {
            graph,
            root_dir,
            max_depth: 12,
            skip_accessors: false,
        }
    }

    fn is_accessor(name: &str) -> bool {
        if name.len() < 4 {
            return false;
        }
        let suffix = if let Some(s) = name.strip_prefix("get").or_else(|| name.strip_prefix("set")) {
            s
        } else if let Some(s) = name.strip_prefix("is") {
            s
        } else {
            return false;
        };
        suffix
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false)
    }

    pub fn build(&self, entry: &SymbolId) -> Option<CallTreeNode> {
        let mut seen: HashSet<SymbolId> = HashSet::new();
        let mut node = self.build_inner(entry, 0, &mut seen)?;
        // Phase C: compute percent_total (vs. the entry's own subtree size)
        // and percent_parent (vs. parent's subtree size).
        let total = node.subtree_size as f64;
        compute_percentages(&mut node, total, total);
        Some(node)
    }

    fn build_inner(
        &self,
        id: &SymbolId,
        depth: usize,
        seen: &mut HashSet<SymbolId>,
    ) -> Option<CallTreeNode> {
        let sym = self.graph.symbols.get(id)?;
        let is_cycle = seen.contains(id);
        seen.insert(id.clone());

        let file = sym
            .file
            .strip_prefix(self.root_dir)
            .unwrap_or(&sym.file)
            .display()
            .to_string();

        let externals = self.graph.externals_of(id).to_vec();
        let category_self = pick_self_category(&externals);
        // Phase E: collect structured findings via the insights module.
        // The legacy Phase D booleans (`n_plus_one_risk`, `blocking_in_async`)
        // are now DERIVED from these findings so older consumers and the
        // flame-mode 'smells' painter keep working unchanged.
        let ctx = insights::Ctx::default();
        let findings = insights::collect_node_findings(sym, &externals, &ctx);
        let n_plus_one_risk = insights::has_kind(&findings, insights::FindingKind::NPlusOne);
        let blocking_in_async = insights::has_kind(&findings, insights::FindingKind::BlockingInAsync);

        let callers = self
            .graph
            .callers_of(id)
            .iter()
            .filter_map(|cid| {
                let s = self.graph.symbols.get(cid)?;
                Some(CallerRef {
                    id: cid.clone(),
                    name: s.name.clone(),
                    file: s
                        .file
                        .strip_prefix(self.root_dir)
                        .unwrap_or(&s.file)
                        .display()
                        .to_string(),
                    line: s.line,
                    parent_class: s.parent.clone(),
                })
            })
            .collect::<Vec<_>>();

        let callees_count = self.graph.callees(id).len();
        let callers_count = callers.len();

        let call_site_count = self
            .graph
            .call_site_count
            .get(id)
            .copied()
            .unwrap_or(0);
        let is_recursive = self.graph.is_recursive.get(id).copied().unwrap_or(false);
        let pagerank = self.graph.pagerank.get(id).copied().unwrap_or(0.0);

        let mut node = CallTreeNode {
            id: id.clone(),
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            file,
            line: sym.line,
            depth,
            parent_class: sym.parent.clone(),
            children: Vec::new(),
            truncated_reason: None,
            callers,
            callers_count,
            callees_count,
            subtree_size: 1,
            category_self,
            categories_reached: BTreeMap::new(),
            external_calls: externals,
            complexity: sym.complexity,
            loc: sym.loc,
            nesting_depth: sym.nesting_depth,
            parameter_count: sym.parameter_count,
            is_async: sym.is_async,
            call_site_count,
            is_recursive,
            pagerank,
            percent_total: 0.0,
            percent_parent: 0.0,
            n_plus_one_risk,
            blocking_in_async,
            findings,
        };

        if is_cycle {
            node.truncated_reason = Some("cycle".into());
            tally_self(&mut node);
            return Some(node);
        }
        if depth >= self.max_depth {
            node.truncated_reason = Some("max-depth".into());
            tally_self(&mut node);
            return Some(node);
        }

        for callee in self.graph.callees(id) {
            if self.skip_accessors {
                if let Some(target) = self.graph.symbols.get(callee) {
                    if Self::is_accessor(&target.name) {
                        continue;
                    }
                }
            }
            if let Some(child) = self.build_inner(callee, depth + 1, seen) {
                node.children.push(child);
            }
        }

        // Aggregate subtree size and reached categories.
        let mut size = 1;
        let mut reached: BTreeMap<String, usize> = BTreeMap::new();
        // Self category contributes.
        if let Some(cat) = node.category_self {
            *reached.entry(cat.as_str().to_string()).or_default() += 1;
        }
        // External calls (each one is a "leaf" event).
        for e in &node.external_calls {
            *reached.entry(e.category.as_str().to_string()).or_default() += 1;
        }
        for c in &node.children {
            size += c.subtree_size;
            for (k, v) in &c.categories_reached {
                *reached.entry(k.clone()).or_default() += v;
            }
        }
        node.subtree_size = size;
        node.categories_reached = reached;

        Some(node)
    }
}

fn compute_percentages(node: &mut CallTreeNode, total: f64, parent_size: f64) {
    let size = node.subtree_size as f64;
    node.percent_total = if total > 0.0 { (size / total) * 100.0 } else { 0.0 };
    node.percent_parent = if parent_size > 0.0 { (size / parent_size) * 100.0 } else { 0.0 };
    for c in node.children.iter_mut() {
        compute_percentages(c, total, size);
    }
}

fn tally_self(node: &mut CallTreeNode) {
    let mut reached: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(cat) = node.category_self {
        *reached.entry(cat.as_str().to_string()).or_default() += 1;
    }
    for e in &node.external_calls {
        *reached.entry(e.category.as_str().to_string()).or_default() += 1;
    }
    node.categories_reached = reached;
}

fn pick_self_category(externals: &[ExternalCall]) -> Option<Category> {
    // Highest-signal category wins: db > network > io > cache > queue > log.
    let priority = [
        Category::Db,
        Category::Network,
        Category::Io,
        Category::Cache,
        Category::Queue,
        Category::Log,
    ];
    for p in priority {
        if externals.iter().any(|e| e.category == p) {
            return Some(p);
        }
    }
    None
}

pub fn render_ascii(node: &CallTreeNode) -> String {
    let mut out = String::new();
    render_ascii_into(node, "", true, &mut out);
    out
}

fn render_ascii_into(node: &CallTreeNode, prefix: &str, is_last: bool, out: &mut String) {
    let connector = if node.depth == 0 {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    let kind = match node.kind {
        SymbolKind::Function => "fn",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
    };
    let parent = node
        .parent_class
        .as_ref()
        .map(|p| format!("{p}."))
        .unwrap_or_default();
    let trunc = node
        .truncated_reason
        .as_ref()
        .map(|r| format!(" [{r}]"))
        .unwrap_or_default();
    let cat = node
        .category_self
        .map(|c| format!(" [{}]", c.as_str()))
        .unwrap_or_default();
    let reaches = if !node.categories_reached.is_empty() {
        let mut parts: Vec<String> = node
            .categories_reached
            .iter()
            .filter(|(_, v)| **v > 0)
            .map(|(k, v)| format!("{k}:{v}"))
            .collect();
        parts.sort();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" → {{{}}}", parts.join(","))
        }
    } else {
        String::new()
    };
    out.push_str(prefix);
    out.push_str(connector);
    out.push_str(&format!(
        "{kind} {parent}{name}  ({file}:{line}){cat}{reaches}{trunc}\n",
        name = node.name,
        file = node.file,
        line = node.line,
    ));
    let new_prefix = if node.depth == 0 {
        String::new()
    } else if is_last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };
    let n = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        render_ascii_into(child, &new_prefix, i + 1 == n, out);
    }
}
