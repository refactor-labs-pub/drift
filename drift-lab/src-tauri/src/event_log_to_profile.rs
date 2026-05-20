//! Phase F4 — convert a dynamic `events.log` JSONL stream into a
//! `profile.schema.json`-shaped document with `mode: "sampled"`.
//!
//! The output is byte-equivalent to a profile the static profiler
//! emits with `mode: "static"`, but populated from runtime samples
//! instead of AST analysis. A viewer that loads either format renders
//! the same tree; loading BOTH for the same codebase lets it join on
//! `CallTreeNode.id` and surface combined facts ("this method has
//! complexity 22 AND 500 sample hits").
//!
//! Why not depend on `drift-static-profiler`'s Rust types?
//!
//! `drift-static-profiler` pulls in 9 tree-sitter parsers plus rayon,
//! petgraph, and friends — all needed for static analysis but useless
//! for a pure data-shape converter. The desktop app already depends
//! on `drift-static-profiler` for the analyzer pipeline, but adding a
//! second use site for converting events would force the same heavy
//! transitive set into the Tauri binary's hot path for `Profile`
//! deserialization. We define a focused set of local serde structs
//! here that produce EXACT `profile.schema.json` output. The JSON
//! contract is the source of truth; the Rust structs are an
//! implementation detail — anything that consumes the output goes
//! through the JSON schema, not these types.
//!
//! Output structure (field names mirror `profile.schema.json`):
//!
//! ```text
//! {
//!   "schema_version": "1.0",
//!   "mode": "sampled",
//!   "generator": { tool, version, host, captured_at, source_root, language_versions },
//!   "summary": { languages, files, symbols, edges, categories },
//!   "entries": [ CallTreeNode ]
//! }
//! ```
//!
//! See `drift-static-profiler/schema/profile.schema.json` for the
//! authoritative field documentation.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::event_log::{self, AggregateReport, TreeNode};

// ===========================================================================
// Output schema — focused mirror of `profile.schema.json`.
// ===========================================================================

/// Top-level document. Matches `profile.schema.json#/properties`.
#[derive(Serialize)]
struct Report {
    schema_version: String,
    mode: String,
    generator: Generator,
    summary: Summary,
    entries: Vec<CallTreeNode>,
}

/// Mirror of `profile.schema.json#/$defs/Generator`. Fields with
/// `skip_serializing_if = "Option::is_none"` stay absent from the
/// output when the dynamic profile didn't emit them — matches the
/// schema's "required: [tool, version]" with everything else optional.
#[derive(Serialize, Default)]
struct Generator {
    tool: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_root: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    language_versions: BTreeMap<String, String>,
}

/// Mirror of `profile.schema.json#/$defs/Summary` — only the required
/// scalars + categories. Static-only rollups (top_callers, hot_paths,
/// dead_code, findings_by_kind, …) are omitted because no detector ran;
/// the schema makes them optional. Empty optional arrays/maps stay
/// out of the output via the `skip_serializing_if` annotations.
#[derive(Serialize, Default)]
struct Summary {
    languages: Vec<String>,
    files: u32,
    symbols: u32,
    edges: u32,
    categories: BTreeMap<String, u32>,
}

/// Mirror of `profile.schema.json#/$defs/CallTreeNode`. Populated
/// with the **runtime-only** fields (`self_value`, `total_value`,
/// `sample_count`) per the schema's docstring at those fields. The
/// static-only fields (complexity, loc, findings, …) are omitted
/// because nothing could have produced them — the schema makes those
/// optional too. A viewer that loads both a static and a sampled
/// profile and merges on `id` produces the full union.
#[derive(Serialize)]
struct CallTreeNode {
    /// Stable identifier in `file::class::name` format. Matches the
    /// static profiler's CallTreeNode.id verbatim — the join key.
    id: String,
    name: String,
    /// `Function` | `Method` | `Class` | `Lambda` | `Native`. We can't
    /// distinguish at runtime without static analysis; default to
    /// `Function` and let the viewer's join with a static profile
    /// override when one's loaded.
    kind: String,
    file: String,
    line: u32,
    depth: u32,
    children: Vec<CallTreeNode>,

    // Runtime-mode-only fields — the meat of `mode: "sampled"`.

    /// Inclusive cost of this node + subtree, in nanoseconds. The
    /// schema's `total_value` field. Derived from `TreeNode.value` (μs)
    /// × 1000.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_value: Option<u64>,
    /// Exclusive cost at this node only, in nanoseconds. Derived from
    /// `TreeNode.self_value` (μs) × 1000.
    #[serde(skip_serializing_if = "Option::is_none")]
    self_value: Option<u64>,
    /// Number of sample hits where this frame appeared. Derived from
    /// `TreeNode.ncalls`.
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_count: Option<u32>,
    /// Reachable subtree node count — useful for "subtree share"
    /// rollups. Required field in the schema.
    subtree_size: u32,

    // ── F1b/F3 join keys forwarded onto the static-schema's optional
    //    Frame-equivalent fields on CallTreeNode. ────────────────────

    /// Fully-qualified name (e.g. `OrderService.create`). Forwarded from
    /// `TreeNode.qualname` (F1b). Absent on Python 3.7-3.10.
    #[serde(skip_serializing_if = "Option::is_none")]
    qualified_name: Option<String>,
    /// Containing module. Forwarded from `TreeNode.module` (F1b).
    #[serde(skip_serializing_if = "Option::is_none")]
    module: Option<String>,
    /// Stdlib / runtime / profiler-self flag. Forwarded from
    /// `TreeNode.is_system` (F1a).
    #[serde(skip_serializing_if = "Option::is_none")]
    is_system: Option<bool>,
    /// `parent_class` if recoverable from `qualified_name`. Same parse
    /// `make_node_id` uses in `event_log.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_class: Option<String>,
}

// ===========================================================================
// Public entry point
// ===========================================================================

/// Read the JSONL file at `path`, harvest the F2 `profile_metadata`
/// header (if present), aggregate the rest via the existing pipeline,
/// and produce a `profile.schema.json`-shaped `Report` with
/// `mode = "sampled"`. Returns the serialized JSON string.
///
/// Errors:
/// - file open / read failure → wrapped via anyhow context
/// - JSON serialization failure → bubbled
/// - **No** error on a missing `profile_metadata` header — falls back
///   to a default Generator with empty `source_root` and just the
///   tool name. The output is still a valid profile, just without
///   provenance details. A consumer that needs them must re-run with
///   a profiler that emits F2.
pub fn convert(path: &Path) -> Result<String> {
    let metadata = read_profile_metadata(path)?;
    let aggregate = event_log::aggregate(path)
        .with_context(|| format!("aggregate {}", path.display()))?;
    let report = build_report(metadata, aggregate);
    serde_json::to_string_pretty(&report).context("serialize Report")
}

// ===========================================================================
// Pipeline
// ===========================================================================

/// Scan the JSONL file for the F2 `profile_metadata` header. Returns
/// `Some(value)` if found, `None` otherwise. Stops at the first
/// non-metadata event — the header is by contract the first line of
/// the session, so we don't waste cycles parsing the whole file.
fn read_profile_metadata(path: &Path) -> Result<Option<Value>> {
    let file = fs::File::open(path)
        .map_err(|e| anyhow!("open {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let ev: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match ev.get("type").and_then(Value::as_str) {
            Some("profile_metadata") => return Ok(Some(ev)),
            // First non-empty parseable line wasn't metadata — by the
            // F2 contract there isn't one in this file. Stop.
            Some(_) => return Ok(None),
            None => continue,
        }
    }
    Ok(None)
}

fn build_report(metadata: Option<Value>, agg: AggregateReport) -> Report {
    let generator = build_generator(metadata.as_ref());
    let entries = build_entries(&agg.tree);
    let summary = build_summary(&entries);
    Report {
        schema_version: "1.0".into(),
        mode: "sampled".into(),
        generator,
        summary,
        entries,
    }
}

fn build_generator(metadata: Option<&Value>) -> Generator {
    let Some(meta) = metadata else {
        return Generator {
            tool: "driftdockerprofiler".into(),
            ..Default::default()
        };
    };
    let gen_obj = meta.get("generator").and_then(Value::as_object);
    let str_field = |key: &str| -> Option<String> {
        gen_obj
            .and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .map(String::from)
    };
    let language_versions: BTreeMap<String, String> = gen_obj
        .and_then(|m| m.get("language_versions"))
        .and_then(Value::as_object)
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Generator {
        tool: str_field("tool").unwrap_or_else(|| "driftdockerprofiler".into()),
        version: str_field("version").unwrap_or_default(),
        host: str_field("host"),
        captured_at: str_field("captured_at"),
        source_root: str_field("source_root"),
        language_versions,
    }
}

/// Convert the existing snakeviz `TreeNode` into static-schema
/// `CallTreeNode` entries. The aggregator's synthetic `<root>` node
/// is skipped — its children become the top-level `entries[]`, which
/// matches the static schema's "one tree per entry point" shape.
fn build_entries(tree: &TreeNode) -> Vec<CallTreeNode> {
    if tree.name == "<root>" {
        tree.children
            .iter()
            .map(|c| convert_node(c, 0))
            .collect()
    } else {
        vec![convert_node(tree, 0)]
    }
}

fn convert_node(node: &TreeNode, depth: u32) -> CallTreeNode {
    let children: Vec<CallTreeNode> = node
        .children
        .iter()
        .map(|c| convert_node(c, depth + 1))
        .collect();
    let subtree_size = 1 + children.iter().map(|c| c.subtree_size).sum::<u32>();
    let parent_class = parse_parent_class(node.qualname.as_deref());
    // TreeNode times are in μs; static schema's value_ns is ns.
    let value_ns: u64 = (node.value.max(0) as u64).saturating_mul(1_000);
    let self_ns: u64 = (node.self_value.max(0) as u64).saturating_mul(1_000);
    CallTreeNode {
        id: node.node_id.clone(),
        name: node.name.clone(),
        kind: classify_kind(node),
        file: node.file.clone().unwrap_or_default(),
        line: node.line.unwrap_or(0),
        depth,
        children,
        total_value: Some(value_ns),
        self_value: Some(self_ns),
        sample_count: Some(node.ncalls),
        subtree_size,
        qualified_name: node.qualname.clone(),
        module: node.module.clone(),
        is_system: node.is_system,
        parent_class,
    }
}

/// Classify the node kind for the schema's `kind` enum. Without
/// static analysis we can't reliably distinguish Function vs Method;
/// we use a simple rule:
///   - if `qualified_name` contains a `.` (and not `<locals>`),
///     the leaf-after-`.` is on a class → Method
///   - otherwise → Function
/// A viewer that joins with a static profile will override with the
/// authoritative `kind`.
fn classify_kind(node: &TreeNode) -> String {
    let Some(qn) = node.qualname.as_deref() else {
        return "Function".into();
    };
    if !qn.contains('.') || qn.contains("<locals>") {
        return "Function".into();
    }
    "Method".into()
}

/// Extract the class portion of a qualified name. Matches the parsing
/// `make_node_id` in `event_log.rs` already does — same rule, same
/// edge cases. Returns `None` for free functions / closures.
fn parse_parent_class(qualified_name: Option<&str>) -> Option<String> {
    let qn = qualified_name?;
    if qn.is_empty() || qn.contains("<locals>") {
        return None;
    }
    let idx = qn.rfind('.')?;
    let class = &qn[..idx];
    if class.is_empty() {
        return None;
    }
    Some(class.to_string())
}

fn build_summary(entries: &[CallTreeNode]) -> Summary {
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut symbols: u32 = 0;
    walk_count(entries, &mut files, &mut symbols);
    Summary {
        languages: vec!["python".into()],
        files: files.len() as u32,
        symbols,
        edges: 0, // not derivable from samples alone
        categories: BTreeMap::new(),
    }
}

fn walk_count(nodes: &[CallTreeNode], files: &mut BTreeSet<String>, symbols: &mut u32) {
    for n in nodes {
        if !n.file.is_empty() {
            files.insert(n.file.clone());
        }
        *symbols += 1;
        walk_count(&n.children, files, symbols);
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn convert_emits_static_schema_shape() {
        // One metadata header + one wall_trace event with the F1b
        // metadata populated. The converter should produce a Report
        // with mode=sampled, the Generator copied from the header,
        // and one CallTreeNode under entries[].
        let header = r#"{"type":"profile_metadata","time":"2026-05-19T12:00:00.000000Z","service":"svc","pod":"p","mode":"sampled","schema_version":"1.0","service_id":"abc123","generator":{"tool":"driftdockerprofiler","version":"4.2.0","host":"laptop","captured_at":"2026-05-19T12:00:00.000000Z","source_root":"/app","language_versions":{"python":"3.11.7"}}}"#;
        let evt = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.100000Z","service":"svc","pod":"p","period_ns":10000000,"duration_ns":1000000000,"count":7,"cpu":0.5,"memory_bytes":1024,"frames":[{"name":"create","file":"/app/orders.py","line":23,"qualified_name":"OrderService.create","module":"orders.service","is_system":false}]}"#;
        let f = write_fixture(&[header, evt]);

        let json = convert(f.path()).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();

        // Top-level invariants — these are the contract.
        assert_eq!(v["schema_version"], "1.0");
        assert_eq!(v["mode"], "sampled");
        assert_eq!(v["generator"]["tool"], "driftdockerprofiler");
        assert_eq!(v["generator"]["version"], "4.2.0");
        assert_eq!(v["generator"]["source_root"], "/app");
        assert_eq!(v["generator"]["language_versions"]["python"], "3.11.7");
        assert_eq!(v["summary"]["languages"][0], "python");
        assert_eq!(v["summary"]["files"], 1);

        // One entry, with the F3 node_id and F1b join keys forwarded.
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let leaf = &entries[0];
        assert_eq!(leaf["id"], "/app/orders.py::OrderService::create");
        assert_eq!(leaf["name"], "create");
        assert_eq!(leaf["kind"], "Method");
        assert_eq!(leaf["file"], "/app/orders.py");
        assert_eq!(leaf["line"], 23);
        assert_eq!(leaf["qualified_name"], "OrderService.create");
        assert_eq!(leaf["module"], "orders.service");
        assert_eq!(leaf["is_system"], false);
        assert_eq!(leaf["parent_class"], "OrderService");
        // 7 ticks × 10 ms period = 70 ms = 70_000 μs = 70_000_000 ns.
        // `total_value` carries the full tick-weighted duration.
        assert_eq!(leaf["total_value"], 70_000_000);
        // `sample_count` reflects the aggregator's unique-stack count
        // (`TreeNode.ncalls`), not raw tick count — tick weight already
        // lives in `total_value`. One event with count=7 produces ONE
        // unique stack, so sample_count=1. A consumer that wants raw
        // ticks divides `total_value` by `period_ns` from the header.
        assert_eq!(leaf["sample_count"], 1);
    }

    #[test]
    fn convert_handles_missing_metadata_header() {
        // A pre-F2 events.log has no profile_metadata line. The
        // converter must still produce a valid Report — just without
        // generator details beyond the tool name.
        let evt = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.100000Z","service":"svc","pod":"p","period_ns":10000000,"duration_ns":1000000000,"count":1,"cpu":0.5,"memory_bytes":1024,"frames":[{"name":"f","file":"/a.py","line":1}]}"#;
        let f = write_fixture(&[evt]);

        let json = convert(f.path()).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["mode"], "sampled");
        assert_eq!(v["generator"]["tool"], "driftdockerprofiler");
        // No source_root in absence of header.
        assert!(v["generator"].get("source_root").is_none());
        // Output still has the tree from aggregation.
        assert!(v["entries"].as_array().unwrap().len() >= 1);
    }

    #[test]
    fn convert_classifies_free_function_kind() {
        // A frame without qualified_name (Py 3.7-3.10) defaults to
        // kind=Function. Closures (`outer.<locals>.inner`) also stay
        // Function — matches `make_node_id`'s closure-as-free-function
        // rule.
        let evt1 = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.000000Z","service":"s","pod":"p","period_ns":10000000,"duration_ns":1000000000,"count":1,"cpu":0.0,"memory_bytes":1,"frames":[{"name":"top","file":"/a.py","line":1}]}"#;
        let evt2 = r#"{"type":"wall_trace","time":"2026-05-19T12:00:00.000000Z","service":"s","pod":"p","period_ns":10000000,"duration_ns":1000000000,"count":1,"cpu":0.0,"memory_bytes":1,"frames":[{"name":"inner","file":"/a.py","line":1,"qualified_name":"outer.<locals>.inner"}]}"#;
        let f = write_fixture(&[evt1, evt2]);
        let json = convert(f.path()).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        for entry in v["entries"].as_array().unwrap() {
            assert_eq!(entry["kind"], "Function", "got {:?}", entry);
        }
    }

    #[test]
    fn convert_omits_runtime_fields_when_zero_safely() {
        // Empty file → empty entries[], summary counts zero. The
        // converter must not panic on emptiness.
        let f = write_fixture(&[]);
        let json = convert(f.path()).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["mode"], "sampled");
        assert_eq!(v["entries"].as_array().unwrap().len(), 0);
        assert_eq!(v["summary"]["files"], 0);
        assert_eq!(v["summary"]["symbols"], 0);
    }

    #[test]
    fn parent_class_parsed_from_qualified_name() {
        assert_eq!(
            parse_parent_class(Some("OrderService.create")),
            Some("OrderService".into())
        );
        assert_eq!(parse_parent_class(Some("outer.<locals>.inner")), None);
        assert_eq!(parse_parent_class(Some("top_level")), None);
        assert_eq!(parse_parent_class(Some("")), None);
        assert_eq!(parse_parent_class(None), None);
    }
}
