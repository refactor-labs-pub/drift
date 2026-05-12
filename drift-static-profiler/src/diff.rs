use crate::report::Report;
use crate::tree::CallTreeNode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    pub schema_version: String,
    pub baseline_generator: String,
    pub current_generator: String,
    pub entry_diffs: Vec<EntryDiff>,
    pub added_symbols: Vec<String>,
    pub removed_symbols: Vec<String>,
    pub regressions: Vec<Issue>,
    pub improvements: Vec<Issue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryDiff {
    pub entry_id: String,
    pub entry_name: String,
    pub category_deltas: BTreeMap<String, i64>,
    pub subtree_size_delta: i64,
    pub complexity_delta_total: i64,
    pub new_smells: Vec<String>,
    pub fixed_smells: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub kind: String,
    pub entry: String,
    pub delta: i64,
    pub message: String,
}

pub fn diff(baseline: &Report, current: &Report) -> DiffReport {
    // Match entries by (parent_class, name) — robust to file moves.
    let key = |n: &CallTreeNode| {
        format!(
            "{}::{}",
            n.parent_class.clone().unwrap_or_default(),
            n.name
        )
    };

    let baseline_entries: HashMap<String, &CallTreeNode> =
        baseline.entries.iter().map(|n| (key(n), n)).collect();
    let current_entries: HashMap<String, &CallTreeNode> =
        current.entries.iter().map(|n| (key(n), n)).collect();

    let mut entry_diffs = Vec::new();
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();

    for (k, cur) in &current_entries {
        let Some(base) = baseline_entries.get(k) else {
            // New entry — record but don't classify as regression here.
            continue;
        };

        let category_deltas = category_deltas(base, cur);
        let subtree_size_delta = cur.subtree_size as i64 - base.subtree_size as i64;
        let complexity_delta_total = sum_complexity(cur) as i64 - sum_complexity(base) as i64;
        let new_smells = new_smells_in_subtree(base, cur);
        let fixed_smells = new_smells_in_subtree(cur, base);

        // Classify deltas into regressions vs improvements
        for (cat, d) in &category_deltas {
            if *d > 0 {
                regressions.push(Issue {
                    kind: format!("category_{}", cat),
                    entry: k.clone(),
                    delta: *d,
                    message: format!("{} {} call(s) reachable from {}", d, cat, cur.name),
                });
            } else if *d < 0 {
                improvements.push(Issue {
                    kind: format!("category_{}", cat),
                    entry: k.clone(),
                    delta: *d,
                    message: format!("{} fewer {} call(s) reachable from {}", -d, cat, cur.name),
                });
            }
        }
        for smell in &new_smells {
            regressions.push(Issue {
                kind: "smell".into(),
                entry: k.clone(),
                delta: 1,
                message: format!("new {} in {}", smell, cur.name),
            });
        }
        for smell in &fixed_smells {
            improvements.push(Issue {
                kind: "smell".into(),
                entry: k.clone(),
                delta: -1,
                message: format!("resolved {} in {}", smell, cur.name),
            });
        }
        if complexity_delta_total > 0 {
            regressions.push(Issue {
                kind: "complexity".into(),
                entry: k.clone(),
                delta: complexity_delta_total,
                message: format!(
                    "subtree complexity +{} in {}",
                    complexity_delta_total, cur.name
                ),
            });
        }

        entry_diffs.push(EntryDiff {
            entry_id: cur.id.0.clone(),
            entry_name: cur.name.clone(),
            category_deltas,
            subtree_size_delta,
            complexity_delta_total,
            new_smells,
            fixed_smells,
        });
    }

    // Symbol-level add/remove (across all entries in both reports)
    let base_syms = collect_symbol_ids(baseline);
    let cur_syms = collect_symbol_ids(current);
    let mut added_symbols: Vec<String> = cur_syms.difference(&base_syms).cloned().collect();
    let mut removed_symbols: Vec<String> = base_syms.difference(&cur_syms).cloned().collect();
    added_symbols.sort();
    removed_symbols.sort();

    DiffReport {
        schema_version: "1.0".into(),
        baseline_generator: format!(
            "{} {}",
            baseline.generator.tool, baseline.generator.version
        ),
        current_generator: format!(
            "{} {}",
            current.generator.tool, current.generator.version
        ),
        entry_diffs,
        added_symbols,
        removed_symbols,
        regressions,
        improvements,
    }
}

fn category_deltas(base: &CallTreeNode, cur: &CallTreeNode) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    let keys: HashSet<&String> = base
        .categories_reached
        .keys()
        .chain(cur.categories_reached.keys())
        .collect();
    for k in keys {
        let b = base.categories_reached.get(k).copied().unwrap_or(0) as i64;
        let c = cur.categories_reached.get(k).copied().unwrap_or(0) as i64;
        let delta = c - b;
        if delta != 0 {
            out.insert(k.clone(), delta);
        }
    }
    out
}

fn sum_complexity(node: &CallTreeNode) -> usize {
    let mut sum = node.complexity;
    for c in &node.children {
        sum += sum_complexity(c);
    }
    sum
}

fn new_smells_in_subtree(base: &CallTreeNode, cur: &CallTreeNode) -> Vec<String> {
    // Match nodes by id between base and cur, find smells set on cur but not on base.
    let mut base_smells: HashMap<String, HashSet<String>> = HashMap::new();
    collect_smells(base, &mut base_smells);
    let mut cur_smells: HashMap<String, HashSet<String>> = HashMap::new();
    collect_smells(cur, &mut cur_smells);

    let mut out = Vec::new();
    for (id, smells) in &cur_smells {
        let base_set = base_smells.get(id).cloned().unwrap_or_default();
        for s in smells.difference(&base_set) {
            out.push(format!("{} ({})", s, id_to_label(id)));
        }
    }
    out.sort();
    out
}

fn collect_smells(node: &CallTreeNode, out: &mut HashMap<String, HashSet<String>>) {
    let mut set = HashSet::new();
    if node.n_plus_one_risk {
        set.insert("n_plus_one_risk".to_string());
    }
    if node.blocking_in_async {
        set.insert("blocking_in_async".to_string());
    }
    if !set.is_empty() {
        out.insert(node.id.0.clone(), set);
    }
    for c in &node.children {
        collect_smells(c, out);
    }
}

fn id_to_label(id: &str) -> String {
    // Take just the last :: segment for readability
    id.rsplit("::").next().unwrap_or(id).to_string()
}

fn collect_symbol_ids(report: &Report) -> HashSet<String> {
    let mut out = HashSet::new();
    for e in &report.entries {
        collect_ids(e, &mut out);
    }
    out
}

fn collect_ids(node: &CallTreeNode, out: &mut HashSet<String>) {
    out.insert(node.id.0.clone());
    for c in &node.children {
        collect_ids(c, out);
    }
}

/// Render a human-readable diff for terminal output.
pub fn render(diff: &DiffReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(&mut s, "drift-static-profiler diff").ok();
    writeln!(&mut s, "  baseline: {}", diff.baseline_generator).ok();
    writeln!(&mut s, "  current : {}", diff.current_generator).ok();
    writeln!(&mut s).ok();

    if diff.entry_diffs.is_empty() {
        writeln!(&mut s, "  no matching entries between reports").ok();
    } else {
        for ed in &diff.entry_diffs {
            let parent = if ed.entry_id.contains("::") {
                ""
            } else {
                ""
            };
            let _ = parent;
            writeln!(&mut s, "ENTRY: {}", ed.entry_name).ok();
            if !ed.category_deltas.is_empty() {
                let parts: Vec<String> = ed
                    .category_deltas
                    .iter()
                    .map(|(k, d)| format!("{k}: {:+}", d))
                    .collect();
                writeln!(&mut s, "  category deltas: {{ {} }}", parts.join(", ")).ok();
            }
            if ed.subtree_size_delta != 0 {
                writeln!(&mut s, "  subtree size:    {:+}", ed.subtree_size_delta).ok();
            }
            if ed.complexity_delta_total != 0 {
                writeln!(
                    &mut s,
                    "  complexity sum:  {:+}",
                    ed.complexity_delta_total
                )
                .ok();
            }
            for sm in &ed.new_smells {
                writeln!(&mut s, "  NEW SMELL: {}", sm).ok();
            }
            for sm in &ed.fixed_smells {
                writeln!(&mut s, "  fixed: {}", sm).ok();
            }
            writeln!(&mut s).ok();
        }
    }

    writeln!(
        &mut s,
        "Summary: {} regression(s), {} improvement(s), {} added symbol(s), {} removed",
        diff.regressions.len(),
        diff.improvements.len(),
        diff.added_symbols.len(),
        diff.removed_symbols.len()
    )
    .ok();
    s
}
