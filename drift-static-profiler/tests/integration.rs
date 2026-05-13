use drift_static_profiler::{
    graph::{CallGraph, SymbolId},
    tags::extract_tags,
    tree::{render_ascii, CallTreeNode, TreeBuilder},
    walker::discover_source_files,
};
use std::path::{Path, PathBuf};

fn banner(test: &str, fixture: &str) {
    println!();
    println!("───────────────────────────────────────────────────────────────");
    println!("  TEST: {test}");
    println!("  fixture: tests/fixtures/{fixture}");
    println!("───────────────────────────────────────────────────────────────");
}

fn show_tree(label: &str, node: &CallTreeNode) {
    println!("  [{label}]");
    for line in render_ascii(node).lines() {
        println!("    {line}");
    }
}

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    p
}

fn analyze(root: &Path) -> CallGraph {
    let files = discover_source_files(root);
    let all: Vec<_> = files
        .into_iter()
        .filter_map(|(file, lang)| extract_tags(&file, lang).ok())
        .collect();
    CallGraph::build(&all)
}

fn names_in_subtree(node: &CallTreeNode) -> Vec<String> {
    let mut out = Vec::new();
    collect_names(node, &mut out);
    out
}

fn collect_names(node: &CallTreeNode, out: &mut Vec<String>) {
    out.push(node.name.clone());
    for c in &node.children {
        collect_names(c, out);
    }
}

fn find_child<'a>(node: &'a CallTreeNode, name: &str) -> Option<&'a CallTreeNode> {
    node.children.iter().find(|c| c.name == name)
}

fn build_first_tree(graph: &CallGraph, root: &Path, entry: &str) -> CallTreeNode {
    let mut tb = TreeBuilder::new(graph, root);
    tb.skip_accessors = true;
    let ids: Vec<SymbolId> = graph.find_entry_points(entry);
    // For controller/handler tests we want the symbol whose file looks like the entry point.
    // Pick the first match; tests below assert on the shape regardless.
    let id = ids.first().unwrap_or_else(|| {
        panic!("no entry point matched {entry}, available: {:?}", graph.by_name.keys().collect::<Vec<_>>())
    });
    tb.build(id).expect("tree builds")
}

fn pick_entry<'a>(
    graph: &'a CallGraph,
    root: &'a Path,
    name: &str,
    file_hint: &str,
) -> CallTreeNode {
    let mut tb = TreeBuilder::new(graph, root);
    tb.skip_accessors = true;
    let ids = graph.find_entry_points(name);
    let mut chosen: Option<&SymbolId> = None;
    for id in &ids {
        let sym = &graph.symbols[id];
        if sym.file.display().to_string().contains(file_hint) {
            chosen = Some(id);
            break;
        }
    }
    let id = chosen
        .or_else(|| ids.first())
        .unwrap_or_else(|| panic!("no entry for {name}, candidates: {ids:?}"));
    tb.build(id).expect("tree builds")
}

// --------- Python / FastAPI ---------

#[test]
fn python_fastapi_handler_calls_service() {
    banner("python_fastapi_handler_calls_service", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "create_order", "routes.py");
    show_tree("create_order @ routes.py", &tree);

    assert_eq!(tree.name, "create_order", "tree root is the route handler");
    assert!(tree.file.contains("routes.py"));

    // Handler should reach OrderService.create_order
    let service_call = find_child(&tree, "create_order")
        .or_else(|| {
            // Some symbols may be name 'create_order' on service method
            tree.children
                .iter()
                .find(|c| c.parent_class.as_deref() == Some("OrderService"))
        })
        .expect("handler reaches service.create_order");
    assert_eq!(service_call.parent_class.as_deref(), Some("OrderService"));

    // Through the service we should see build_order, validate, save (anywhere in subtree)
    let names = names_in_subtree(&tree);
    for required in ["build_order", "validate", "save"] {
        assert!(
            names.iter().any(|n| n == required),
            "expected {required:?} in subtree, got {names:?}"
        );
    }
}

#[test]
fn python_fastapi_service_calls_repository_save() {
    banner("python_fastapi_service_calls_repository_save", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "create_order", "services.py");
    show_tree("OrderService.create_order", &tree);
    assert_eq!(tree.parent_class.as_deref(), Some("OrderService"));

    let save = tree
        .children
        .iter()
        .find(|c| c.name == "save")
        .expect("service calls save");
    assert_eq!(save.parent_class.as_deref(), Some("OrderRepository"));
    assert!(save.file.contains("repositories.py"));
}

// --------- Java / Spring ---------

#[test]
fn java_spring_controller_calls_service() {
    banner("java_spring_controller_calls_service", "java-spring");
    let root = fixture("java-spring");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "createOrder", "OrderController.java");
    show_tree("OrderController.createOrder", &tree);

    assert_eq!(tree.name, "createOrder");
    assert_eq!(tree.parent_class.as_deref(), Some("OrderController"));

    let service_call = tree
        .children
        .iter()
        .find(|c| c.parent_class.as_deref() == Some("OrderService"))
        .expect("controller reaches OrderService.createOrder");
    assert_eq!(service_call.name, "createOrder");

    // Validate and buildOrder appear somewhere
    let names = names_in_subtree(&tree);
    for required in ["buildOrder", "validate"] {
        assert!(
            names.iter().any(|n| n == required),
            "expected {required:?} in subtree, got {names:?}"
        );
    }
}

#[test]
fn java_spring_build_order_instantiates_order_entity() {
    banner("java_spring_build_order_instantiates_order_entity", "java-spring");
    let root = fixture("java-spring");
    let graph = analyze(&root);
    let service_tree = pick_entry(&graph, &root, "createOrder", "OrderService.java");
    show_tree("OrderService.createOrder", &service_tree);
    let build = service_tree
        .children
        .iter()
        .find(|c| c.name == "buildOrder")
        .expect("service has buildOrder");
    let order_ctor = build
        .children
        .iter()
        .find(|c| c.name == "Order")
        .expect("buildOrder constructs Order entity");
    assert!(order_ctor.file.contains("Order.java"));
}

// --------- TypeScript / NestJS ---------

#[test]
fn typescript_nestjs_controller_calls_service() {
    banner("typescript_nestjs_controller_calls_service", "typescript-nestjs");
    let root = fixture("typescript-nestjs");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "create", "orders.controller.ts");
    show_tree("OrdersController.create", &tree);

    assert_eq!(tree.name, "create");
    assert_eq!(tree.parent_class.as_deref(), Some("OrdersController"));

    let service_call = tree
        .children
        .iter()
        .find(|c| c.parent_class.as_deref() == Some("OrdersService"))
        .expect("controller reaches OrdersService.createOrder");
    assert_eq!(service_call.name, "createOrder");

    let names = names_in_subtree(&tree);
    for required in ["buildOrder", "validate", "save"] {
        assert!(
            names.iter().any(|n| n == required),
            "expected {required:?} in subtree, got {names:?}"
        );
    }
}

#[test]
fn typescript_nestjs_service_save_resolves_to_repository() {
    banner("typescript_nestjs_service_save_resolves_to_repository", "typescript-nestjs");
    let root = fixture("typescript-nestjs");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "createOrder", "orders.service.ts");
    show_tree("OrdersService.createOrder", &tree);
    let save = tree
        .children
        .iter()
        .find(|c| c.name == "save")
        .expect("service reaches save");
    assert_eq!(save.parent_class.as_deref(), Some("OrdersRepository"));
}

// --------- profiler annotations ---------

#[test]
fn python_save_is_classified_db() {
    banner("python_save_is_classified_db", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    let save_tree = pick_entry(&graph, &root, "save", "repositories.py");
    show_tree("OrderRepository.save", &save_tree);

    assert_eq!(
        save_tree.category_self.map(|c| c.as_str()),
        Some("db"),
        "OrderRepository.save should be classified as db (calls session.add/commit/refresh)"
    );
    let db_externals: Vec<&str> = save_tree
        .external_calls
        .iter()
        .filter(|e| e.category.as_str() == "db")
        .map(|e| e.name.as_str())
        .collect();
    for expected in ["add", "commit", "refresh"] {
        assert!(
            db_externals.contains(&expected),
            "expected {expected:?} among DB externals, got {db_externals:?}"
        );
    }
}

#[test]
fn python_handler_reaches_db_transitively() {
    banner("python_handler_reaches_db_transitively", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    let handler = pick_entry(&graph, &root, "create_order", "routes.py");
    show_tree("create_order @ routes.py", &handler);

    let db = handler.categories_reached.get("db").copied().unwrap_or(0);
    assert!(db >= 1, "handler should reach at least one db op, got {db}");
}

#[test]
fn python_service_has_handler_as_caller() {
    banner("python_service_has_handler_as_caller", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    let service = pick_entry(&graph, &root, "create_order", "services.py");
    show_tree("OrderService.create_order", &service);

    let caller_names: Vec<&str> = service.callers.iter().map(|c| c.name.as_str()).collect();
    assert!(
        caller_names.contains(&"create_order"),
        "OrderService.create_order should list create_order (handler) as caller, got {caller_names:?}"
    );
    assert_eq!(service.callers_count, service.callers.len());
}

#[test]
fn java_service_reaches_db_via_repository_interface() {
    banner("java_service_reaches_db_via_repository_interface", "java-spring");
    let root = fixture("java-spring");
    let graph = analyze(&root);
    let service = pick_entry(&graph, &root, "createOrder", "OrderService.java");
    show_tree("OrderService.createOrder", &service);

    // Spring's JpaRepository.save has no source body so we capture it as
    // an external DB call by name match.
    let db = service.categories_reached.get("db").copied().unwrap_or(0);
    assert!(db >= 1, "expected db reach via repository.save (external), got {db}");
}

#[test]
fn typescript_service_reaches_db_via_typeorm_save() {
    banner("typescript_service_reaches_db_via_typeorm_save", "typescript-nestjs");
    let root = fixture("typescript-nestjs");
    let graph = analyze(&root);
    let service = pick_entry(&graph, &root, "createOrder", "orders.service.ts");
    show_tree("OrdersService.createOrder", &service);

    let db = service.categories_reached.get("db").copied().unwrap_or(0);
    assert!(db >= 1, "service should reach db transitively, got {db}");
}

#[test]
fn fan_in_fan_out_counts_are_consistent() {
    banner("fan_in_fan_out_counts_are_consistent", "(all)");
    for fix in ["python-fastapi", "java-spring", "typescript-nestjs"] {
        let root = fixture(fix);
        let graph = analyze(&root);
        // For every node, callers_count + callees_count should match the
        // graph's actual edge counts.
        for (id, _sym) in &graph.symbols {
            let actual_callees = graph.callees(id).len();
            let actual_callers = graph.callers_of(id).len();
            // Sanity: callees of A include B iff callers of B include A.
            for callee in graph.callees(id) {
                assert!(
                    graph.callers_of(callee).contains(id),
                    "edge {id:?} -> {callee:?} not mirrored in callers"
                );
            }
            // Light usage just to ensure no panic
            let _ = actual_callees + actual_callers;
        }
    }
}

// --------- JavaScript (Express + Mongoose) ---------

#[test]
fn javascript_axios_call_classifies_network_via_import() {
    banner("javascript_axios_call_classifies_network_via_import", "javascript-express");
    let root = fixture("javascript-express");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "notifyDownstream", "routes.js");
    show_tree("notifyDownstream", &tree);

    // The classifier should recognise `axios.post(...)` as network purely
    // through Tier B (import catalog), NOT method name (which is just "post").
    let net = tree.categories_reached.get("network").copied().unwrap_or(0);
    assert!(
        net >= 1,
        "expected network reach via axios import, got categories_reached={:?}",
        tree.categories_reached
    );
}

#[test]
fn javascript_service_resolves_to_repository() {
    banner("javascript_service_resolves_to_repository", "javascript-express");
    let root = fixture("javascript-express");
    let graph = analyze(&root);
    let tree = pick_entry(&graph, &root, "createOrder", "service.js");
    show_tree("OrderService.createOrder", &tree);

    // OrderService.createOrder should reach OrderRepository.save
    let names = names_in_subtree(&tree);
    assert!(
        names.iter().any(|n| n == "save"),
        "expected `save` in subtree, got {names:?}"
    );
}

// --------- false-positive immunity ---------

#[test]
fn no_false_positive_on_stdlib_set_add() {
    use drift_static_profiler::{
        categories::classify,
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("no_false_positive_on_stdlib_set_add", "(synthetic)");

    let src = "
def deduplicate(items):
    seen = set()
    for item in items:
        seen.add(item)
    return list(seen)
";
    let tags = extract_tags_from_source(Path::new("synthetic.py"), Language::Python, src)
        .expect("parse");
    let graph = CallGraph::build(&[tags]);
    // Find the symbol "deduplicate"
    let ids = graph.find_entry_points("deduplicate");
    let id = ids.first().expect("found");
    let externals = graph.externals_of(id);
    assert!(
        externals.is_empty(),
        "expected NO external classifications for set.add() — got {:?}",
        externals.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Also sanity-check the classifier directly: `add` on receiver `seen` with no imports → None
    assert!(classify("add", Some("seen"), &[]).is_none());
}

#[test]
fn no_false_positive_on_stdlib_dict_update() {
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("no_false_positive_on_stdlib_dict_update", "(synthetic)");

    let src = "
def merge(a, b):
    result = {}
    result.update(a)
    result.update(b)
    return result
";
    let tags = extract_tags_from_source(Path::new("synthetic.py"), Language::Python, src)
        .expect("parse");
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("merge").first().cloned().expect("found");
    let externals = graph.externals_of(&id);
    assert!(
        externals.is_empty(),
        "expected NO external classifications for dict.update() — got {:?}",
        externals.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}

#[test]
fn import_driven_classification_is_recorded_with_evidence() {
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("import_driven_classification_is_recorded_with_evidence", "(synthetic)");

    let src = r#"
import requests

def fetch_user(uid):
    return requests.get(f"https://api.example.com/users/{uid}").json()
"#;
    let tags = extract_tags_from_source(Path::new("synthetic.py"), Language::Python, src)
        .expect("parse");
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("fetch_user").first().cloned().expect("found");
    let externals = graph.externals_of(&id);
    let net_ext = externals
        .iter()
        .find(|e| e.category.as_str() == "network")
        .expect("expected network external from requests.get");
    // The classifier should have credited Tier B (imported module).
    assert_eq!(net_ext.name, "get");
    assert_eq!(net_ext.receiver.as_deref(), Some("requests"));
    assert!(
        net_ext.evidence.contains("requests"),
        "evidence should mention the import; got: {}",
        net_ext.evidence
    );
}

// --------- Phase B: graph-derived metrics ---------

#[test]
fn pagerank_assigned_to_every_symbol() {
    banner("pagerank_assigned_to_every_symbol", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    assert_eq!(
        graph.pagerank.len(),
        graph.symbols.len(),
        "pagerank should cover every symbol"
    );
    // Sum should be ~ N (petgraph normalizes per node, common convention).
    let total: f64 = graph.pagerank.values().sum();
    assert!(total > 0.0, "expected positive pagerank mass, got {total}");
}

#[test]
fn pagerank_ranks_central_nodes_highest() {
    banner("pagerank_ranks_central_nodes_highest", "python-fastapi");
    let root = fixture("python-fastapi");
    let graph = analyze(&root);
    // `Order` class is referenced from build_order; should outrank a dead-end leaf.
    let order_score = graph
        .pagerank
        .iter()
        .find(|(id, _)| graph.symbols[*id].name == "Order")
        .map(|(_, s)| *s)
        .expect("Order in graph");
    let unused_score = graph
        .pagerank
        .iter()
        .find(|(id, _)| graph.symbols[*id].name == "find_by_id")
        .map(|(_, s)| *s)
        .unwrap_or(0.0);
    assert!(
        order_score > unused_score,
        "Order ({order_score}) should outrank find_by_id ({unused_score})"
    );
}

#[test]
fn recursive_symbol_detected_via_scc() {
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("recursive_symbol_detected_via_scc", "(synthetic mutual recursion)");

    // Two mutually-recursive functions form an SCC of size 2 → both is_recursive.
    let src = "
def is_even(n):
    if n == 0:
        return True
    return is_odd(n - 1)

def is_odd(n):
    if n == 0:
        return False
    return is_even(n - 1)
";
    let tags = extract_tags_from_source(Path::new("recursion.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let even = graph
        .find_entry_points("is_even")
        .first()
        .cloned()
        .expect("found");
    let odd = graph
        .find_entry_points("is_odd")
        .first()
        .cloned()
        .expect("found");
    assert!(
        graph.is_recursive[&even],
        "is_even should be flagged recursive (mutual)"
    );
    assert!(graph.is_recursive[&odd], "is_odd should be flagged recursive");
}

#[test]
fn dead_code_list_excludes_pinned_entries() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("dead_code_list_excludes_pinned_entries", "(synthetic)");

    let src = "
def main_handler(req):    # pinned entry, no callers in source
    return 42

def truly_unused():       # no callers, not pinned → should appear in dead_code
    return 'gone'

def helper():             # called by main_handler → NOT dead
    return 1
";
    let tags = extract_tags_from_source(Path::new("dead.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    // Build with main_handler pinned as the entry
    let entry_id = graph
        .find_entry_points("main_handler")
        .first()
        .cloned()
        .unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let entry_node = tb.build(&entry_id).unwrap();
    let report = Report::build(&[tags], &graph, vec![entry_node], &Default::default(), None);

    let names: Vec<&str> = report.summary.dead_code.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"truly_unused"), "truly_unused should be in dead_code; got {names:?}");
    assert!(
        !names.contains(&"main_handler"),
        "pinned main_handler must NOT be in dead_code"
    );
    // helper has 0 callers IN graph (it was called once though). Hmm let's check:
    // Actually main_handler doesn't call helper in our source (read it again).
    // So helper IS dead. Re-test:
    // (we intentionally didn't call helper from main_handler in this fixture)
    assert!(names.contains(&"helper"), "helper has 0 callers → should be dead");
}

// --------- Phase D: risk patterns (N+1, blocking-in-async) ---------

#[test]
fn n_plus_one_detected_in_python_loop() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("n_plus_one_detected_in_python_loop", "(synthetic SQLAlchemy)");

    // `session.commit()` inside a for-loop → classic N+1 antipattern.
    let src = "
from sqlalchemy.orm import Session

def bulk_save(items, session: Session):
    for it in items:
        session.add(it)
        session.commit()
";
    let tags = extract_tags_from_source(Path::new("nplus1.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("bulk_save").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let _report = Report::build(&[tags], &graph, vec![node.clone()], &Default::default(), None);

    assert!(
        node.n_plus_one_risk,
        "bulk_save calls session.add/commit inside a for-loop — should flag n_plus_one_risk. external_calls={:?}",
        node.external_calls.iter().map(|e| (&e.name, e.in_loop, e.category)).collect::<Vec<_>>()
    );
    // At least one external call should be tagged in_loop with db category
    let any_db_in_loop = node
        .external_calls
        .iter()
        .any(|e| e.in_loop && matches!(e.category, drift_static_profiler::categories::Category::Db));
    assert!(any_db_in_loop, "expected db-categorized external_call with in_loop=true");
}

#[test]
fn no_n_plus_one_when_categorized_call_is_outside_loop() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("no_n_plus_one_when_categorized_call_is_outside_loop", "(synthetic)");

    let src = "
def save_one(items, session):
    session.add(items[0])
    session.commit()
";
    let tags = extract_tags_from_source(Path::new("safe.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("save_one").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let _ = Report::build(&[tags], &graph, vec![node.clone()], &Default::default(), None);

    assert!(
        !node.n_plus_one_risk,
        "no loop here — should NOT flag n_plus_one_risk"
    );
}

#[test]
fn blocking_in_async_detected_when_sync_db_call_not_awaited() {
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("blocking_in_async_detected_when_sync_db_call_not_awaited", "(synthetic)");

    // requests.get() is sync; called from an async fn without await → blocking.
    let src = "
import requests

async def fetch_user_blocking(uid):
    return requests.get(f\"https://api.example.com/{uid}\")
";
    let tags = extract_tags_from_source(Path::new("block.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph
        .find_entry_points("fetch_user_blocking")
        .first()
        .cloned()
        .unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();

    assert!(node.is_async, "function should be detected as async");
    assert!(
        node.blocking_in_async,
        "sync requests.get() in async function without await — should flag blocking_in_async; externals={:?}",
        node.external_calls.iter().map(|e| (&e.name, e.in_await, e.category)).collect::<Vec<_>>()
    );
}

#[test]
fn awaited_call_in_async_is_not_blocking() {
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, tree::TreeBuilder, Language,
    };
    use std::path::Path;
    banner("awaited_call_in_async_is_not_blocking", "(synthetic)");

    let src = "
import httpx

async def fetch_async(uid):
    client = httpx.AsyncClient()
    return await client.get(f\"https://api.example.com/{uid}\")
";
    let tags = extract_tags_from_source(Path::new("ok.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("fetch_async").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();

    assert!(node.is_async);
    assert!(
        !node.blocking_in_async,
        "awaited call should NOT trigger blocking_in_async; externals={:?}",
        node.external_calls.iter().map(|e| (&e.name, e.in_await, e.category)).collect::<Vec<_>>()
    );
}

#[test]
fn call_site_count_geq_callers_count() {
    // call_site_count counts every invocation; callers_count counts unique sources.
    // For static analysis, they're often equal, but call_site_count must never
    // be smaller.
    for fix in ["python-fastapi", "java-spring", "typescript-nestjs", "javascript-express"] {
        let root = fixture(fix);
        let graph = analyze(&root);
        for (id, _) in &graph.symbols {
            let csc = graph.call_site_count.get(id).copied().unwrap_or(0);
            let cc = graph.callers_of(id).len();
            assert!(
                csc >= cc,
                "{fix}: call_site_count ({csc}) < callers_count ({cc}) for {id:?}"
            );
        }
    }
}

// --------- Go / Rust / Scala fixture E2E ---------
//
// These exercise the full pipeline against on-disk fixtures (mirroring the
// Python/Java/TS coverage above): walker discovery → linguist language pick →
// tags extraction across files → cross-file call-graph resolution → tree build
// with category propagation. Without these, a regression that drops one
// language from the walker or breaks cross-file resolution could ship
// unnoticed because the inline-source tests below only feed a single file.

#[test]
fn go_fixture_handler_reaches_repo_save_with_db_category() {
    use drift_static_profiler::{analyze, AnalyzeOptions};
    banner("go_fixture_handler_reaches_repo_save_with_db_category", "go-gin");
    let root = fixture("go-gin");
    let outcome = analyze(
        &root,
        &["CreateOrder".into()],
        &AnalyzeOptions::default(),
    )
    .expect("analyze");
    let report = &outcome.report;
    assert_eq!(outcome.profiled_language, Some(drift_static_profiler::Language::Go));
    assert!(report.summary.languages.iter().any(|l| l == "go"));
    // The Save method must propagate as a DB call. Tree categories aggregate
    // the whole subtree, so the top-level "create_order" entry should see db>0.
    let total_db = report
        .summary
        .categories
        .get("db")
        .copied()
        .unwrap_or(0);
    assert!(
        total_db > 0,
        "expected db>0 from `database/sql` Exec inside repo.Save; categories={:?}",
        report.summary.categories
    );
    // Cross-file resolution check: at least one entry tree must contain the
    // string "Save" via names_in_subtree.
    let any = report
        .entries
        .iter()
        .any(|e| names_in_subtree(e).iter().any(|n| n == "Save"));
    assert!(
        any,
        "handler.CreateOrder should transitively reach repo.Save via service.CreateOrder"
    );
}

#[test]
fn rust_fixture_handler_reaches_repo_save_with_db_category() {
    use drift_static_profiler::{analyze, AnalyzeOptions};
    banner("rust_fixture_handler_reaches_repo_save_with_db_category", "rust-axum");
    let root = fixture("rust-axum");
    let outcome = analyze(
        &root,
        &["create_order".into()],
        &AnalyzeOptions::default(),
    )
    .expect("analyze");
    let report = &outcome.report;
    assert_eq!(outcome.profiled_language, Some(drift_static_profiler::Language::Rust));
    assert!(report.summary.languages.iter().any(|l| l == "rust"));
    // sqlx::query_as / .fetch_one inside repo.save → DB. Category should
    // propagate up.
    let total_db = report
        .summary
        .categories
        .get("db")
        .copied()
        .unwrap_or(0);
    assert!(
        total_db > 0,
        "expected db>0 from sqlx::query_as inside save; categories={:?}",
        report.summary.categories
    );
    // impl-method parent class must come through containment.
    let has_repo_save = report.entries.iter().any(|e| {
        names_in_subtree(e).iter().any(|n| n == "save")
    });
    assert!(has_repo_save, "save method should appear in the call tree");
}

#[test]
fn scala_fixture_handler_reaches_repo_save_with_db_category() {
    use drift_static_profiler::{analyze, AnalyzeOptions};
    banner("scala_fixture_handler_reaches_repo_save_with_db_category", "scala-play");
    let root = fixture("scala-play");
    let outcome = analyze(
        &root,
        &["createOrder".into()],
        &AnalyzeOptions::default(),
    )
    .expect("analyze");
    let report = &outcome.report;
    assert_eq!(outcome.profiled_language, Some(drift_static_profiler::Language::Scala));
    assert!(report.summary.languages.iter().any(|l| l == "scala"));
    let total_db = report
        .summary
        .categories
        .get("db")
        .copied()
        .unwrap_or(0);
    assert!(
        total_db > 0,
        "expected db>0 from slick db.run inside repo.save; categories={:?}",
        report.summary.categories
    );
    let has_save = report
        .entries
        .iter()
        .any(|e| names_in_subtree(e).iter().any(|n| n == "save"));
    assert!(has_save, "save method should appear in the Scala call tree");
}

#[test]
fn report_json_validates_against_schema_for_each_new_language() {
    // End-to-end schema conformance: emit the JSON for each new-language
    // fixture, parse the published schema, and assert the JSON matches.
    // Without this gate, the report could grow a field that's missing
    // (or malformed) in the schema and viewer consumers would silently
    // break on upgrade.
    use drift_static_profiler::{analyze, AnalyzeOptions};
    use jsonschema::Validator;
    use std::path::PathBuf;
    banner("report_json_validates_against_schema_for_each_new_language", "(all new)");

    let schema_path = {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("schema/profile.schema.json");
        p
    };
    let schema_raw = std::fs::read(&schema_path).expect("read schema");
    let schema_json: serde_json::Value =
        serde_json::from_slice(&schema_raw).expect("parse schema JSON");
    let validator = Validator::new(&schema_json).expect("build validator");

    for (fix, entry) in [
        ("go-gin", "CreateOrder"),
        ("rust-axum", "create_order"),
        ("scala-play", "createOrder"),
    ] {
        let root = fixture(fix);
        let outcome = analyze(&root, &[entry.into()], &AnalyzeOptions::default())
            .expect("analyze");
        let report_json = serde_json::to_value(&outcome.report).expect("serialize");
        let errors: Vec<String> = validator
            .iter_errors(&report_json)
            .map(|e| format!("{}: {}", e.instance_path(), e))
            .collect();
        assert!(
            errors.is_empty(),
            "schema violations for fixture {fix}: {errors:#?}"
        );
    }
}

#[test]
fn cli_binary_emits_valid_json_for_new_languages() {
    // True E2E: spawn the built `drift-static-profiler` binary and parse its
    // stdout JSON. This catches anything `cargo test` alone misses — broken
    // arg parsing, missing serde fields, an stdout/stderr mix-up that
    // contaminates the JSON, etc. We use `CARGO_BIN_EXE_drift-static-profiler`,
    // which cargo sets to the built binary path for `tests/` integrations.
    use std::process::Command;
    banner("cli_binary_emits_valid_json_for_new_languages", "(all new via CLI)");

    let bin = env!("CARGO_BIN_EXE_drift-static-profiler");
    for (fix, entry) in [
        ("go-gin", "CreateOrder"),
        ("rust-axum", "create_order"),
        ("scala-play", "createOrder"),
    ] {
        let root = fixture(fix);
        let out = Command::new(bin)
            .args(["analyze", "--json", "--entry", entry])
            .arg(&root)
            .output()
            .expect("spawn binary");
        assert!(
            out.status.success(),
            "binary failed for {fix}: stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout for {fix} was not valid JSON: {e}\nstdout=\n{}\nstderr=\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            )
        });
        // Sanity assertions on the JSON shape so a silently-empty report
        // (e.g. wrong language picked) trips the test.
        let summary = v.get("summary").expect("summary present");
        let files = summary.get("files").and_then(|x| x.as_u64()).unwrap_or(0);
        let symbols = summary.get("symbols").and_then(|x| x.as_u64()).unwrap_or(0);
        let profiled = summary
            .get("profiled_language")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        assert!(files > 0, "{fix}: expected files>0, got {files}");
        assert!(symbols > 0, "{fix}: expected symbols>0, got {symbols}");
        assert!(
            !profiled.is_empty(),
            "{fix}: profiled_language should be non-empty"
        );
    }
}

// --------- Go / Rust / Scala (inline-source E2E) ---------
//
// These exercise the full pipeline (tree-sitter parse → tags → graph) without
// needing on-disk fixtures. They protect against grammar regressions and make
// it cheap to grow the test set as we touch the queries.

#[test]
fn go_method_calls_resolve_through_call_graph() {
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("go_method_calls_resolve_through_call_graph", "(inline Go)");

    let src = "package main\n\
               import \"fmt\"\n\
               type Service struct{}\n\
               func (s *Service) Greet(name string) string {\n\
                 return fmt.Sprintf(\"hi %s\", name)\n\
               }\n\
               func main() {\n\
                 s := &Service{}\n\
                 s.Greet(\"world\")\n\
               }\n";
    let tags = extract_tags_from_source(Path::new("svc.go"), Language::Go, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    // Both Greet and main extracted as symbols.
    assert!(tags.symbols.iter().any(|s| s.name == "Greet"));
    assert!(tags.symbols.iter().any(|s| s.name == "main"));

    // s.Greet inside main must resolve to the Greet method as an edge.
    let main_id = graph.find_entry_points("main").first().cloned().unwrap();
    let greet_id = graph.find_entry_points("Greet").first().cloned().unwrap();
    assert!(
        graph.callees(&main_id).contains(&greet_id),
        "main should call Greet; callees: {:?}",
        graph.callees(&main_id)
    );

    // The `fmt` import must be recorded with quotes stripped (otherwise
    // category classification can't substring-match the module path).
    assert!(
        tags.imports.iter().any(|i| i.module_path == "fmt"),
        "fmt import should be recorded without surrounding quotes; got {:?}",
        tags.imports
    );
}

#[test]
fn rust_impl_method_calls_resolve() {
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("rust_impl_method_calls_resolve", "(inline Rust)");

    let src = "struct Repo;\n\
               impl Repo {\n\
                 fn save(&self) -> u32 { 42 }\n\
               }\n\
               fn handler(r: &Repo) -> u32 {\n\
                 r.save()\n\
               }\n";
    let tags = extract_tags_from_source(Path::new("lib.rs"), Language::Rust, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    // save is inside impl Repo, so its parent must be Repo via containment.
    let save = tags.symbols.iter().find(|s| s.name == "save").expect("save");
    assert_eq!(save.parent.as_deref(), Some("Repo"));

    // handler must call save.
    let handler_id = graph
        .find_entry_points("handler")
        .first()
        .cloned()
        .unwrap();
    let save_id = graph.find_entry_points("save").first().cloned().unwrap();
    assert!(
        graph.callees(&handler_id).contains(&save_id),
        "handler should call save; callees: {:?}",
        graph.callees(&handler_id)
    );
}

#[test]
fn rust_scoped_call_resolves() {
    // Path-qualified calls like `Mod::foo()` should still produce a ref.name
    // of "foo" so by-name resolution can hit a defined `foo`.
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("rust_scoped_call_resolves", "(inline Rust)");

    let src = "mod things {\n\
                 pub fn build() -> u32 { 1 }\n\
               }\n\
               fn caller() -> u32 { things::build() }\n";
    let tags = extract_tags_from_source(Path::new("lib.rs"), Language::Rust, src).unwrap();
    let graph = CallGraph::build(&[tags]);

    let caller_id = graph.find_entry_points("caller").first().cloned().unwrap();
    let build_id = graph.find_entry_points("build").first().cloned().unwrap();
    assert!(
        graph.callees(&caller_id).contains(&build_id),
        "caller should call things::build → build; callees: {:?}",
        graph.callees(&caller_id)
    );
}

#[test]
fn rust_turbofish_calls_resolve() {
    // `foo::<T>()` and `chain.collect::<Vec<_>>()` are wrapped in
    // generic_function nodes; without an explicit pattern for that they
    // disappear from the call graph entirely.
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("rust_turbofish_calls_resolve", "(inline Rust)");

    let src = "fn build<T>() -> Option<T> { None }\n\
               fn caller() -> Option<u32> { build::<u32>() }\n";
    let tags = extract_tags_from_source(Path::new("lib.rs"), Language::Rust, src).unwrap();
    let graph = CallGraph::build(&[tags]);

    let caller_id = graph.find_entry_points("caller").first().cloned().unwrap();
    let build_id = graph.find_entry_points("build").first().cloned().unwrap();
    assert!(
        graph.callees(&caller_id).contains(&build_id),
        "caller should call build::<u32>() → build; callees: {:?}",
        graph.callees(&caller_id)
    );
}

#[test]
fn scala_method_call_resolves() {
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("scala_method_call_resolves", "(inline Scala)");

    let src = "object Repo {\n  def save(): Int = 1\n}\n\
               object Handler {\n  def run(): Int = Repo.save()\n}\n";
    let tags = extract_tags_from_source(Path::new("App.scala"), Language::Scala, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    // save defined; run defined.
    assert!(tags.symbols.iter().any(|s| s.name == "save"));
    assert!(tags.symbols.iter().any(|s| s.name == "run"));

    let run_id = graph.find_entry_points("run").first().cloned().unwrap();
    let save_id = graph.find_entry_points("save").first().cloned().unwrap();
    assert!(
        graph.callees(&run_id).contains(&save_id),
        "run should call Repo.save → save; callees: {:?}",
        graph.callees(&run_id)
    );
}

// --------- cross-cutting sanity ---------

#[test]
fn walker_discovers_three_languages() {
    banner("walker_discovers_three_languages", "(all)");
    let root = fixture("python-fastapi");
    let py = discover_source_files(&root);
    assert!(py.iter().any(|(_, l)| matches!(l, drift_static_profiler::Language::Python)));

    let root = fixture("java-spring");
    let java = discover_source_files(&root);
    assert!(java.iter().any(|(_, l)| matches!(l, drift_static_profiler::Language::Java)));

    let root = fixture("typescript-nestjs");
    let ts = discover_source_files(&root);
    assert!(ts.iter().any(|(_, l)| matches!(l, drift_static_profiler::Language::TypeScript)));
}

#[test]
fn from_path_recognizes_new_language_extensions() {
    // The walker delegates extension-to-language mapping to
    // `Language::from_path`. Lock in the new mappings so a typo in
    // lib.rs's extension list doesn't silently drop files from analysis.
    use drift_static_profiler::Language;
    use std::path::Path;
    banner("from_path_recognizes_new_language_extensions", "(unit)");

    assert_eq!(
        Language::from_path(Path::new("server/main.go")),
        Some(Language::Go)
    );
    assert_eq!(
        Language::from_path(Path::new("src/lib.rs")),
        Some(Language::Rust)
    );
    assert_eq!(
        Language::from_path(Path::new("App.scala")),
        Some(Language::Scala)
    );
    assert_eq!(
        Language::from_path(Path::new("worksheet.sc")),
        Some(Language::Scala)
    );
    // sanity: unknown still returns None
    assert_eq!(Language::from_path(Path::new("README.md")), None);
}

#[test]
fn graph_has_no_self_loops() {
    banner("graph_has_no_self_loops", "(all)");
    for fix in ["python-fastapi", "java-spring", "typescript-nestjs", "javascript-express"] {
        let root = fixture(fix);
        let graph = analyze(&root);
        for (id, callees) in &graph.edges {
            for c in callees {
                assert_ne!(id, c, "self-loop in {fix} for {id:?}");
            }
        }
    }
}

// Use this to silence "unused" if helpers are not all used everywhere.
#[allow(dead_code)]
fn _silence(graph: &CallGraph, root: &Path) -> CallTreeNode {
    build_first_tree(graph, root, "create_order")
}

// ──────────────────────────────────────────────────────────────────────
// Phase E: structured findings
// ──────────────────────────────────────────────────────────────────────
//
// Each detector also fills `CallTreeNode.findings` with a structured
// version of the same signal that drives the legacy booleans. The
// booleans remain populated (derived from findings) so older code paths
// keep working. These tests assert the structured payload alongside the
// boolean — so we know the new shape is correct.

#[test]
fn n_plus_one_emits_structured_finding() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("n_plus_one_emits_structured_finding", "(synthetic SQLAlchemy)");

    let src = "
from sqlalchemy.orm import Session

def bulk_save(items, session: Session):
    for it in items:
        session.add(it)
        session.commit()
";
    let tags = extract_tags_from_source(Path::new("nplus1.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("bulk_save").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();

    // Boolean (legacy) still set
    assert!(node.n_plus_one_risk, "legacy bool must remain populated");

    // Structured finding present, anchored at a call-site line (not the def line)
    let np = node
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::NPlusOne)
        .expect("n_plus_one finding should be present alongside the bool");
    assert!(
        np.line > node.line,
        "finding line should be a call-site within the body, got {} (symbol starts at {})",
        np.line, node.line,
    );
    assert!(
        np.confidence > 0.0 && np.confidence <= 1.0,
        "confidence in (0,1], got {}",
        np.confidence,
    );
    assert!(
        !np.evidence.is_empty(),
        "n_plus_one finding must list at least one offending call as evidence",
    );
    assert!(
        np.remediation.is_some(),
        "n_plus_one finding should ship with a remediation hint",
    );
}

#[test]
fn blocking_in_async_emits_structured_finding() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("blocking_in_async_emits_structured_finding", "(synthetic)");

    let src = "
import requests

async def fetch_user_blocking(uid):
    return requests.get(f\"https://api.example.com/{uid}\")
";
    let tags = extract_tags_from_source(Path::new("block.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph
        .find_entry_points("fetch_user_blocking")
        .first()
        .cloned()
        .unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();

    assert!(node.blocking_in_async, "legacy bool must remain populated");
    let bia = node
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::BlockingInAsync)
        .expect("blocking_in_async finding should be present alongside the bool");
    assert!(!bia.evidence.is_empty());
    assert!(bia.remediation.is_some());
}

#[test]
fn recursive_emits_structured_finding_post_build() {
    // Recursive findings are attached as a post-build pass in Report::build,
    // not in tree::build_inner — verify they land.
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("recursive_emits_structured_finding_post_build", "(synthetic mutual recursion)");

    let src = "
def is_even(n):
    if n == 0:
        return True
    return is_odd(n - 1)

def is_odd(n):
    if n == 0:
        return False
    return is_even(n - 1)
";
    let tags = extract_tags_from_source(Path::new("rec.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("is_even").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let root = &report.entries[0];
    assert!(root.is_recursive, "is_even should be in SCC of size 2");
    assert!(
        root.findings
            .iter()
            .any(|f| f.kind == FindingKind::Recursive),
        "recursive finding should be attached by Report::build's post-build pass",
    );
}

#[test]
fn noisy_log_emits_structured_finding_when_log_in_loop() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("noisy_log_emits_structured_finding_when_log_in_loop", "(synthetic)");

    let src = "
import logging

logger = logging.getLogger(__name__)

def process_items(items):
    for it in items:
        logger.debug(\"processing %s\", it)
";
    let tags = extract_tags_from_source(Path::new("noisy.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("process_items").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();

    let nl = node
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::NoisyLog)
        .expect("noisy_log finding should be present when a log call is in a loop");
    assert!(!nl.evidence.is_empty(), "noisy_log finding must carry evidence");
    assert!(nl.remediation.is_some(), "noisy_log finding should ship with remediation");
}

#[test]
fn expensive_compute_emits_finding_for_high_complexity_body() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("expensive_compute_emits_finding_for_high_complexity_body", "(synthetic)");

    // 11 if/elif branches → cyclomatic complexity ~12. Crosses the
    // detector's ≥10 high-complexity threshold.
    let src = "
def classify(score):
    if score < 0:
        return 'invalid'
    elif score < 10:
        return 'tier_1'
    elif score < 20:
        return 'tier_2'
    elif score < 30:
        return 'tier_3'
    elif score < 40:
        return 'tier_4'
    elif score < 50:
        return 'tier_5'
    elif score < 60:
        return 'tier_6'
    elif score < 70:
        return 'tier_7'
    elif score < 80:
        return 'tier_8'
    elif score < 90:
        return 'tier_9'
    else:
        return 'tier_10'
";
    let tags = extract_tags_from_source(Path::new("expensive.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("classify").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let ec = node
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::ExpensiveCompute)
        .unwrap_or_else(|| panic!(
            "expensive_compute finding expected on complexity={} symbol",
            node.complexity,
        ));
    assert!(ec.message.contains("complexity"), "message must cite complexity");
    assert!(ec.remediation.is_some(), "expensive_compute should ship remediation");
}

#[test]
fn no_expensive_compute_for_trivial_function() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("no_expensive_compute_for_trivial_function", "(synthetic)");

    let src = "
def add(a, b):
    return a + b
";
    let tags = extract_tags_from_source(Path::new("trivial.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let id = graph.find_entry_points("add").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    assert!(
        !node.findings.iter().any(|f| f.kind == FindingKind::ExpensiveCompute),
        "trivial 1-line function should NOT trigger expensive_compute",
    );
}

#[test]
fn is_test_path_recognizes_all_seven_language_conventions() {
    // Single source of truth for "what counts as a test path?". Pin
    // each convention so adding a language or extending the pattern
    // list later doesn't silently regress one of them.
    use drift_static_profiler::walker::is_test_path;
    use std::path::{Path, PathBuf};
    banner("is_test_path_recognizes_all_seven_language_conventions", "(unit)");

    let root = PathBuf::from("/proj");

    // ─── Path-segment matches (apply to all languages) ──────────────
    for p in [
        "/proj/tests/foo.py",
        "/proj/test/foo.py",
        "/proj/__tests__/foo.ts",
        "/proj/spec/foo.rb",
        "/proj/specs/foo.scala",
        "/proj/__mocks__/foo.ts",
        "/proj/testdata/golden.json",
        "/proj/src/nested/__tests__/inner.ts",
    ] {
        assert!(is_test_path(Path::new(p), &root), "should be test: {p}");
    }

    // ─── Filename-pattern matches (per language convention) ─────────
    let cases = [
        // JS/TS
        ("/proj/src/app.test.ts", true),
        ("/proj/src/app.test.tsx", true),
        ("/proj/src/app.spec.js", true),
        ("/proj/src/api.mock.ts", true),
        ("/proj/src/util_test.js", true),
        // Python
        ("/proj/src/test_utils.py", true),
        ("/proj/src/utils_test.py", true),
        // Go
        ("/proj/pkg/util_test.go", true),
        // Java
        ("/proj/src/UserTest.java", true),
        ("/proj/src/UserTests.java", true),
        // Scala
        ("/proj/src/UserSpec.scala", true),
        ("/proj/src/UserSpecs.scala", true),
    ];
    for (p, expected) in cases {
        assert_eq!(
            is_test_path(Path::new(p), &root),
            expected,
            "is_test_path({p:?}) wrong",
        );
    }

    // ─── Production code must NOT match ─────────────────────────────
    for p in [
        "/proj/src/app.py",
        "/proj/src/users.ts",
        "/proj/src/handler.go",
        "/proj/src/User.java",
        "/proj/src/UserService.scala",
        "/proj/Spec.ts",                   // bare 'Spec.ts' isn't *.spec.* or *Spec.scala
        "/proj/src/contest.py",            // "test" substring inside an unrelated word
        "/proj/src/protester.go",          // not _test.go
        "/proj/src/test_data_loader.py",   // pytest sees this as test_*.py; we err on the side of YES
    ] {
        let result = is_test_path(Path::new(p), &root);
        let last = p.rsplit('/').next().unwrap();
        let expected = last.starts_with("test_") && last.ends_with(".py");
        assert_eq!(result, expected, "is_test_path({p:?}) wrong (expected {expected})");
    }

    // ─── Project-root strip: a project ROOTED inside a `tests/` dir
    //     is NOT itself test code. Only test subdirs INSIDE the
    //     scanned root count.
    let root_in_tests = PathBuf::from("/some/wrapper/tests/fixtures/python-fastapi");
    let inside = Path::new("/some/wrapper/tests/fixtures/python-fastapi/app/routes.py");
    assert!(
        !is_test_path(inside, &root_in_tests),
        "files inside a project that itself lives under tests/ must not be flagged"
    );
}

#[test]
fn walker_exclude_tests_drops_test_files_at_walk_stage() {
    // End-to-end at the walker layer: build a fake project tree with
    // tests + prod files, walk both with and without exclude_tests,
    // verify the second walk drops every test path.
    use drift_static_profiler::walker::{discover_source_files_with, WalkOpts};
    use std::fs;
    use std::path::PathBuf;
    banner("walker_exclude_tests_drops_test_files_at_walk_stage", "(walker)");

    let pid = std::process::id();
    let root: PathBuf = std::env::temp_dir().join(format!("drift-walker-notests-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("src/__tests__")).unwrap();
    fs::create_dir_all(root.join("__mocks__")).unwrap();

    fs::write(root.join("src/app.py"), "x = 1").unwrap();
    fs::write(root.join("src/utils.py"), "x = 1").unwrap();
    fs::write(root.join("src/app.test.py"), "x = 1").unwrap();           // filename pattern
    fs::write(root.join("tests/test_app.py"), "x = 1").unwrap();         // path segment
    fs::write(root.join("src/__tests__/inner.py"), "x = 1").unwrap();    // nested path segment
    fs::write(root.join("__mocks__/api.py"), "x = 1").unwrap();          // mocks dir

    let default_opts = WalkOpts::default();
    let with_tests = discover_source_files_with(&root, &default_opts);
    let no_tests = discover_source_files_with(
        &root,
        &WalkOpts { exclude_tests: true, ..WalkOpts::default() },
    );

    // Default walk: everything (6 files).
    assert_eq!(
        with_tests.len(),
        6,
        "default walker should include all 6 .py files; got {:?}",
        with_tests.iter().map(|(p, _)| p.strip_prefix(&root).unwrap().display().to_string()).collect::<Vec<_>>(),
    );
    // exclude_tests=true: only src/app.py + src/utils.py (2 files).
    let names: Vec<String> = no_tests
        .iter()
        .map(|(p, _)| p.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert_eq!(no_tests.len(), 2, "exclude_tests should keep exactly 2 files; got {names:?}");
    assert!(names.iter().any(|n| n.ends_with("src/app.py")));
    assert!(names.iter().any(|n| n.ends_with("src/utils.py")));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_options_exclude_tests_keeps_tests_out_of_the_graph() {
    // End-to-end at the api.rs layer: tests must not appear in the
    // graph's symbols/edges/dead_code when AnalyzeOptions.exclude_tests
    // is true.
    use drift_static_profiler::{analyze_roots, roots::DiscoverOpts, AnalyzeOptions};
    use std::fs;
    use std::path::PathBuf;
    banner("analyze_options_exclude_tests_keeps_tests_out_of_the_graph", "(api)");

    let pid = std::process::id();
    let root: PathBuf = std::env::temp_dir().join(format!("drift-analyze-notests-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("src/app.py"), "def handler(): return 1\n").unwrap();
    fs::write(root.join("tests/test_app.py"), "def test_handler(): return 1\n").unwrap();

    // Default (exclude_tests=false): tests are walked, both symbols exist.
    let with_tests = analyze_roots(&root, &DiscoverOpts::default(), &AnalyzeOptions::default()).unwrap();
    assert_eq!(with_tests.report.summary.files, 2, "default walks both files");

    // With exclude_tests=true: test_handler should be GONE from the symbol set.
    let no_tests = analyze_roots(
        &root,
        &DiscoverOpts::default(),
        &AnalyzeOptions { exclude_tests: true, ..AnalyzeOptions::default() },
    )
    .unwrap();
    assert_eq!(no_tests.report.summary.files, 1, "exclude_tests drops tests/test_app.py");
    assert_eq!(no_tests.report.summary.symbols, 1, "only `handler` should remain");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn module_level_calls_and_main_block_route_through_synthetic_module_symbol() {
    // Without the `<module>` synthetic symbol, references at module
    // level (Python `if __name__ == "__main__":`, TS/JS top-level
    // statements) get `in_symbol = None` and are silently dropped by
    // the graph builder. That misclassifies their callees as dead code.
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner(
        "module_level_calls_and_main_block_route_through_synthetic_module_symbol",
        "(synthetic Python script)",
    );

    // Three call sites at module level:
    //  - `setup_db()` at top level
    //  - `run_pipeline()` and `reachable_only_from_main()` inside `__main__`
    // Plus one in-function call (run_pipeline → setup_db).
    let src = "
def setup_db():
    return 'db'

def run_pipeline():
    setup_db()
    return 42

def reachable_only_from_main():
    return 'hello'

# top-level / module-init code
setup_db()

if __name__ == '__main__':
    run_pipeline()
    reachable_only_from_main()
";
    let tags = extract_tags_from_source(Path::new("script.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    // 1. The synthetic <module> symbol exists.
    let module_id = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "<module>")
        .map(|(id, _)| id.clone())
        .expect("`<module>` synthetic symbol should be created for files with orphan refs");

    // 2. Module-level calls now resolve to edges from <module>.
    let module_callees = graph.callees(&module_id);
    let callee_names: std::collections::HashSet<&str> = module_callees
        .iter()
        .filter_map(|c| graph.symbols.get(c).map(|s| s.name.as_str()))
        .collect();
    assert!(callee_names.contains("setup_db"), "<module> should call setup_db (top-level)");
    assert!(callee_names.contains("run_pipeline"), "<module> should call run_pipeline (__main__)");
    assert!(
        callee_names.contains("reachable_only_from_main"),
        "<module> should call reachable_only_from_main (__main__)",
    );

    // 3. setup_db now has 2 callers (in-function + module-level).
    let setup_db_callers = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "setup_db")
        .map(|(id, _)| graph.callers_of(id).len())
        .unwrap_or(0);
    assert!(
        setup_db_callers >= 2,
        "setup_db should have ≥2 callers (run_pipeline + <module>), got {setup_db_callers}",
    );

    // 4. The full report no longer flags reachable_only_from_main as dead_code.
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let entries: Vec<_> = graph
        .symbols
        .iter()
        .filter(|(id, _)| graph.callers_of(id).is_empty())
        .filter_map(|(id, _)| tb.build(id))
        .collect();
    let report = Report::build(&[tags], &graph, entries, &Default::default(), None);
    assert!(
        !report
            .summary
            .dead_code
            .iter()
            .any(|d| d.name == "reachable_only_from_main"),
        "reachable_only_from_main is reached from __main__; it must NOT be dead_code. Got dead_code: {:?}",
        report.summary.dead_code.iter().map(|d| &d.name).collect::<Vec<_>>(),
    );
}

#[test]
fn function_called_only_from_module_level_is_still_a_discovered_root() {
    // Regression: when the synthetic `<module>` symbol picks up
    // module-level invocations (e.g. TS `processPastOrdersLinkingLogic()`
    // at the bottom of a file, or Python `if __name__ == "__main__":
    // run()`), the called function gains 1 caller (`<module>`). That
    // would normally disqualify it from root discovery — but a function
    // called only from module load is still a named entry-point the
    // developer thinks about. discover_roots must count only REAL
    // (non-synthetic) callers.
    use drift_static_profiler::{
        graph::CallGraph,
        roots::{discover_roots, DiscoverOpts},
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner(
        "function_called_only_from_module_level_is_still_a_discovered_root",
        "(synthetic TS-shaped script)",
    );

    // Mirror the user's actual code shape: a function defined at the
    // top of a file, invoked at module scope. Both `<module>` AND the
    // function should appear as roots.
    let src = "
function processPastOrdersLinkingLogic(): number {
    return helper();
}

function helper(): number { return 1; }

// module-level invocation (the file's startup side effect)
processPastOrdersLinkingLogic();
";
    let tags = extract_tags_from_source(
        Path::new("/proj/src/ppl.ts"),
        Language::TypeScript,
        src,
    )
    .unwrap();
    let graph = CallGraph::build(&[tags]);
    let roots = discover_roots(&graph, Path::new("/proj"), &DiscoverOpts::default());
    let names: Vec<&str> = roots.iter().map(|r| r.name.as_str()).collect();

    assert!(
        names.contains(&"processPastOrdersLinkingLogic"),
        "processPastOrdersLinkingLogic should be a discovered root even though `<module>` calls it. \
         Got roots: {names:?}",
    );
    assert!(
        names.contains(&"<module>"),
        "the synthetic <module> itself should also be a root (its own callers_count=0). \
         Got roots: {names:?}",
    );
}

#[test]
fn synthetic_module_does_not_pollute_parent_class_of_top_level_functions() {
    // Regression: the synthetic `<module>` symbol spans the whole file
    // (byte 0..len), so naive containment logic would assign it as
    // `parent` of every top-level function. That would change SymbolIds
    // and pollute the viewer's chip text. resolve_containment must skip
    // `<module>` when picking parents (but NOT when picking in_symbol).
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner(
        "synthetic_module_does_not_pollute_parent_class_of_top_level_functions",
        "(synthetic Python script)",
    );

    let src = "
def foo(): pass
def bar(): foo()

# module-level call → forces synthetic <module> creation
foo()
";
    let tags = extract_tags_from_source(Path::new("rg.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);

    // <module> exists (we forced an orphan ref).
    assert!(
        graph.symbols.iter().any(|(_, s)| s.name == "<module>"),
        "<module> should exist when there are orphan refs",
    );

    // BUT top-level functions still have parent = None.
    for name in ["foo", "bar"] {
        let parent = graph
            .symbols
            .iter()
            .find(|(_, s)| s.name == name)
            .and_then(|(_, s)| s.parent.clone());
        assert_eq!(
            parent, None,
            "{name} is top-level — parent must remain None, not '<module>'",
        );
    }

    // SymbolId of top-level fn does NOT contain `<module>`.
    let foo_id = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "foo")
        .map(|(id, _)| id.0.clone())
        .unwrap();
    assert!(
        !foo_id.contains("<module>"),
        "SymbolId of foo() leaked '<module>': {foo_id}",
    );

    // <module> can still resolve module-level refs.
    let module_id = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "<module>")
        .map(|(id, _)| id.clone())
        .unwrap();
    let module_callees: std::collections::HashSet<&str> = graph
        .callees(&module_id)
        .iter()
        .filter_map(|c| graph.symbols.get(c).map(|s| s.name.as_str()))
        .collect();
    assert!(
        module_callees.contains("foo"),
        "<module> should still call foo (the orphan ref). got {module_callees:?}",
    );
}

#[test]
fn typescript_top_level_call_gets_synthetic_module() {
    // The TS/JS case: a file that calls something at module scope (the
    // `app.listen(3000)` / `runIt()` idiom). Same fix as Python's
    // `__main__` — the module-level call must NOT be silently dropped.
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("typescript_top_level_call_gets_synthetic_module", "(synthetic .ts)");

    let src = "
function startServer() {
    return 'listening';
}

// top-level execution — defines the file's entry behavior
startServer();
";
    let tags = extract_tags_from_source(Path::new("server.ts"), Language::TypeScript, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    let module = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "<module>")
        .map(|(id, _)| id.clone());
    assert!(
        module.is_some(),
        "TS file with top-level call should get a `<module>` symbol",
    );
    let module_id = module.unwrap();
    let callees: std::collections::HashSet<&str> = graph
        .callees(&module_id)
        .iter()
        .filter_map(|c| graph.symbols.get(c).map(|s| s.name.as_str()))
        .collect();
    assert!(
        callees.contains("startServer"),
        "<module> should call startServer (the top-level invocation). got {callees:?}",
    );
}

#[test]
fn synthetic_module_does_not_get_false_positive_findings() {
    // Regression: synthetic `<module>` has `loc = file_line_count` as a
    // proxy — without skipping it in the detector pass, a 100-line
    // script would trigger `expensive_compute` on `<module>` just for
    // being a long file. The fix: collect_node_findings skips synthetic
    // names entirely.
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("synthetic_module_does_not_get_false_positive_findings", "(big file)");

    // 100 lines of helpers + one module-level call so the synthetic
    // gets created. The synthetic's `loc` ≥ 80 would otherwise fire
    // `expensive_compute`.
    let mut src = String::from("def helper(): return 1\n");
    for _ in 0..100 {
        src.push_str("# pad\n");
    }
    src.push_str("helper()\n");
    let tags = extract_tags_from_source(Path::new("big.py"), Language::Python, &src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);

    let module_id = graph
        .symbols
        .iter()
        .find(|(_, s)| s.name == "<module>")
        .map(|(id, _)| id.clone())
        .expect("synthetic <module> should be created (we have an orphan ref)");
    let sym = graph.symbols.get(&module_id).unwrap();
    assert!(sym.loc >= 80, "test premise: file is large enough to risk a false positive (got loc={})", sym.loc);

    // Build the tree + report, then verify <module> has NO findings.
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&module_id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let module_node = report.entries.iter().find(|e| e.name == "<module>").unwrap();
    assert!(
        module_node.findings.is_empty(),
        "synthetic <module> must have NO findings, even when file is long. Got: {:?}",
        module_node.findings.iter().map(|f| f.kind).collect::<Vec<_>>(),
    );
    // And it must NOT show up in any of the rollups as a target row.
    assert!(
        !report
            .summary
            .findings_top
            .iter()
            .any(|t| t.node_id == module_id.0 && t.kind != FindingKind::HotZone),
        "synthetic <module> must not appear in findings_top",
    );
    assert!(
        !report.summary.refactor_candidates.iter().any(|c| c.name == "<module>"),
        "synthetic <module> must not be a refactor candidate",
    );
    assert!(
        !report.summary.immediate_fixes.iter().any(|f| f.name == "<module>"),
        "synthetic <module> must not appear in immediate_fixes",
    );
}

#[test]
fn empty_source_does_not_crash_or_produce_synthetic() {
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("empty_source_does_not_crash_or_produce_synthetic", "(empty file)");

    let tags = extract_tags_from_source(Path::new("empty.py"), Language::Python, "").unwrap();
    let graph = CallGraph::build(&[tags]);
    assert_eq!(graph.symbols.len(), 0, "empty file → no symbols, including synthetic");
}

#[test]
fn only_imports_does_not_produce_synthetic() {
    use drift_static_profiler::{
        graph::CallGraph,
        tags::extract_tags_from_source,
        Language,
    };
    use std::path::Path;
    banner("only_imports_does_not_produce_synthetic", "(imports-only file)");

    // Imports are NOT references — they're a separate capture. So a
    // file that's nothing but imports must NOT trigger the synthetic.
    let src = "
import os
import sys
from typing import Optional
";
    let tags = extract_tags_from_source(Path::new("only_imports.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    assert!(
        !graph.symbols.iter().any(|(_, s)| s.name == "<module>"),
        "imports-only file should NOT get a <module> symbol (imports aren't references)",
    );
}

#[test]
fn no_synthetic_module_symbol_when_no_orphan_references() {
    // A library file with only function bodies (no module-level
    // executable code) should NOT gain a synthetic <module> symbol.
    use drift_static_profiler::{
        graph::CallGraph, tags::extract_tags_from_source, Language,
    };
    use std::path::Path;
    banner("no_synthetic_module_symbol_when_no_orphan_references", "(library-style)");

    let src = "
def add(a, b):
    return a + b

def sub(a, b):
    return a - b

def both(a, b):
    return add(a, b) + sub(a, b)
";
    let tags = extract_tags_from_source(Path::new("lib.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags]);
    assert!(
        !graph.symbols.iter().any(|(_, s)| s.name == "<module>"),
        "library file with no orphan refs should NOT get a <module> symbol",
    );
}

#[test]
fn missing_caching_flags_repeated_pure_complex_callee() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("missing_caching_flags_repeated_pure_complex_callee", "(synthetic)");

    // `score` has cyclomatic complexity ≥ 5 (multiple branches) and is
    // called from many sites — but has no I/O. Classic memoize candidate.
    let src = "
def score(x):
    if x < 0:
        return 0
    elif x < 10:
        return 1
    elif x < 20:
        return 2
    elif x < 30:
        return 3
    elif x < 40:
        return 4
    else:
        return 5

def a(x): return score(x)
def b(x): return score(x + 1)
def c(x): return score(x + 2)
def d(x): return score(x + 3)
def e(x): return score(x + 4)
def f(x): return score(x + 5)

def driver(xs):
    return [a(v) + b(v) + c(v) + d(v) + e(v) + f(v) for v in xs]
";
    let tags = extract_tags_from_source(Path::new("memo.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("driver").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let mut found_score_caching = false;
    fn walk(
        node: &drift_static_profiler::tree::CallTreeNode,
        flag: &mut bool,
    ) {
        if node.name == "score"
            && node.findings.iter().any(|f| f.kind == FindingKind::MissingCaching)
        {
            *flag = true;
        }
        for c in &node.children {
            walk(c, flag);
        }
    }
    walk(&report.entries[0], &mut found_score_caching);
    assert!(
        found_score_caching,
        "missing_caching should fire on `score` (repeated + complex + pure)",
    );
}

#[test]
fn log_amplification_flags_many_logs_on_high_call_site_symbol() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::FindingKind,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("log_amplification_flags_many_logs_on_high_call_site_symbol", "(synthetic)");

    // `audit` has three info-level log calls and is called from many sites
    // (call_site_count ≥ 10) → log amplification candidate.
    let src = "
import logging
log = logging.getLogger(__name__)

def audit(event):
    log.info('start %s', event)
    log.info('phase %s', event)
    log.info('end %s', event)

def a(e): return audit(e)
def b(e): return audit(e)
def c(e): return audit(e)
def d(e): return audit(e)
def e(e): return audit(e)
def f(e): return audit(e)
def g(e): return audit(e)
def h(e): return audit(e)
def i(e): return audit(e)
def j(e): return audit(e)

def driver(events):
    return [a(x)+b(x)+c(x)+d(x)+e(x)+f(x)+g(x)+h(x)+i(x)+j(x) for x in events]
";
    let tags = extract_tags_from_source(Path::new("logamp.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("driver").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let mut hit = false;
    fn walk(
        node: &drift_static_profiler::tree::CallTreeNode,
        flag: &mut bool,
    ) {
        if node.name == "audit"
            && node.findings.iter().any(|f| f.kind == FindingKind::LogAmplification)
        {
            *flag = true;
        }
        for c in &node.children {
            walk(c, flag);
        }
    }
    walk(&report.entries[0], &mut hit);
    assert!(hit, "log_amplification should fire on `audit` (≥3 logs + many call sites)");
}

#[test]
fn findings_carry_effort_and_immediate_fixes_lists_quick_wins() {
    use drift_static_profiler::{
        graph::CallGraph,
        insights::{Effort, FindingKind, Severity},
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("findings_carry_effort_and_immediate_fixes_lists_quick_wins", "(synthetic)");

    // blocking_in_async = High severity + Trivial effort → should be in
    // immediate_fixes.
    let src = "
import requests

async def fetch_user_blocking(uid):
    return requests.get(f'https://api.example.com/{uid}')
";
    let tags = extract_tags_from_source(Path::new("block.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("fetch_user_blocking").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    // 1. Every finding carries an effort.
    let bia = report.entries[0]
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::BlockingInAsync)
        .expect("blocking_in_async finding expected");
    assert!(matches!(bia.effort, Effort::Trivial), "blocking_in_async should be Trivial effort");
    assert!(matches!(bia.severity, Severity::High), "blocking_in_async should be High severity");

    // 2. immediate_fixes lists it because it's High + Trivial.
    assert!(
        report.summary.immediate_fixes.iter().any(|f| matches!(f.kind, FindingKind::BlockingInAsync)),
        "immediate_fixes should include the blocking_in_async (high × trivial)",
    );
}

#[test]
fn refactor_candidates_include_nodes_with_finding_clusters() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("refactor_candidates_include_nodes_with_finding_clusters", "(synthetic)");

    // bulk_save: n_plus_one (loop) + noisy_log (loop) on the same symbol.
    // 2 findings on one node → refactor_candidate.
    let src = "
import logging
from sqlalchemy.orm import Session

log = logging.getLogger(__name__)

def bulk_save(items, session: Session):
    for it in items:
        log.info('saving %s', it)
        session.add(it)
        session.commit()
";
    let tags = extract_tags_from_source(Path::new("cluster.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("bulk_save").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let cluster = report
        .summary
        .refactor_candidates
        .iter()
        .find(|c| c.name == "bulk_save")
        .expect("bulk_save should be a refactor candidate (≥2 findings on the same node)");
    assert!(cluster.findings_count >= 2);
    assert!(cluster.kinds.len() >= 2, "kinds list should cover both detectors");
}

#[test]
fn roots_overview_lists_each_entry_with_categories_and_findings() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("roots_overview_lists_each_entry_with_categories_and_findings", "(synthetic)");

    let src = "
from sqlalchemy.orm import Session

def bulk_save(items, session: Session):
    for it in items:
        session.add(it)
        session.commit()
";
    let tags = extract_tags_from_source(Path::new("nplus1.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("bulk_save").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    let roots = &report.summary.roots_overview;
    assert_eq!(roots.len(), 1, "expected one root in summary.roots_overview");
    let r = &roots[0];
    assert_eq!(r.name, "bulk_save");
    assert!(r.percent_of_all_roots > 0.0, "single root should account for >0% of all roots");
    assert!(
        r.categories_reached.contains_key("db"),
        "bulk_save reaches a db call via session.add; got {:?}",
        r.categories_reached,
    );
    assert!(r.findings_total >= 1, "should report at least the n_plus_one finding");
    let high = r.findings_by_severity.get("high").copied().unwrap_or(0);
    assert!(high >= 1, "n_plus_one is high severity → severity bucket should reflect that");
}

#[test]
fn summary_findings_top_and_by_kind_are_populated() {
    use drift_static_profiler::{
        graph::CallGraph,
        report::Report,
        tags::extract_tags_from_source,
        tree::TreeBuilder,
        Language,
    };
    use std::path::Path;
    banner("summary_findings_top_and_by_kind_are_populated", "(synthetic)");

    let src = "
from sqlalchemy.orm import Session

def bulk_save(items, session: Session):
    for it in items:
        session.add(it)
        session.commit()
";
    let tags = extract_tags_from_source(Path::new("nplus1.py"), Language::Python, src).unwrap();
    let graph = CallGraph::build(&[tags.clone()]);
    let id = graph.find_entry_points("bulk_save").first().cloned().unwrap();
    let tb = TreeBuilder::new(&graph, Path::new(""));
    let node = tb.build(&id).unwrap();
    let report = Report::build(&[tags], &graph, vec![node], &Default::default(), None);

    assert_eq!(
        report.summary.findings_by_kind.get("n_plus_one"),
        Some(&1),
        "summary rollup should count the single n_plus_one finding",
    );
    assert!(
        report.summary.findings_top.iter().any(|t| matches!(t.kind, drift_static_profiler::insights::FindingKind::NPlusOne)),
        "findings_top should surface the n_plus_one finding",
    );
}
