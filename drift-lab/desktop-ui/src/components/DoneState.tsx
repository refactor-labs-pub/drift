import type { VisibilityMap } from "../lib/tauri";

import { ArrowRightIcon, CheckIcon } from "./icons";

interface Props {
  issuesFound: number;
  criticalCount: number;
  /** End-of-run visibility map. Null until/unless the backend emitted
   *  `run://report` — when present, we lead with the CPU-reduction headline
   *  instead of the raw issue count. */
  visibilityMap: VisibilityMap | null;
  onView: () => void;
  onRerun: () => void;
  onReset: () => void;
}

export default function DoneState({
  issuesFound,
  criticalCount,
  visibilityMap,
  onView,
  onRerun,
  onReset,
}: Props) {
  const headline = visibilityMap
    ? `Up to ~${Math.max(visibilityMap.estimatedCpuReductionPct, 1).toFixed(0)}% CPU reduction available`
    : "Found it ✨";
  const sub = visibilityMap
    ? `${visibilityMap.critical.length} critical · ${visibilityMap.warnings.length} warnings`
    : `${issuesFound} performance ${issuesFound === 1 ? "issue" : "issues"} detected · ${criticalCount} critical`;

  return (
    <div className="done-state">
      <div className="done-icon">
        <CheckIcon />
      </div>
      <div>
        <div className="done-title">{headline}</div>
        <div className="done-sub">{sub}</div>
      </div>
      <div className="done-actions">
        <button type="button" className="view-btn" onClick={onView}>
          View report
          <ArrowRightIcon />
        </button>
        <button type="button" className="ghost-btn" onClick={onRerun}>
          ↻ Rerun
        </button>
        <button type="button" className="ghost-btn" onClick={onReset}>
          Run another
        </button>
      </div>
    </div>
  );
}
