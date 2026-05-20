// Call Tree tab — hierarchical view over `report.tree`.
//
// Shows the same aggregated call tree the flame graph draws, but as an
// expandable indent list with per-row metrics. The use case is the
// inverse of the flame graph: when the user wants to read the call path
// for a specific function ("who calls `create_order`?"), a tree row is
// faster than tracing rectangles vertically.
//
// Search behavior — three rules that work together:
//
//   1. Any path that contains a match is force-expanded so the match is
//      reachable without clicking. We pre-compute `pathsWithMatch` in
//      one walk so the render pass is O(n).
//   2. Non-matching rows are dimmed (foreground only — we keep them
//      visible because they're the structural context of the match).
//   3. Empty search → only the first two levels are expanded by default;
//      everything deeper stays collapsed until the user clicks. This
//      keeps the initial render light on big traces.

import { useMemo, useState } from "react";

import { matchFrameFilter, type FrameFilter } from "../../lib/frame_filter";
import type { EventLogTreeNode } from "../../lib/tauri";

interface Props {
  root: EventLogTreeNode;
  search: FrameFilter;
  /** Currently-selected node (e.g. clicked in the flame graph). Drives
   *  row highlighting; null means "no selection." */
  selected: EventLogTreeNode | null;
  onSelect: (node: EventLogTreeNode) => void;
}

/** Depth (counted from the synthetic root) up to which we render
 *  immediately when no search is active. 2 ≈ "root + first call layer." */
const DEFAULT_OPEN_DEPTH = 2;

export default function CallTreePanel({ root, search, selected, onSelect }: Props): JSX.Element {
  // The set of node paths the user has *manually* toggled. Combined
  // with `DEFAULT_OPEN_DEPTH` and the search-driven force-open below to
  // produce the actual "is this row expanded?" decision.
  const [userToggled, setUserToggled] = useState<Map<string, boolean>>(() => new Map());

  const searchActive = !search.empty;

  // Pre-walk: which paths contain a matching descendant? We use this
  // both to force-open paths and to fade nodes that aren't on a match's
  // lineage so the match itself is visually emphasized.
  const pathsWithMatch = useMemo(() => {
    if (!searchActive) return null;
    const hit = new Set<string>();
    visit(root, [], (node, path) => {
      const isMatch = matchFrameFilter(search, { qualname: node.name, file: node.file });
      if (isMatch) {
        // Mark every ancestor (including self) as "on a matching path."
        for (let i = 0; i <= path.length; i++) {
          hit.add(path.slice(0, i).join("\0"));
        }
      }
    });
    return hit;
  }, [root, search, searchActive]);

  const isOpen = (pathKey: string, depth: number): boolean => {
    const manual = userToggled.get(pathKey);
    if (manual != null) return manual;
    if (searchActive && pathsWithMatch?.has(pathKey)) return true;
    return depth < DEFAULT_OPEN_DEPTH;
  };

  const toggle = (pathKey: string, depth: number) => {
    setUserToggled((prev) => {
      const next = new Map(prev);
      next.set(pathKey, !isOpen(pathKey, depth));
      return next;
    });
  };

  const rows = useMemo(() => {
    const out: Row[] = [];
    walkVisible(root, [], 0, out, (pathKey, depth) => isOpen(pathKey, depth));
    return out;
    // `isOpen` closes over state — list it explicitly so React reruns
    // when toggles happen. Pre-walk inputs (root, search) are already in
    // its closure via `pathsWithMatch`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, userToggled, pathsWithMatch]);

  return (
    <div className="call-tree">
      <table className="call-tree-table">
        <thead>
          <tr>
            <th className="call-tree-th call-tree-th-name">qualname</th>
            <th className="call-tree-th call-tree-th-num">ncalls</th>
            <th className="call-tree-th call-tree-th-num">tottime</th>
            <th className="call-tree-th call-tree-th-num">cumtime</th>
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td colSpan={4} className="call-tree-empty muted">
                No call data yet.
              </td>
            </tr>
          )}
          {rows.map((r) => {
            const pathKey = r.pathKey;
            const open = isOpen(pathKey, r.depth);
            const matches = searchActive
              ? matchFrameFilter(search, { qualname: r.node.name, file: r.node.file })
              : true;
            // Dim a row when search is active AND the row itself doesn't
            // match. We still show ancestors of matches because their
            // children are visible — fading the ancestors would leave
            // visible descendants floating without context.
            const dimmed = searchActive && !matches;
            const highlighted = selected?.name === r.node.name && selected.depth === r.node.depth;
            return (
              <tr
                key={pathKey}
                className={[
                  "call-tree-row",
                  dimmed && "call-tree-row--dim",
                  highlighted && "call-tree-row--hi",
                ]
                  .filter(Boolean)
                  .join(" ")}
                onClick={() => onSelect(r.node)}
                title={r.node.file ? `${r.node.file}${r.node.line ? `:${r.node.line}` : ""}` : ""}
              >
                <td className="call-tree-name">
                  <span
                    className="call-tree-indent"
                    style={{ paddingLeft: r.depth * 16 }}
                  />
                  {r.node.children.length > 0 ? (
                    <button
                      type="button"
                      className="call-tree-chevron"
                      onClick={(e) => {
                        // Don't bubble — row click selects, chevron toggles.
                        e.stopPropagation();
                        toggle(pathKey, r.depth);
                      }}
                      aria-label={open ? "Collapse" : "Expand"}
                    >
                      {open ? "▾" : "▸"}
                    </button>
                  ) : (
                    <span className="call-tree-chevron call-tree-chevron--leaf">·</span>
                  )}
                  <span className="call-tree-symbol">{r.node.name}</span>
                </td>
                <td className="call-tree-num">{r.node.ncalls.toLocaleString()}</td>
                <td className="call-tree-num">{formatUs(r.node.selfValue)}</td>
                <td className="call-tree-num">{formatUs(r.node.value)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

interface Row {
  node: EventLogTreeNode;
  depth: number;
  pathKey: string;
}

/** DFS visit that emits ONLY currently-visible rows. A child is visited
 *  iff its parent is open. The pathKey is `\0`-joined (NUL never appears
 *  in a qualname / file path so it makes a safe separator). */
function walkVisible(
  node: EventLogTreeNode,
  path: string[],
  depth: number,
  out: Row[],
  isOpen: (pathKey: string, depth: number) => boolean,
): void {
  const pathKey = path.join("\0");
  out.push({ node, depth, pathKey });
  if (!isOpen(pathKey, depth)) return;
  for (const child of node.children) {
    walkVisible(child, [...path, child.name], depth + 1, out, isOpen);
  }
}

/** Cheaper DFS used by the pre-walk for `pathsWithMatch` — it always
 *  recurses regardless of open state. */
function visit(
  node: EventLogTreeNode,
  path: string[],
  fn: (node: EventLogTreeNode, path: string[]) => void,
): void {
  fn(node, path);
  for (const child of node.children) {
    visit(child, [...path, child.name], fn);
  }
}

function formatUs(us: number): string {
  if (us < 1) return "0 μs";
  if (us < 1_000) return `${us.toFixed(0)} μs`;
  if (us < 1_000_000) return `${(us / 1_000).toFixed(2)} ms`;
  return `${(us / 1_000_000).toFixed(3)} s`;
}
