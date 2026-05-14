import type { ScanProgress } from "../../lib/tauri";

/**
 * Render a live timeline of the static-scan phases.
 *
 * The backend emits a heterogeneous stream of `ScanProgress` events. We
 * reduce them into a flat phase list — one row per phase that ever entered
 * — and render each with an icon, a counted bar (when known), and the
 * "current item" trailing slot.
 *
 * Reduction rules:
 *   - `walk_start` / `parse_start` / `step_start` / `phase` → push a new
 *     phase row (or activate the existing one if the same label re-enters).
 *   - `walk_progress` / `parse_progress` / `step_progress` → update the
 *     active row's counters and current item.
 *   - `walk_end` → finalize walk with the total files + bytes shown in the
 *     trailing detail string.
 *
 * The previous active row is marked `done` when the next start arrives.
 */

export interface PhaseRow {
  key: string;
  icon: string;
  label: string;
  detail: string | null;
  status: "active" | "done";
  done: number | null;
  total: number | null;
}

const PHASE_ICONS: Record<string, string> = {
  walking_filesystem: "🗂️",
  walk: "🗂️",
  parse: "📑",
  parsing_source: "📑",
  graph: "🕸️",
  pagerank: "📈",
  scanning_roots: "🌱",
  "scanning roots": "🌱",
  "building call graph": "🕸️",
  "building call trees": "🌲",
  "collecting entry declarations": "📦",
  "computing pagerank percentile": "📈",
};

function iconFor(label: string): string {
  const lc = label.toLowerCase();
  for (const k of Object.keys(PHASE_ICONS)) {
    if (lc.includes(k)) return PHASE_ICONS[k];
  }
  if (lc.includes("attach")) return "🏷️";
  if (lc.includes("entry")) return "📦";
  if (lc.includes("root")) return "🌱";
  if (lc.includes("tree")) return "🌲";
  if (lc.includes("graph")) return "🕸️";
  if (lc.includes("language")) return "🔤";
  if (lc.includes("hot")) return "🔥";
  if (lc.includes("write") || lc.includes("save")) return "💾";
  return "⚙️";
}

/**
 * Pure reducer — fold one event into the running phase list. Exposed
 * directly so the consumer (Scan page) can hold the array in its own state
 * and use this in a `useReducer` or a plain `useState` `.reduce`.
 */
export function reduceProgress(rows: PhaseRow[], ev: ScanProgress): PhaseRow[] {
  switch (ev.kind) {
    case "walk_start":
      return finalizePrevious(rows, {
        key: "walk",
        icon: iconFor("walking filesystem"),
        label: "walking filesystem",
        detail: "discovering files…",
        status: "active",
        done: null,
        total: null,
      });
    case "walk_progress":
      return upsertActive(rows, "walk", (r) => ({
        ...r,
        detail: `${ev.filesSeen} files seen`,
      }));
    case "walk_end":
      return upsertActive(rows, "walk", (r) => ({
        ...r,
        detail: `${ev.totalFiles} files · ${humanBytes(ev.bytes)}`,
      }));
    case "parse_start":
      return finalizePrevious(rows, {
        key: "parse",
        icon: iconFor("parsing source"),
        label: "parsing source",
        detail: null,
        status: "active",
        done: 0,
        total: ev.totalSourceFiles,
      });
    case "parse_progress":
      return upsertActive(rows, "parse", (r) => ({
        ...r,
        done: ev.done,
        total: ev.total,
        detail: ev.current ?? r.detail,
      }));
    case "phase": {
      const key = `phase:${ev.name}`;
      return finalizePrevious(rows, {
        key,
        icon: iconFor(ev.name),
        label: ev.name,
        detail: null,
        status: "active",
        done: null,
        total: null,
      });
    }
    case "step_start": {
      const key = `step:${ev.label}`;
      return finalizePrevious(rows, {
        key,
        icon: iconFor(ev.label),
        label: ev.label,
        detail: null,
        status: "active",
        done: 0,
        total: ev.total,
      });
    }
    case "step_progress": {
      const key = `step:${ev.label}`;
      return upsertActive(rows, key, (r) => ({
        ...r,
        done: ev.done,
        total: ev.total,
        detail: ev.current ?? r.detail,
      }));
    }
  }
}

function finalizePrevious(rows: PhaseRow[], next: PhaseRow): PhaseRow[] {
  const flipped: PhaseRow[] = rows.map((r) =>
    r.status === "active" ? { ...r, status: "done" as const } : r,
  );
  // Re-entering the same phase key (e.g. parse loop after compaction): bring
  // it back to active rather than appending a duplicate.
  const existingIdx = flipped.findIndex((r) => r.key === next.key);
  if (existingIdx >= 0) {
    flipped[existingIdx] = { ...next };
    return flipped;
  }
  flipped.push(next);
  return flipped;
}

function upsertActive(
  rows: PhaseRow[],
  key: string,
  fn: (r: PhaseRow) => PhaseRow,
): PhaseRow[] {
  return rows.map((r) => (r.key === key && r.status === "active" ? fn(r) : r));
}

function humanBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(0)} kB`;
  return `${bytes} B`;
}

/**
 * Render the reduced phase list. Stays small so the parent can compose it
 * inside whatever surface (Scan page today; could be a side panel tomorrow).
 */
export default function ScanProgressList({ rows }: { rows: PhaseRow[] }) {
  if (rows.length === 0) {
    return null;
  }
  return (
    <div className="scan-progress" role="status" aria-live="polite">
      {rows.map((r) => (
        <PhaseRowView key={r.key} row={r} />
      ))}
    </div>
  );
}

function PhaseRowView({ row }: { row: PhaseRow }) {
  const pct =
    row.total != null && row.total > 0 && row.done != null
      ? (row.done / row.total) * 100
      : null;
  const showSpinner = row.status === "active" && pct == null;
  const iconText = row.status === "done" ? "✓" : row.icon;

  return (
    <div className={`scan-progress-row is-${row.status}`}>
      <span className="scan-progress-icon" aria-hidden>
        {showSpinner ? <span className="scan-progress-spinner" /> : iconText}
      </span>
      <span className="scan-progress-label">
        <span className="scan-progress-label-line">{row.label}</span>
        {row.detail && (
          <span className="scan-progress-label-detail">{row.detail}</span>
        )}
      </span>
      {pct != null ? (
        <>
          <span className="scan-progress-bar">
            <span
              className="scan-progress-bar-fill"
              style={{ width: `${Math.min(100, pct)}%` }}
            />
          </span>
          <span className="scan-progress-count">
            {row.done}/{row.total}
          </span>
        </>
      ) : (
        <span className="scan-progress-count">{row.status === "done" ? "done" : ""}</span>
      )}
    </div>
  );
}
