//! Fuzzy join between a sampled (dynamic) profile and a static profile.
//!
//! A dynamic profile carries every frame the sampler saw — framework
//! code, stdlib, site-packages, user code, frozen importlib bootstrap.
//! A static profile only knows the source the analyzer scanned (`/app/`,
//! `src/`, etc.). The overlap is by construction partial — most dynamic
//! frames have no static counterpart and stay un-joined.
//!
//! Even within the overlap, naive equality on `node_id` is too brittle:
//!   - Path conventions drift across environments. A container session
//!     emits `/app/orders.py` but the static profile may have scanned
//!     the source tree as `orders.py` or `services/orders.py`.
//!   - Python <3.11 has no `co_qualname`, so sampled frames may lack
//!     the class prefix the static node carries.
//!   - Two classes can have a method with the same `name`. Only the
//!     qualified_name (or the parent_class) disambiguates.
//!
//! This module suggests the best static match per sampled node with
//! a confidence score (0.0 - 1.0) and a human-readable reason. The
//! viewer renders the suggestion as "we joined this row with X (95 %
//! sure, same file + same qualified_name)" and lets the user
//! confirm / override.
//!
//! ## Scoring tiers (descending confidence)
//!
//! | Tier | Rule                                                     | Score |
//! |------|----------------------------------------------------------|-------|
//! | 1    | `node_id` equality                                       | 1.00  |
//! | 2    | same file + same qualified_name                          | 0.95  |
//! | 3    | same file basename + same qualified_name                 | 0.85  |
//! | 4    | same file + same parent_class + same name                | 0.85  |
//! | 5    | same file + same name                                    | 0.70  |
//! | 6    | same file basename + same parent_class + same name       | 0.65  |
//! | 7    | same file basename + same name                           | 0.50  |
//! | -    | otherwise                                                | 0.00  |
//!
//! Only candidates with score ≥ [`MIN_CONFIDENCE`] are surfaced. Below
//! that, "un-joined" is the safer answer — better to show a row with
//! only dynamic data than to attach the wrong static metadata to it.
//!
//! The matcher is symmetric in inputs but asymmetric in intent: for
//! each *sampled* node, we look up the best *static* candidate. This
//! is the natural direction because the dynamic profile has many
//! more frames than the static one; the static set is the search
//! space.

use std::collections::HashMap;

/// Confidence threshold. Candidates below this are not surfaced — the
/// matcher returns `None` rather than attach a low-confidence static
/// node to dynamic data.
pub const MIN_CONFIDENCE: f32 = 0.5;

/// Minimum information a node needs for matching. Both sampled and
/// static nodes use the same field set; we use distinct type
/// names so the call sites read "match these sampled nodes against
/// these static ones" without ambiguity.
///
/// Field shapes mirror the join keys we emit on both sides:
///   - `node_id` is the canonical `file::class::name` (F3 / static schema).
///   - `name` is the bare function name (`create`).
///   - `file` is the file path. The sampler emits container paths
///     (`/app/orders.py`); the static profiler emits source-tree paths.
///   - `qualified_name` is the class-qualified name (`OrderService.create`).
///     Optional — Py 3.7-3.10 sampled nodes don't have it.
///   - `parent_class` is the enclosing class name (`OrderService`).
///     Optional for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SampledNode {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub qualified_name: Option<String>,
    pub parent_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StaticNode {
    pub node_id: String,
    pub name: String,
    pub file: String,
    pub qualified_name: Option<String>,
    pub parent_class: Option<String>,
}

/// One ranked candidate match. The matcher returns up to one
/// `best_match` + up to three `alternatives` per sampled node;
/// callers can show the alternatives in a "confirm / pick another"
/// affordance for ambiguous cases.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinCandidate {
    pub static_node_id: String,
    pub static_qualified_name: Option<String>,
    pub static_file: String,
    pub confidence: f32,
    pub reason: String,
}

/// Result for one sampled node: the best static candidate (or None
/// if nothing scored above [`MIN_CONFIDENCE`]) plus up to three
/// alternatives sorted by score descending.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinSuggestion {
    pub sampled_node_id: String,
    pub sampled_file: String,
    pub sampled_qualified_name: Option<String>,
    pub best_match: Option<JoinCandidate>,
    pub alternatives: Vec<JoinCandidate>,
}

impl JoinSuggestion {
    pub fn is_joined(&self) -> bool {
        self.best_match.is_some()
    }
}

/// Suggest a static join target for each sampled node. The returned
/// `Vec` is parallel to `sampled` — one suggestion per input, in the
/// same order. Suggestions whose `best_match` is `None` represent
/// "un-joined" rows (the viewer should show only dynamic data).
pub fn suggest_joins(
    sampled: &[SampledNode],
    static_: &[StaticNode],
) -> Vec<JoinSuggestion> {
    // Build two indexes:
    //   - by_id   — exact node_id lookup (Tier 1 hot path).
    //   - by_name — all static nodes grouped by their bare function name.
    //               Most fuzzy work happens within a name-group so we
    //               never scan the whole static profile per sampled node.
    //               That keeps the matcher O(n_sampled + total_static)
    //               instead of O(n_sampled × n_static).
    let by_id: HashMap<&str, &StaticNode> = static_
        .iter()
        .map(|n| (n.node_id.as_str(), n))
        .collect();
    let mut by_name: HashMap<&str, Vec<&StaticNode>> = HashMap::new();
    for n in static_ {
        by_name.entry(n.name.as_str()).or_default().push(n);
    }

    sampled.iter().map(|s| suggest_one(s, &by_id, &by_name)).collect()
}

fn suggest_one(
    s: &SampledNode,
    by_id: &HashMap<&str, &StaticNode>,
    by_name: &HashMap<&str, Vec<&StaticNode>>,
) -> JoinSuggestion {
    // Tier 1 — exact node_id match. By far the common case once both
    // profiles agree on a file-path convention; short-circuit so we
    // don't pay the per-name fuzzy walk.
    if let Some(stat) = by_id.get(s.node_id.as_str()) {
        return JoinSuggestion {
            sampled_node_id: s.node_id.clone(),
            sampled_file: s.file.clone(),
            sampled_qualified_name: s.qualified_name.clone(),
            best_match: Some(JoinCandidate {
                static_node_id: stat.node_id.clone(),
                static_qualified_name: stat.qualified_name.clone(),
                static_file: stat.file.clone(),
                confidence: 1.0,
                reason: "exact node_id match".into(),
            }),
            alternatives: Vec::new(),
        };
    }

    // Tier 2-7 — score every static node sharing the sampled node's
    // bare name. Empty if no candidate has a matching name; then we
    // also try a class-prefix search via parent_class on a separate
    // pass below (handles "sampled has qualname class.method, static
    // has only name + parent_class" mismatches).
    let candidates: Vec<&StaticNode> =
        by_name.get(s.name.as_str()).cloned().unwrap_or_default();

    let mut scored: Vec<(f32, String, &StaticNode)> = candidates
        .iter()
        .map(|stat| {
            let (score, reason) = score_pair(s, stat);
            (score, reason, *stat)
        })
        .filter(|(score, _, _)| *score >= MIN_CONFIDENCE)
        .collect();

    // Descending by score. Ties broken by static node_id for
    // determinism — the matcher must be reproducible across runs.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.2.node_id.cmp(&b.2.node_id))
    });

    let best_match = scored.first().map(|(score, reason, stat)| JoinCandidate {
        static_node_id: stat.node_id.clone(),
        static_qualified_name: stat.qualified_name.clone(),
        static_file: stat.file.clone(),
        confidence: *score,
        reason: reason.clone(),
    });
    let alternatives: Vec<JoinCandidate> = scored
        .iter()
        .skip(1)
        .take(3)
        .map(|(score, reason, stat)| JoinCandidate {
            static_node_id: stat.node_id.clone(),
            static_qualified_name: stat.qualified_name.clone(),
            static_file: stat.file.clone(),
            confidence: *score,
            reason: reason.clone(),
        })
        .collect();

    JoinSuggestion {
        sampled_node_id: s.node_id.clone(),
        sampled_file: s.file.clone(),
        sampled_qualified_name: s.qualified_name.clone(),
        best_match,
        alternatives,
    }
}

/// Score one (sampled, static) pair. Returns `(score, reason)`. Scores
/// are computed top-down through the tier table — the first matching
/// tier wins, so a Tier-2 result short-circuits the Tier-5 check.
fn score_pair(s: &SampledNode, t: &StaticNode) -> (f32, String) {
    let same_file = !s.file.is_empty() && s.file == t.file;
    let sb = basename(&s.file);
    let tb = basename(&t.file);
    let same_basename = !sb.is_empty() && sb == tb;

    let qn_match = s
        .qualified_name
        .as_deref()
        .filter(|q| !q.is_empty())
        .zip(t.qualified_name.as_deref().filter(|q| !q.is_empty()))
        .map(|(a, b)| a == b)
        .unwrap_or(false);

    let class_match = s
        .parent_class
        .as_deref()
        .filter(|c| !c.is_empty())
        .zip(t.parent_class.as_deref().filter(|c| !c.is_empty()))
        .map(|(a, b)| a == b)
        .unwrap_or(false);

    let name_match = !s.name.is_empty() && s.name == t.name;

    // Tier 2 — same file + same qualified_name.
    if same_file && qn_match {
        return (0.95, "same file + same qualified_name".into());
    }
    // Tier 3 — same basename + same qualified_name. Handles container
    // path (`/app/orders.py`) vs source-tree path (`orders.py`).
    if same_basename && qn_match {
        return (0.85, "same basename + same qualified_name".into());
    }
    // Tier 4 — same file + same class + same name. Used when sampled
    // has no qualified_name (Py 3.7-3.10) but static has both. Falls
    // back to (parent_class, name) which still disambiguates two
    // classes with same method name in the same file.
    if same_file && class_match && name_match {
        return (0.85, "same file + same class + same name".into());
    }
    // Tier 5 — same file + same name. Solid for free functions or
    // when the file has only one symbol with that name.
    if same_file && name_match {
        return (0.7, "same file + same name".into());
    }
    // Tier 6 — same basename + same class + same name.
    if same_basename && class_match && name_match {
        return (0.65, "same basename + same class + same name".into());
    }
    // Tier 7 — same basename + same name. Last useful tier.
    if same_basename && name_match {
        return (0.5, "same basename + same name".into());
    }
    (0.0, "no reasonable similarity".into())
}

fn basename(path: &str) -> &str {
    // splitn keeps the right-hand suffix after the last '/'. Handles
    // both POSIX (`/app/orders.py`) and bare basenames (`orders.py`).
    path.rsplit('/').next().unwrap_or(path)
}

// ===========================================================================
// Tests — runnable via `cargo test --lib fuzzy_join -- --nocapture`.
//
// The realistic-scenario test at the bottom prints a table; the per-tier
// tests are assertion-only. Running with --nocapture shows a worked
// example of every tier and the un-joined fallback.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Small fixture helpers — keep tests dense and readable.

    fn sampled(node_id: &str, name: &str, file: &str, qn: Option<&str>, cls: Option<&str>) -> SampledNode {
        SampledNode {
            node_id: node_id.into(),
            name: name.into(),
            file: file.into(),
            qualified_name: qn.map(String::from),
            parent_class: cls.map(String::from),
        }
    }
    fn static_(node_id: &str, name: &str, file: &str, qn: Option<&str>, cls: Option<&str>) -> StaticNode {
        StaticNode {
            node_id: node_id.into(),
            name: name.into(),
            file: file.into(),
            qualified_name: qn.map(String::from),
            parent_class: cls.map(String::from),
        }
    }

    // -------- per-tier tests --------

    #[test]
    fn tier_1_exact_node_id_match_returns_1_0() {
        let s = sampled(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let t = static_(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let out = suggest_joins(&[s], &[t]);
        let suggestion = &out[0];
        let best = suggestion.best_match.as_ref().unwrap();
        assert_eq!(best.confidence, 1.0);
        assert_eq!(best.reason, "exact node_id match");
        assert!(suggestion.alternatives.is_empty());
    }

    #[test]
    fn tier_2_same_file_same_qualname_returns_0_95() {
        // Same data but different node_id encodings — say one side
        // used "file::name" and the other "file::class::name". The
        // qualified_name comparison still pins them as the same symbol.
        let s = sampled(
            "/app/orders.py::create", // different id encoding
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let t = static_(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let out = suggest_joins(&[s], &[t]);
        let best = out[0].best_match.as_ref().unwrap();
        assert!((best.confidence - 0.95).abs() < 1e-6);
        assert_eq!(best.reason, "same file + same qualified_name");
    }

    #[test]
    fn tier_3_basename_match_handles_container_vs_source_paths() {
        // The realistic mismatch: container says /app/orders.py,
        // source tree says orders.py. Both refer to the same symbol.
        let s = sampled(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let t = static_(
            "orders.py::OrderService::create",
            "create", "orders.py",  // bare relative path
            Some("OrderService.create"), Some("OrderService"),
        );
        let out = suggest_joins(&[s], &[t]);
        let best = out[0].best_match.as_ref().unwrap();
        assert!((best.confidence - 0.85).abs() < 1e-6);
        assert_eq!(best.reason, "same basename + same qualified_name");
    }

    #[test]
    fn tier_4_py37_fallback_uses_parent_class_when_qualname_absent() {
        // Py 3.7-3.10 sampled frame: no qualified_name. We still
        // know the class via the static side's parent_class +
        // matching file + matching name.
        let s = sampled(
            "/app/orders.py::create",
            "create", "/app/orders.py",
            None,                // <-- no qualified_name on sampled
            Some("OrderService"),
        );
        let t = static_(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let out = suggest_joins(&[s], &[t]);
        let best = out[0].best_match.as_ref().unwrap();
        assert!((best.confidence - 0.85).abs() < 1e-6);
        assert_eq!(best.reason, "same file + same class + same name");
    }

    #[test]
    fn tier_5_same_file_same_name_for_free_function() {
        // Free function — no class on either side, no qualified_name.
        // Force a node_id mismatch so we exercise Tier 5 specifically
        // (without the mismatch, Tier 1 would win at 1.0).
        let s = sampled(
            "different-encoding-id",
            "compute", "/app/utils.py",
            None, None,
        );
        let t = static_(
            "/app/utils.py::compute",
            "compute", "/app/utils.py",
            None, None,
        );
        let out = suggest_joins(&[s], &[t]);
        let best = out[0].best_match.as_ref().unwrap();
        assert!((best.confidence - 0.7).abs() < 1e-6);
        assert_eq!(best.reason, "same file + same name");
    }

    #[test]
    fn tier_7_basename_and_name_only_just_clears_threshold() {
        // Worst case we still surface — basename + name match,
        // nothing else known.
        let s = sampled(
            "/runtime/agent/orders.py::create",
            "create", "/runtime/agent/orders.py",
            None, None,
        );
        let t = static_(
            "/project/src/orders.py::create",
            "create", "/project/src/orders.py",
            None, None,
        );
        let out = suggest_joins(&[s], &[t]);
        let best = out[0].best_match.as_ref().unwrap();
        assert!((best.confidence - 0.5).abs() < 1e-6);
        assert_eq!(best.reason, "same basename + same name");
    }

    #[test]
    fn no_match_when_filename_and_name_both_differ() {
        let s = sampled(
            "/usr/lib/python3.11/asyncio/runners.py::run",
            "run", "/usr/lib/python3.11/asyncio/runners.py",
            None, None,
        );
        // Static profile only has user code.
        let t = static_(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let out = suggest_joins(&[s], &[t]);
        assert!(out[0].best_match.is_none(), "stdlib frame must not join to user code");
        assert!(!out[0].is_joined());
    }

    #[test]
    fn ambiguous_create_disambiguated_by_qualified_name() {
        // Two static methods named `create` in different classes.
        // The sampled frame's qualified_name picks one.
        let s = sampled(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let order_create = static_(
            "/app/orders.py::OrderService::create",
            "create", "/app/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let customer_create = static_(
            "/app/customers.py::CustomerService::create",
            "create", "/app/customers.py",
            Some("CustomerService.create"), Some("CustomerService"),
        );
        let out = suggest_joins(&[s], &[order_create.clone(), customer_create.clone()]);
        let best = out[0].best_match.as_ref().unwrap();
        assert_eq!(best.static_node_id, order_create.node_id);
    }

    #[test]
    fn returns_alternatives_sorted_by_confidence() {
        // Sampled "create" in /app/orders.py. Two candidates:
        //   - the right OrderService.create (Tier 1, 1.0)
        //   - a CustomerService.create in /app/customers.py
        //     (Tier 7: same basename "create"? No — bases differ.)
        // To exercise alternatives, point the sampled frame to a
        // basename that matches BOTH candidates but pick the right
        // one via qualified_name match.
        let s = sampled(
            "non-matching-id",
            "create", "orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let same_basename_qn = static_(
            "/proj/src/orders.py::OrderService::create",
            "create", "/proj/src/orders.py",
            Some("OrderService.create"), Some("OrderService"),
        );
        let same_basename_name = static_(
            "/proj/src/orders.py::Other::create",
            "create", "/proj/src/orders.py",
            Some("Other.create"), Some("Other"),
        );
        let out = suggest_joins(&[s], &[same_basename_qn.clone(), same_basename_name.clone()]);
        let best = out[0].best_match.as_ref().unwrap();
        // Best is the qualified_name match (Tier 3 — basename + qn).
        assert_eq!(best.static_node_id, same_basename_qn.node_id);
        // Alternative is the same-basename-same-name (Tier 7).
        assert_eq!(out[0].alternatives.len(), 1);
        assert_eq!(out[0].alternatives[0].static_node_id, same_basename_name.node_id);
        // Alternatives are sorted by score descending; sole alternative
        // here is the lower one.
        assert!(out[0].alternatives[0].confidence < best.confidence);
    }

    // -------- The "test script" — realistic scenario, prints a table ----

    #[test]
    fn demo_realistic_join_scenario() {
        // Mix of frames the way they'd actually appear in events.log:
        //   - 3 user-code frames at varying confidence levels
        //   - 1 sampler-without-class (Py 3.7-3.10) frame
        //   - 2 system frames that should never join
        //   - 1 ambiguous "create" frame
        let sampled = vec![
            sampled(
                "/app/orders.py::OrderService::create",
                "create", "/app/orders.py",
                Some("OrderService.create"), Some("OrderService"),
            ),
            sampled(
                "/app/orders.py::ship",
                "ship", "/app/orders.py",
                Some("OrderService.ship"), Some("OrderService"),
            ),
            sampled(
                "py37-no-qualname-id",
                "charge", "/app/orders.py",
                None,                  // simulate Py 3.7 — no co_qualname
                None,
            ),
            sampled(
                "<frozen importlib._bootstrap>::_call_with_frames_removed",
                "_call_with_frames_removed",
                "<frozen importlib._bootstrap>",
                None, None,
            ),
            sampled(
                "/usr/local/lib/python3.7/site-packages/uvicorn/server.py::serve",
                "serve",
                "/usr/local/lib/python3.7/site-packages/uvicorn/server.py",
                Some("Server.serve"), Some("Server"),
            ),
            sampled(
                "/app/orders.py::ambiguous_method",
                "create", "/app/orders.py",   // same file, name, no qualname
                None, None,
            ),
        ];
        // Static profile scanned `/app/` only — no stdlib, no
        // site-packages. Includes TWO `create` methods inside
        // /app/orders.py so the ambiguous-without-qualname case
        // below actually surfaces an alternative.
        let static_ = vec![
            static_(
                "/app/orders.py::OrderService::create",
                "create", "/app/orders.py",
                Some("OrderService.create"), Some("OrderService"),
            ),
            static_(
                "/app/orders.py::OrderItem::create",
                "create", "/app/orders.py",
                Some("OrderItem.create"), Some("OrderItem"),
            ),
            static_(
                "/app/orders.py::OrderService::ship",
                "ship", "/app/orders.py",
                Some("OrderService.ship"), Some("OrderService"),
            ),
            static_(
                "/app/orders.py::OrderService::charge",
                "charge", "/app/orders.py",
                Some("OrderService.charge"), Some("OrderService"),
            ),
            static_(
                "/app/customers.py::CustomerService::create",
                "create", "/app/customers.py",
                Some("CustomerService.create"), Some("CustomerService"),
            ),
        ];
        let out = suggest_joins(&sampled, &static_);

        // Pretty-print the results so `cargo test -- --nocapture`
        // produces a worked example a human can read.
        let bar = "─".repeat(110);
        println!("\n{bar}");
        println!("Fuzzy-join demo: {} sampled nodes against {} static nodes",
                 sampled.len(), static_.len());
        println!("{bar}");
        println!("{:<46} {:<6} {:<46}  {}", "sampled", "score", "joined to", "reason");
        println!("{bar}");
        for (i, sug) in out.iter().enumerate() {
            let sampled_label = trunc(&sampled[i].node_id, 46);
            match &sug.best_match {
                Some(m) => println!(
                    "{:<46} {:<6.2} {:<46}  {}",
                    sampled_label, m.confidence, trunc(&m.static_node_id, 46), m.reason
                ),
                None => println!(
                    "{:<46} {:<6} {:<46}  {}",
                    sampled_label, "—", "(un-joined)", "no static match within scope"
                ),
            }
            for alt in &sug.alternatives {
                println!(
                    "{:<46} {:<6.2}   ↳ alt: {:<40}  {}",
                    "", alt.confidence, trunc(&alt.static_node_id, 40), alt.reason
                );
            }
        }
        let joined = out.iter().filter(|s| s.is_joined()).count();
        println!("{bar}");
        println!(
            "Summary: {} of {} sampled nodes joined ({} un-joined → viewer shows dynamic data only)",
            joined, out.len(), out.len() - joined,
        );
        println!("{bar}");

        // Hard assertions on what the demo above demonstrated.

        // 1. OrderService.create — exact match, 1.0.
        assert_eq!(out[0].best_match.as_ref().unwrap().confidence, 1.0);
        assert_eq!(out[0].best_match.as_ref().unwrap().reason, "exact node_id match");

        // 2. ship — node_id differs (sampled was `/app/orders.py::ship`,
        //    static is `/app/orders.py::OrderService::ship`), but
        //    qualified_name matches → Tier 2 at 0.95.
        let ship = out[1].best_match.as_ref().unwrap();
        assert!((ship.confidence - 0.95).abs() < 1e-6);

        // 3. charge — Py 3.7-style: no qualname, no class on sampled.
        //    Falls back to same file + same name (Tier 5, 0.7).
        let charge = out[2].best_match.as_ref().unwrap();
        assert!((charge.confidence - 0.7).abs() < 1e-6);

        // 4. frozen importlib — no static counterpart, un-joined.
        assert!(out[3].best_match.is_none());

        // 5. uvicorn — site-packages, un-joined.
        assert!(out[4].best_match.is_none());

        // 6. ambiguous "create" without class — TWO same-file
        //    same-name candidates in /app/orders.py (OrderService.create
        //    and OrderItem.create) both score 0.7. Without a qualname
        //    to disambiguate, the matcher picks the alphabetically-first
        //    by static node_id (deterministic tie-break — see the
        //    `sort_by` in `suggest_one`) and surfaces the other as an
        //    alternative. The CustomerService.create candidate is in
        //    /app/customers.py so its basename differs and it scores
        //    0.0 (not surfaced).
        let ambig = out[5].best_match.as_ref().unwrap();
        assert!((ambig.confidence - 0.7).abs() < 1e-6);
        // Deterministic tie-break: "OrderItem" < "OrderService" lexically,
        // so OrderItem::create wins on ties.
        assert_eq!(ambig.static_node_id, "/app/orders.py::OrderItem::create");
        // The alternative is the other same-file same-name candidate.
        assert_eq!(out[5].alternatives.len(), 1);
        assert!((out[5].alternatives[0].confidence - 0.7).abs() < 1e-6);
        assert_eq!(
            out[5].alternatives[0].static_node_id,
            "/app/orders.py::OrderService::create"
        );
    }

    fn trunc(s: &str, max: usize) -> String {
        if s.len() <= max {
            return s.to_string();
        }
        format!("…{}", &s[s.len() - (max - 1)..])
    }
}
