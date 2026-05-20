// Call Graph tab — neighborhood view for one focus symbol.
//
// Picks a single "focus" function and lists its callers (functions that
// invoke it) and its callees (functions it invokes). Each side is
// aggregated across every occurrence of the focus in the call tree, so
// even when the same function is called from three places, we collapse
// the three caller rows into one with `×3` and a summed edge weight.
//
// Why a neighborhood and not a full force-directed graph: a Sugiyama-
// style layout is great for static analysis (the viewer ships one), but
// the runtime profile is interactive — the user is drilling, not
// surveying. Showing one symbol's immediate neighborhood matches the
// JetBrains "Find Usages" / VS Code "Call Hierarchy" UX, which is what
// the user actually needs when they ask "who calls create_order?"
//
// Focus-pick precedence (first that resolves wins):
//
//   1. The currently-selected node (set by clicking in Flame / Tree).
//   2. The single search match, if the search resolves to exactly one
//      function name (the common case for `name:create_order`).
//   3. The top-cumulative-time function. Always something to show.

import { useMemo } from "react";

import { matchFrameFilter, type FrameFilter } from "../../lib/frame_filter";
import type { EventLogTreeNode } from "../../lib/tauri";

interface Props {
  root: EventLogTreeNode;
  /** Per-qualname rollup — only used to fall back to a top-time symbol
   *  when neither selection nor search resolves a focus. */
  functions: readonly { qualname: string; cumulativeUs: number }[];
  search: FrameFilter;
  selected: EventLogTreeNode | null;
  /** Forward selection to the parent so clicking a caller or callee
   *  jumps the rest of the page (table highlight, future flame zoom). */
  onSelect: (node: EventLogTreeNode) => void;
}

export default function CallGraphPanel({
  root,
  functions,
  search,
  selected,
  onSelect,
}: Props): JSX.Element {
  const focusName = useMemo(
    () => pickFocusName(selected, search, root, functions),
    [selected, search, root, functions],
  );

  // Collect every occurrence of the focus in the tree. A function can
  // appear in many places (different call paths), and each contributes
  // its own callers + callees to the aggregate.
  const occurrences = useMemo(
    () => (focusName ? collectOccurrences(root, focusName) : []),
    [root, focusName],
  );

  const callers = useMemo(() => aggregateCallers(occurrences), [occurrences]);
  const callees = useMemo(() => aggregateCallees(occurrences), [occurrences]);

  // Sum metrics for the focus itself across all occurrences. Showing
  // the totals at the focus card makes the caller/callee weights
  // comparable to a known anchor.
  const focusTotals = useMemo(() => sumTotals(occurrences), [occurrences]);

  if (!focusName) {
    return (
      <div className="call-graph-empty muted">
        No focus symbol. Click a function in the flame graph, tree, or
        Functions table to inspect its callers and callees.
      </div>
    );
  }

  return (
    <div className="call-graph">
      <Column
        title="Callers"
        subtitle="Who invokes this function"
        rows={callers}
        onPick={(name) => jumpTo(name, root, onSelect)}
        emptyMsg={
          occurrences.some((o) => o.parent === null)
            ? "This is a top-level entry — no callers in the trace."
            : "No callers recorded."
        }
      />
      <FocusCard
        name={focusName}
        ncalls={focusTotals.ncalls}
        cumulativeUs={focusTotals.cumulativeUs}
        selfUs={focusTotals.selfUs}
        occurrences={occurrences.length}
      />
      <Column
        title="Callees"
        subtitle="What this function invokes"
        rows={callees}
        onPick={(name) => jumpTo(name, root, onSelect)}
        emptyMsg="No callees — this is a leaf in the call tree."
      />
    </div>
  );
}

function pickFocusName(
  selected: EventLogTreeNode | null,
  search: FrameFilter,
  root: EventLogTreeNode,
  functions: readonly { qualname: string; cumulativeUs: number }[],
): string | null {
  if (selected && selected.depth > 0) return selected.name;

  if (!search.empty) {
    // If the search resolves to exactly one matching qualname in the
    // tree, pick that. More than one → ambiguous; punt to the fallback
    // so we don't silently pick the wrong symbol.
    const matches = new Set<string>();
    visit(root, (n) => {
      if (n.depth === 0) return;
      if (matchFrameFilter(search, { qualname: n.name, file: n.file })) {
        matches.add(n.name);
      }
    });
    if (matches.size === 1) return [...matches][0];
  }

  // Fallback: heaviest cumulative-time symbol in the report. `functions`
  // is already sorted desc by cumulative by the aggregator, but we
  // recompute the max in case the parent's sort key differs.
  let best: { name: string; cum: number } | null = null;
  for (const f of functions) {
    if (!best || f.cumulativeUs > best.cum) {
      best = { name: f.qualname, cum: f.cumulativeUs };
    }
  }
  return best?.name ?? null;
}

interface Occurrence {
  node: EventLogTreeNode;
  parent: EventLogTreeNode | null;
}

function collectOccurrences(root: EventLogTreeNode, name: string): Occurrence[] {
  const out: Occurrence[] = [];
  const walk = (node: EventLogTreeNode, parent: EventLogTreeNode | null) => {
    if (node.name === name && node.depth > 0) out.push({ node, parent });
    for (const c of node.children) walk(c, node);
  };
  walk(root, null);
  return out;
}

interface EdgeRow {
  name: string;
  edges: number;
  totalUs: number;
}

function aggregateCallers(occ: readonly Occurrence[]): EdgeRow[] {
  const byCaller = new Map<string, EdgeRow>();
  for (const o of occ) {
    if (!o.parent || o.parent.depth === 0) continue;
    upsertEdge(byCaller, o.parent.name, o.node.value);
  }
  return [...byCaller.values()].sort((a, b) => b.totalUs - a.totalUs);
}

function aggregateCallees(occ: readonly Occurrence[]): EdgeRow[] {
  const byCallee = new Map<string, EdgeRow>();
  for (const o of occ) {
    for (const child of o.node.children) {
      upsertEdge(byCallee, child.name, child.value);
    }
  }
  return [...byCallee.values()].sort((a, b) => b.totalUs - a.totalUs);
}

function upsertEdge(map: Map<string, EdgeRow>, name: string, weightUs: number): void {
  const existing = map.get(name);
  if (existing) {
    existing.edges += 1;
    existing.totalUs += weightUs;
  } else {
    map.set(name, { name, edges: 1, totalUs: weightUs });
  }
}

interface Totals {
  ncalls: number;
  cumulativeUs: number;
  selfUs: number;
}

function sumTotals(occ: readonly Occurrence[]): Totals {
  return occ.reduce<Totals>(
    (acc, o) => ({
      ncalls: acc.ncalls + o.node.ncalls,
      cumulativeUs: acc.cumulativeUs + o.node.value,
      selfUs: acc.selfUs + o.node.selfValue,
    }),
    { ncalls: 0, cumulativeUs: 0, selfUs: 0 },
  );
}

/** Find the first tree node with this qualname and forward it to the
 *  parent's selection handler. "First occurrence" is good enough for a
 *  navigation hop; the panel will re-derive callers/callees from there. */
function jumpTo(
  name: string,
  root: EventLogTreeNode,
  onSelect: (node: EventLogTreeNode) => void,
): void {
  let hit: EventLogTreeNode | null = null;
  const walk = (n: EventLogTreeNode) => {
    if (hit) return;
    if (n.name === name && n.depth > 0) {
      hit = n;
      return;
    }
    for (const c of n.children) walk(c);
  };
  walk(root);
  if (hit) onSelect(hit);
}

function visit(node: EventLogTreeNode, fn: (n: EventLogTreeNode) => void): void {
  fn(node);
  for (const c of node.children) visit(c, fn);
}

interface ColumnProps {
  title: string;
  subtitle: string;
  rows: readonly EdgeRow[];
  emptyMsg: string;
  onPick: (name: string) => void;
}

function Column({ title, subtitle, rows, emptyMsg, onPick }: ColumnProps) {
  return (
    <section className="call-graph-col" aria-label={title}>
      <header className="call-graph-col-head">
        <h3 className="call-graph-col-title">{title}</h3>
        <p className="call-graph-col-sub">{subtitle}</p>
      </header>
      {rows.length === 0 ? (
        <div className="call-graph-col-empty muted">{emptyMsg}</div>
      ) : (
        <ul className="call-graph-col-list">
          {rows.map((r) => (
            <li key={r.name}>
              <button
                type="button"
                className="call-graph-row"
                onClick={() => onPick(r.name)}
                title={`Jump to ${r.name}`}
              >
                <span className="call-graph-row-name">{r.name}</span>
                <span className="call-graph-row-meta">
                  <span className="call-graph-row-weight">{formatUs(r.totalUs)}</span>
                  {r.edges > 1 && (
                    <span className="call-graph-row-edges">×{r.edges}</span>
                  )}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

interface FocusCardProps {
  name: string;
  ncalls: number;
  cumulativeUs: number;
  selfUs: number;
  occurrences: number;
}

function FocusCard({ name, ncalls, cumulativeUs, selfUs, occurrences }: FocusCardProps) {
  return (
    <section className="call-graph-focus" aria-label={`Focus: ${name}`}>
      <div className="call-graph-focus-label">focus</div>
      <div className="call-graph-focus-name">{name}</div>
      <dl className="call-graph-focus-stats">
        <div>
          <dt>ncalls</dt>
          <dd>{ncalls.toLocaleString()}</dd>
        </div>
        <div>
          <dt>cumtime</dt>
          <dd>{formatUs(cumulativeUs)}</dd>
        </div>
        <div>
          <dt>tottime</dt>
          <dd>{formatUs(selfUs)}</dd>
        </div>
        <div>
          <dt>paths</dt>
          <dd>{occurrences}</dd>
        </div>
      </dl>
    </section>
  );
}

function formatUs(us: number): string {
  if (us < 1) return "0 μs";
  if (us < 1_000) return `${us.toFixed(0)} μs`;
  if (us < 1_000_000) return `${(us / 1_000).toFixed(2)} ms`;
  return `${(us / 1_000_000).toFixed(3)} s`;
}
