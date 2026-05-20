import { useEffect, useMemo, useRef, useState } from "react";

import type { EventLogTreeNode } from "../lib/tauri";
import { matchFrameFilter, type FrameFilter } from "../lib/frame_filter";

/**
 * Snakeviz-style icicle chart. Top row is `<root>`; each function below
 * occupies the horizontal slice of its parent in proportion to its
 * inclusive duration. Click a node to "zoom" so it becomes the new root —
 * snakeviz semantics. The breadcrumb at the top steps back up the chain.
 *
 * Why hand-rolled SVG and not a chart library: every dep added to a
 * Tauri build hits both the JS bundle and the Rust webview surface. The
 * icicle layout is ~80 lines of math; a chart lib would be 200KB+ of
 * vendor JS for what's effectively a stacked-rectangle visitor.
 *
 * Search behavior — find-and-zoom (NOT find-and-dim):
 *
 *   When a non-empty `search` is provided, the chart automatically zooms
 *   to the matching node's subtree the same way clicking that node would.
 *   This is the snakeviz "click to drill in" interaction, fired by the
 *   page-level search input.
 *
 *   Why re-root instead of dimming:
 *     - The user types a name to *see that function's call path*, not
 *       to see a haystack with one needle highlighted.
 *     - Dimming wastes 95% of the screen on context the user already
 *       chose to filter out.
 *     - Click-to-zoom-deeper and breadcrumb-to-zoom-out keep working —
 *       search just primes the same zoomPath the UI already manages.
 *
 *   Multi-match rule: pick the match with the highest cumulative time.
 *   Deterministic, stable under live re-aggregation (the dominant
 *   occurrence stays dominant), and matches the snakeviz default of
 *   surfacing where the time actually went.
 *
 *   No-match: leave the zoom alone. The user sees the chart didn't
 *   change and knows their query missed.
 */
export interface IcicleChartProps {
  root: EventLogTreeNode;
  rowHeight?: number;
  height?: number;
  /** Optional search expression. When non-empty and matching, the chart
   *  auto-zooms to the best matching node's subtree. Empty search ≡ no
   *  auto-zoom; the user's manual zoom (or root) wins.
   *  The underlying predicate keeps its `FrameFilter` type name because
   *  it's a persisted AppConfig field and a stable internal contract;
   *  only the UX surface speaks of "search". */
  search?: FrameFilter;
  /** Called when the user clicks a node (e.g. to drill the table view). */
  onNodeClick?: (node: EventLogTreeNode, breadcrumb: string[]) => void;
}

interface Rect {
  node: EventLogTreeNode;
  x: number;
  y: number;
  width: number;
  depth: number;
  /** Breadcrumb path from the *current zoom root* to this node. */
  path: string[];
}

/** Width of the synthetic "user-space" the SVG viewBox spans. Rect
 *  x/width values from the layout pass live in [0, VIEW_WIDTH]. The
 *  scroll-zoom viewport (`view` state) crops a sub-window of this same
 *  space and the display pass scales each rect to fill it. */
const VIEW_WIDTH = 1000;

/** Floor on `view.width` — the smallest sub-window we'll render. Caps
 *  scroll-zoom-in at ~200× and stops the math from dividing by zero. */
const MIN_VIEW_WIDTH = 5;

/** Per-tick geometric zoom factor. 0.85 means each wheel notch shrinks
 *  the viewport to 85% (zoom in) or 1/0.85 ≈ 117% (zoom out). Geometric
 *  so the perceived "speed" of zooming feels consistent at any scale. */
const ZOOM_FACTOR = 0.85;

/** Viewport over the user-space layout. `width: VIEW_WIDTH` ≡ no zoom;
 *  smaller `width` zooms in. `x` is the left edge in user-space. */
interface Viewport {
  x: number;
  width: number;
}

const FULL_VIEW: Viewport = { x: 0, width: VIEW_WIDTH };

const COLORS = [
  "#ff6b3d",
  "#ff9558",
  "#ffb547",
  "#ffd073",
  "#ffe199",
  "#f6c87a",
  "#e8a44f",
  "#d77f3a",
];

function colorFor(name: string, depth: number): string {
  let h = depth * 17;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) | 0;
  return COLORS[Math.abs(h) % COLORS.length];
}

/** Locate the matching node with the highest cumulative time and return
 *  the path of node names from the (synthetic) root to it. Excludes the
 *  synthetic root itself — depth 0 is `<root>` and is never a real
 *  searchable frame.
 *
 *  Returns `null` if no node matches; the caller should leave the
 *  current zoom alone in that case.
 *
 *  Implementation note: we track best-path and best-cum as separate
 *  scalars (not as one object) because TypeScript's control-flow
 *  analysis can't see closure-side mutations on a `let object` and
 *  would narrow the result type away. Two scalars sidestep that. */
function findBestMatchPath(
  root: EventLogTreeNode,
  search: FrameFilter,
): string[] | null {
  let bestPath: string[] | null = null;
  let bestCum = -Infinity;
  const walk = (node: EventLogTreeNode, path: string[]) => {
    if (
      node.depth > 0 &&
      matchFrameFilter(search, { qualname: node.name, file: node.file }) &&
      node.value > bestCum
    ) {
      bestPath = path;
      bestCum = node.value;
    }
    for (const child of node.children) {
      walk(child, [...path, child.name]);
    }
  };
  walk(root, []);
  return bestPath;
}

function formatUs(us: number): string {
  if (us < 1) return "0 μs";
  if (us < 1_000) return `${us.toFixed(0)} μs`;
  if (us < 1_000_000) return `${(us / 1_000).toFixed(2)} ms`;
  return `${(us / 1_000_000).toFixed(3)} s`;
}

function clamp(value: number, lo: number, hi: number): number {
  if (value < lo) return lo;
  if (value > hi) return hi;
  return value;
}

export default function IcicleChart(props: IcicleChartProps): JSX.Element {
  const { root, rowHeight = 22, height = 360, search, onNodeClick } = props;
  const [zoomPath, setZoomPath] = useState<string[]>([]);
  const [hover, setHover] = useState<Rect | null>(null);
  // Independent of `zoomPath`: scroll-zoom viewport over the current
  // (click-zoomed) subtree. Click-to-drill re-roots the layout; scroll
  // crops a viewport into that layout. The two zooms compose naturally.
  const [view, setView] = useState<Viewport>(FULL_VIEW);
  const svgRef = useRef<SVGSVGElement | null>(null);

  // Search-driven auto-zoom. The page's search input parses to a stable
  // `FrameFilter` reference (memoized on the raw query string), so this
  // effect fires once per actual query change — not on every render.
  //
  // Empty search → reset to the full tree. Non-empty + hit → zoom to
  // the best match (highest cumulative time). Non-empty + miss → leave
  // the current zoom alone; an unchanged chart is the right "I didn't
  // find anything" signal.
  //
  // Crucially we do NOT depend on `root`. In live mode the Tauri
  // backend re-aggregates the event log once a second and ships a new
  // `report.tree` reference each tick; if `root` were a dep, every
  // tick would re-pick the match and snap the zoom back, clobbering
  // any manual breadcrumb navigation the user did in between. The
  // `zoomRoot` resolver below walks the path each render and falls back
  // to the deepest still-valid segment if the tree shape changes, so
  // we don't need to re-fire on tree updates.
  useEffect(() => {
    if (!search || search.empty) {
      setZoomPath([]);
      return;
    }
    const match = findBestMatchPath(root, search);
    if (match) setZoomPath(match);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  // Whenever the click-zoom (or search-zoom) re-roots the layout, the
  // scroll-zoom viewport becomes meaningless (it pointed at coordinates
  // in the old layout). Snap back to a full view of the new subtree.
  useEffect(() => {
    setView(FULL_VIEW);
  }, [zoomPath]);

  // Mouse-anchored scroll-wheel zoom. Implemented as a manual listener
  // because React's `onWheel` is passive — `preventDefault()` is a
  // no-op there, and without it the page would also scroll. We need
  // `{ passive: false }` to suppress the default.
  //
  // The math: keep the user-space cursor position invariant across the
  // zoom. After zooming, the cursor must still point at the same call
  // tree node — that's what makes "scroll to zoom in on this tiny
  // sliver" feel right. Pan with shift+wheel.
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    const onWheel = (e: WheelEvent) => {
      const rect = svg.getBoundingClientRect();
      if (rect.width <= 0) return;
      e.preventDefault();
      const cursorFrac = Math.min(
        1,
        Math.max(0, (e.clientX - rect.left) / rect.width),
      );

      // Shift+wheel pans horizontally. Pan amount is in user-space
      // units scaled by viewport width so 1 wheel notch moves a
      // consistent visual distance regardless of zoom level.
      if (e.shiftKey) {
        setView((v) => {
          const panUserSpace = (e.deltaY / rect.width) * v.width;
          const nextX = clamp(v.x + panUserSpace, 0, VIEW_WIDTH - v.width);
          return { ...v, x: nextX };
        });
        return;
      }

      setView((v) => {
        const userCursorX = v.x + cursorFrac * v.width;
        // deltaY < 0 = scroll up = zoom in (shrink width).
        const factor = e.deltaY < 0 ? ZOOM_FACTOR : 1 / ZOOM_FACTOR;
        const nextWidth = clamp(v.width * factor, MIN_VIEW_WIDTH, VIEW_WIDTH);
        // Already at the rail? Skip — no-op avoids a render churn.
        if (nextWidth === v.width) return v;
        const nextX = clamp(
          userCursorX - cursorFrac * nextWidth,
          0,
          VIEW_WIDTH - nextWidth,
        );
        return { x: nextX, width: nextWidth };
      });
    };
    svg.addEventListener("wheel", onWheel, { passive: false });
    return () => svg.removeEventListener("wheel", onWheel);
  }, []);

  const zoomRoot = useMemo(() => {
    let cur: EventLogTreeNode = root;
    for (const seg of zoomPath) {
      const next = cur.children.find((c) => c.name === seg);
      if (!next) break;
      cur = next;
    }
    return cur;
  }, [root, zoomPath]);

  // Layout pass — every node's natural x/width in user-space, before any
  // scroll-zoom is applied. Cached on the click-zoom subtree so the
  // layout doesn't recompute every wheel tick (only the display pass
  // below depends on `view`).
  const rects = useMemo<Rect[]>(() => {
    const out: Rect[] = [];
    const visit = (
      node: EventLogTreeNode,
      x: number,
      width: number,
      depth: number,
      path: string[],
    ) => {
      out.push({ node, x, y: depth * rowHeight, width, depth, path });
      const total = node.children.reduce((s, c) => s + c.value, 0);
      if (total <= 0 || width < 0.5) return;
      let cx = x;
      for (const child of node.children) {
        const cw = (child.value / total) * width;
        if (cw < 0.5) continue;
        visit(child, cx, cw, depth + 1, [...path, child.name]);
        cx += cw;
      }
    };
    visit(zoomRoot, 0, VIEW_WIDTH, 0, []);
    return out;
  }, [zoomRoot, rowHeight]);

  // Display pass — apply the scroll-zoom viewport. We translate by -x
  // and scale by VIEW_WIDTH/view.width so the cropped sub-window fills
  // the full SVG viewBox. We do this per-rect (rather than via a single
  // <g transform>) so SVG text inside each rect keeps its natural size
  // — wrapping the layout in `transform: scale(N, 1)` would stretch
  // glyphs horizontally and turn the labels into unreadable taffy.
  //
  // Off-screen rects are filtered out: when zoomed deep, the vast
  // majority of nodes sit outside the viewport. The filter keeps the
  // DOM proportional to what's visible, not what's in the layout.
  const displayRects = useMemo(() => {
    const scale = VIEW_WIDTH / view.width;
    const out: Rect[] = [];
    for (const r of rects) {
      const dx = (r.x - view.x) * scale;
      const dw = r.width * scale;
      if (dx + dw <= 0 || dx >= VIEW_WIDTH) continue;
      out.push({ ...r, x: dx, width: dw });
    }
    return out;
  }, [rects, view]);

  const maxDepth = useMemo(
    () => rects.reduce((m, r) => Math.max(m, r.depth), 0),
    [rects],
  );
  const svgHeight = Math.max(height, (maxDepth + 1) * rowHeight + 4);

  const handleClick = (r: Rect) => {
    setZoomPath((prev) => [...prev, ...r.path]);
    onNodeClick?.(r.node, [...zoomPath, ...r.path]);
  };

  // Best-practice details strip: always show metrics for the active
  // frame. Active = hovered frame > zoom root. This is the speedscope /
  // Chrome DevTools / IntelliJ pattern — at every zoom level the user
  // can read off "what am I looking at and how big is it" without
  // having to hover.
  //
  // `pctOfTotal` is normalized against the FULL original root, not the
  // zoom root, so a frame retains its objective weight in the trace as
  // you drill in (e.g. an 8.3%-of-trace subtree still reads 8.3% after
  // zooming, instead of being renormalized to 100%).
  const totalUs = root.value;
  const activeNode = hover?.node ?? zoomRoot;
  const pctOfTotal = totalUs > 0 ? (activeNode.value / totalUs) * 100 : 0;
  const isSyntheticRoot = activeNode === root && zoomPath.length === 0;

  return (
    <div className="icicle-wrap">
      <div className="icicle-breadcrumb">
        <button
          type="button"
          className="icicle-crumb"
          onClick={() => setZoomPath([])}
          disabled={zoomPath.length === 0}
        >
          {root.name}
        </button>
        {zoomPath.map((seg, i) => (
          <span key={`${i}-${seg}`}>
            <span className="icicle-crumb-sep">›</span>
            <button
              type="button"
              className="icicle-crumb"
              onClick={() => setZoomPath(zoomPath.slice(0, i + 1))}
            >
              {seg}
            </button>
          </span>
        ))}
        {(zoomPath.length > 0 || view.width < VIEW_WIDTH) && (
          <button
            type="button"
            className="icicle-reset"
            onClick={() => {
              setZoomPath([]);
              setView(FULL_VIEW);
            }}
            title="Reset both click-zoom (subtree) and scroll-zoom (viewport)"
          >
            reset zoom
          </button>
        )}
      </div>

      <div className="icicle-stats" aria-live="polite">
        <span className="icicle-stats-name" title={activeNode.name}>
          {isSyntheticRoot ? "Full trace" : activeNode.name}
        </span>
        <span className="icicle-stats-metric">
          <span className="icicle-stats-num">{formatUs(activeNode.value)}</span>
          <span className="icicle-stats-label">cum</span>
          {totalUs > 0 && (
            <span className="icicle-stats-pct">{pctOfTotal.toFixed(1)}%</span>
          )}
        </span>
        <span className="icicle-stats-metric">
          <span className="icicle-stats-num">{formatUs(activeNode.selfValue)}</span>
          <span className="icicle-stats-label">self</span>
        </span>
        <span className="icicle-stats-metric">
          <span className="icicle-stats-num">{activeNode.ncalls.toLocaleString()}</span>
          <span className="icicle-stats-label">
            call{activeNode.ncalls === 1 ? "" : "s"}
          </span>
        </span>
        {activeNode.file && (
          <span className="icicle-stats-loc" title={activeNode.file}>
            {activeNode.file}
            {activeNode.line ? `:${activeNode.line}` : ""}
          </span>
        )}
      </div>

      <div className="icicle-svg-wrap">
        <svg
          ref={svgRef}
          className="icicle-svg"
          viewBox={`0 0 ${VIEW_WIDTH} ${svgHeight}`}
          preserveAspectRatio="none"
          style={{
            height: svgHeight,
            cursor: view.width < VIEW_WIDTH ? "grab" : undefined,
          }}
        >
          {displayRects.map((r) => {
            const fill = r.depth === 0 ? "#555" : colorFor(r.node.name, r.depth);
            const textFill = r.depth === 0 ? "#fff" : "#1a1a1a";
            // Inline label: name plus inclusive time when the rect is
            // wide enough to fit both (>20% of the viewport, non-root).
            // Because `r.width` is in display space (post-scroll-zoom
            // scale), this means "show the time on any bar that's
            // visually big enough" — small frames that the user has
            // zoomed into get their inline metric back. Matches the
            // perfetto / speedscope pattern of "the bigger the bar, the
            // more you can read at a glance."
            const showInlineMetric = r.width > 200 && r.depth > 0;
            const timeSuffix = showInlineMetric ? ` · ${formatUs(r.node.value)}` : "";
            const nameBudget = Math.floor(
              (r.width - 12 - timeSuffix.length * 7) / 7,
            );
            const label = clip(r.node.name, nameBudget) + timeSuffix;
            return (
              <g
                key={`${r.depth}-${r.path.join("/")}`}
                transform={`translate(${r.x},${r.y})`}
                onClick={() => handleClick(r)}
                onMouseEnter={() => setHover(r)}
                onMouseLeave={() => setHover((h) => (h === r ? null : h))}
                style={{ cursor: r.node.children.length > 0 ? "pointer" : "default" }}
              >
                <rect
                  width={Math.max(0, r.width - 0.5)}
                  height={Math.max(0, rowHeight - 1)}
                  fill={fill}
                  opacity={hover && hover !== r ? 0.65 : 0.95}
                  rx={1.5}
                />
                {r.width > 60 && (
                  <text
                    x={6}
                    y={rowHeight / 2 + 4}
                    fontSize={11}
                    fill={textFill}
                    style={{ pointerEvents: "none", userSelect: "none" }}
                  >
                    {label}
                  </text>
                )}
              </g>
            );
          })}
        </svg>
      </div>

    </div>
  );
}

function clip(s: string, maxChars: number): string {
  if (maxChars <= 1) return "";
  if (s.length <= maxChars) return s;
  return s.slice(0, Math.max(1, maxChars - 1)) + "…";
}
