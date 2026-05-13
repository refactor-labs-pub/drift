//! Power-iteration PageRank with early-exit on convergence.
//!
//! Replaces `petgraph::algo::page_rank`, which runs a fixed iteration
//! count with no convergence check. That left us trading accuracy for
//! speed via the `iter_count` knob — 30 iters was fast but only
//! 0.85³⁰ ≈ 7.6×10⁻³ accurate, which is enough for top-N ordering but
//! visibly loose at the p90 cutoff that gates `hot_zone` /
//! `log_amplification` / severity-bump findings.
//!
//! Strategy: stop when the rank vector stops moving. We use the L1
//! norm of `||r_k - r_{k-1}||₁` against `tol = 1e-6` (the NetworkX
//! default — the de-facto industry standard for PageRank tolerance).
//!
//! Why this gives both accuracy and speed:
//!
//! * **Accuracy**: at tol=1e-6 the residual from the true fixed point
//!   is bounded by `tol × α / (1−α) ≈ 5.7×10⁻⁶` — three orders of
//!   magnitude tighter than the fixed-30 path, and within striking
//!   distance of fixed-100's ≈ 1.7×10⁻⁷. The hot-zone cutoff has
//!   per-node gaps in the 10⁻⁴–10⁻³ range on typical graphs, so 5.7×10⁻⁶
//!   precision never flips a classification.
//!
//! * **Speed**: power iteration with damping α=0.85 contracts at rate
//!   α per step, so reaching tol=1e-6 takes at most `log(tol)/log(α) ≈
//!   85` iters in the worst case. On real graphs (well-mixing call
//!   graphs without near-disconnected components) convergence is
//!   usually hit in 25–50 iters. We hard-cap at `MAX_ITER = 100` so a
//!   pathological graph can't make the phase unbounded.
//!
//! Implementation niceties vs petgraph's `page_rank`:
//!   - Out-degree of each node is computed ONCE (petgraph re-counts on
//!     every iteration via `graph.edges(...).count()`).
//!   - The `new_ranks` buffer is allocated ONCE and swapped each iter
//!     instead of allocated fresh per iter (petgraph allocates).
//!   - Dangling-node detection happens once at startup; the per-iter
//!     dangling-sum loop is skipped entirely when there are none.
//!   - Returns `(ranks, iters_run)` so the caller can surface the
//!     iteration count for diagnostics — useful for noticing if a
//!     graph regresses to the max cap.

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

/// L1-norm convergence threshold. Mirrors NetworkX's
/// `nx.pagerank(tol=1e-6)` default.
pub const DEFAULT_TOL: f64 = 1e-6;

/// Safety cap so a degenerate graph can't spin forever. In practice
/// the geometric contraction at α=0.85 puts convergence at tol=1e-6
/// inside ~85 iters worst case.
pub const MAX_ITER: usize = 100;

/// Run power-iteration PageRank to convergence on a petgraph DiGraph.
///
/// Returns the rank vector (indexed by `NodeIndex::index()`) and the
/// number of iterations actually executed. The graph is borrowed
/// immutably; no graph rebuild is performed.
///
/// Behavior matches the standard Brin & Page formulation:
///   - Each node contributes `α × rank/out_deg` to its successors.
///   - Dangling nodes (out_deg == 0) redistribute their mass uniformly
///     via `α × dangling_sum / N`.
///   - Every node receives `(1−α)/N` teleport per step.
///
/// The initial rank vector is the uniform distribution `1/N`. With
/// damping α=0.85 the spectral gap guarantees convergence regardless
/// of graph structure (including disconnected components).
pub fn page_rank(
    g: &DiGraph<crate::graph::SymbolId, ()>,
    damping: f64,
    tol: f64,
    max_iter: usize,
) -> (Vec<f64>, usize) {
    let n = g.node_count();
    if n == 0 {
        return (Vec::new(), 0);
    }
    let n_f = n as f64;
    let teleport = (1.0 - damping) / n_f;
    let dangling_share = damping / n_f;

    // Pre-compute out-degree per node. petgraph's page_rank calls
    // `graph.edges(i).count()` on every iter — for max_iter iters
    // that's O(max_iter × N) extra walks over the edge index. One
    // pass here, then the iteration loop reads the result.
    let out_deg: Vec<usize> = (0..n)
        .map(|i| g.edges(NodeIndex::new(i)).count())
        .collect();
    let has_any_dangling = out_deg.iter().any(|&d| d == 0);

    let mut ranks: Vec<f64> = vec![1.0 / n_f; n];
    let mut next: Vec<f64> = vec![0.0; n];

    for iter in 1..=max_iter {
        // Reset the receive-buffer to the teleport floor every node
        // gets unconditionally. Writing in a tight loop is faster
        // than `vec![teleport; n]` (no allocation, no zeroing pass).
        for r in next.iter_mut() {
            *r = teleport;
        }

        // Push each node's mass to its callees. Dangling nodes
        // (no out-edges) accumulate into `dangling_sum`, which gets
        // redistributed uniformly at the end of the iteration.
        let mut dangling_sum = 0.0;
        for i in 0..n {
            let deg = out_deg[i];
            if deg == 0 {
                dangling_sum += ranks[i];
                continue;
            }
            let contrib = damping * ranks[i] / deg as f64;
            for edge in g.edges(NodeIndex::new(i)) {
                let target = edge.target().index();
                next[target] += contrib;
            }
        }

        if has_any_dangling {
            let dangling_contribution = dangling_share * dangling_sum;
            for r in next.iter_mut() {
                *r += dangling_contribution;
            }
        }

        // L1 convergence check. Done BEFORE the swap so we can read
        // both vectors directly without aliasing tricks.
        let delta: f64 = next
            .iter()
            .zip(ranks.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();

        std::mem::swap(&mut ranks, &mut next);

        if delta < tol {
            return (ranks, iter);
        }
    }

    (ranks, max_iter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::SymbolId;

    fn sym(s: &str) -> SymbolId {
        SymbolId(s.to_string())
    }

    /// Convenience: build a small DiGraph from `(src, dst)` pairs.
    fn build_graph(nodes: &[&str], edges: &[(&str, &str)]) -> DiGraph<SymbolId, ()> {
        let mut g: DiGraph<SymbolId, ()> = DiGraph::new();
        let mut idx: std::collections::HashMap<&str, NodeIndex> =
            std::collections::HashMap::new();
        for name in nodes {
            let n = g.add_node(sym(name));
            idx.insert(*name, n);
        }
        for (s, d) in edges {
            g.add_edge(idx[s], idx[d], ());
        }
        g
    }

    #[test]
    fn empty_graph_returns_empty() {
        let g: DiGraph<SymbolId, ()> = DiGraph::new();
        let (ranks, iters) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        assert!(ranks.is_empty());
        assert_eq!(iters, 0);
    }

    #[test]
    fn single_isolated_node_has_rank_one() {
        // One node, no edges — entire mass sits on it after teleport
        // + dangling redistribution stabilizes (which is immediate for
        // a single node).
        let g = build_graph(&["a"], &[]);
        let (ranks, _iters) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        assert_eq!(ranks.len(), 1);
        assert!(
            (ranks[0] - 1.0).abs() < 1e-9,
            "single-node rank should equal 1.0, got {}",
            ranks[0]
        );
    }

    #[test]
    fn symmetric_two_node_cycle_has_equal_ranks() {
        // a → b → a — by symmetry both ranks should be 0.5.
        let g = build_graph(&["a", "b"], &[("a", "b"), ("b", "a")]);
        let (ranks, _) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        assert!((ranks[0] - 0.5).abs() < 1e-5, "rank[0] = {}", ranks[0]);
        assert!((ranks[1] - 0.5).abs() < 1e-5, "rank[1] = {}", ranks[1]);
    }

    #[test]
    fn rank_mass_sums_to_one() {
        // Standard invariant: ranks form a probability distribution.
        // Holds for any graph since teleport + dangling redistribution
        // preserve mass.
        let g = build_graph(
            &["a", "b", "c", "d"],
            &[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d"), ("d", "a")],
        );
        let (ranks, _) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        let sum: f64 = ranks.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "rank mass should sum to 1.0, got {sum}"
        );
    }

    #[test]
    fn central_hub_outranks_periphery() {
        // Star graph: every leaf points at the hub. The hub should
        // have strictly higher rank than every leaf.
        let g = build_graph(
            &["hub", "a", "b", "c", "d"],
            &[("a", "hub"), ("b", "hub"), ("c", "hub"), ("d", "hub")],
        );
        let (ranks, _) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        // hub is node 0; leaves are 1..=4.
        for leaf in 1..5 {
            assert!(
                ranks[0] > ranks[leaf],
                "hub rank {} should exceed leaf rank {}",
                ranks[0],
                ranks[leaf]
            );
        }
    }

    #[test]
    fn converges_before_max_iter_on_typical_graph() {
        // On a small well-connected graph, convergence at tol=1e-6
        // should happen well inside MAX_ITER. If this regresses
        // suddenly something pathological is going on.
        let g = build_graph(
            &["a", "b", "c", "d", "e"],
            &[
                ("a", "b"), ("b", "c"), ("c", "d"), ("d", "e"), ("e", "a"),
                ("a", "c"), ("b", "d"), ("c", "e"),
            ],
        );
        let (_, iters) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        assert!(
            iters < MAX_ITER,
            "expected convergence before MAX_ITER, used {iters}/{MAX_ITER}"
        );
        // Geometric bound: should converge in tens of iters for α=0.85.
        assert!(
            iters < 80,
            "5-node graph took {iters} iters — investigate",
        );
    }

    #[test]
    fn satisfies_pagerank_fixed_point_equation() {
        // Rigorous self-check that doesn't couple to another impl:
        // assert that the returned `r` satisfies the PageRank fixed-
        // point equation directly. For each node i:
        //
        //     r[i] = (1−α)/N + α × Σ_{j → i} r[j] / out_deg[j]
        //                          (dangling mass added uniformly)
        //
        // No external reference needed; any conforming PageRank must
        // satisfy this within numerical tolerance. We run to a tighter
        // tol than DEFAULT to make the assertion meaningful.
        let g = build_graph(
            &["a", "b", "c", "d", "e", "f"],
            &[
                ("a", "b"), ("a", "c"),
                ("b", "d"), ("c", "d"),
                ("d", "e"), ("d", "f"),
                ("e", "a"), ("f", "a"),
            ],
        );
        let (r, _) = page_rank(&g, 0.85, 1e-12, 500);
        let n = g.node_count();
        let teleport = 0.15 / n as f64;

        // Precompute out-degrees.
        let out_deg: Vec<usize> = (0..n)
            .map(|i| g.edges(NodeIndex::new(i)).count())
            .collect();

        // Sum of dangling-node mass for the redistribution term. None
        // in this test graph, so it'll be zero — the assertion still
        // holds and the test exercises the no-dangling branch.
        let dangling_sum: f64 = (0..n).filter(|i| out_deg[*i] == 0).map(|i| r[i]).sum();
        let dangling_per_node = 0.85 * dangling_sum / n as f64;

        for i in 0..n {
            // Σ_{j → i} r[j] / out_deg[j]
            let inbound: f64 = (0..n)
                .filter(|j| {
                    out_deg[*j] > 0
                        && g.edges(NodeIndex::new(*j))
                            .any(|e| e.target().index() == i)
                })
                .map(|j| r[j] / out_deg[j] as f64)
                .sum();
            let predicted = teleport + 0.85 * inbound + dangling_per_node;
            assert!(
                (r[i] - predicted).abs() < 1e-9,
                "fixed-point violated at node {i}: r[{i}]={}, predicted={}, diff={}",
                r[i],
                predicted,
                (r[i] - predicted).abs(),
            );
        }
    }

    #[test]
    fn analytically_known_fixed_point_for_butterfly_graph() {
        // Cross-check: derive r_a for the same 6-node graph by hand
        // from the symmetry r_b = r_c, r_e = r_f, then verify the
        // implementation lands on the same value within tol's
        // residual bound (≈ tol × α / (1−α)).
        //
        // Symmetry reduces the system to:
        //   r_a = 0.025 + 1.7 × r_e
        //   r_b = 0.025 + 0.425 × r_a
        //   r_d = 0.025 + 1.7 × r_b
        //   r_e = 0.025 + 0.425 × r_d
        // Substituting yields r_a ≈ 0.24320 (exact: 0.11627 / 0.478).
        let g = build_graph(
            &["a", "b", "c", "d", "e", "f"],
            &[
                ("a", "b"), ("a", "c"),
                ("b", "d"), ("c", "d"),
                ("d", "e"), ("d", "f"),
                ("e", "a"), ("f", "a"),
            ],
        );
        let (r, _) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        let r_a_expected = 0.11627 / 0.478; // ≈ 0.24324
        assert!(
            (r[0] - r_a_expected).abs() < 1e-3,
            "r_a={}, expected≈{}",
            r[0],
            r_a_expected,
        );
        // Symmetry checks: r_b == r_c, r_e == r_f to high precision.
        assert!((r[1] - r[2]).abs() < 1e-7, "b/c symmetry: {} vs {}", r[1], r[2]);
        assert!((r[4] - r[5]).abs() < 1e-7, "e/f symmetry: {} vs {}", r[4], r[5]);
    }

    #[test]
    fn dangling_node_redistributes_mass() {
        // Node `sink` has no outgoing edges. Without the dangling
        // redistribution it would accumulate all mass and the others
        // would drift toward 0. With redistribution every node retains
        // positive rank.
        let g = build_graph(
            &["a", "b", "sink"],
            &[("a", "sink"), ("b", "sink")],
        );
        let (ranks, _) = page_rank(&g, 0.85, DEFAULT_TOL, MAX_ITER);
        for (i, r) in ranks.iter().enumerate() {
            assert!(*r > 1e-3, "node {i} rank shouldn't collapse, got {r}");
        }
        // Mass conservation still holds with dangling nodes.
        let sum: f64 = ranks.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "sum = {sum}");
    }
}
