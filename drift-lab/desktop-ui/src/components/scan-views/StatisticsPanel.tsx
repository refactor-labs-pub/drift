// Statistics tab for the active scan / loaded report.
//
// "Where did the time go?" — answered four ways at a glance, all from
// the already-aggregated per-function rollups in `report.functions`:
//
//   1. Top by cumulative time   → wide-context hotspots (inclusive ms).
//   2. Top by self time         → the actual hot leaves (exclusive ms).
//   3. Most-called functions    → micro-optimization candidates.
//   4. Errored functions        → failure hotspots; hidden when zero.
//
// Search is applied UPSTREAM by `ReportView` (the parent passes the
// already-filtered `functions` array). That keeps the panel a pure
// presentation component — same contract as the rest of the scan-views/
// directory. The panel does NOT re-parse the search query.
//
// What this panel deliberately doesn't show:
//   - Per-percentile latency (p50/p99). We don't have per-call samples
//     here; the aggregator collapsed them into mean/total at ingest.
//   - Call-path attribution. That's the Call Tree / Call Graph tabs.
//   - Source jumps. The desktop UI doesn't host a code viewer yet; when
//     it does, the `(file, line)` columns become click targets.

import type { EventLogFunctionStat, EventLogReport } from "../../lib/tauri";

/** Cap on rows per panel. Five is the snakeviz default and matches the
 *  density of a single-screen view; users who want more should jump
 *  into the Functions tab and sort there. */
const TOP_N = 5;

interface Props {
  /** Search-filtered function stats from the parent. */
  functions: readonly EventLogFunctionStat[];
  /** The full report — used for global counters that should not shrink
   *  with the search (e.g. total calls in the run, run duration). */
  report: EventLogReport;
}

export default function StatisticsPanel({ functions, report }: Props): JSX.Element {
  // Each top-N is a cheap sort + slice — no memoization needed unless
  // `functions.length` blows up past low-thousands. The aggregator
  // ships at most a few hundred symbols per scan today.
  const byCumulative = topN(functions, (f) => f.cumulativeUs);
  const bySelf = topN(functions, (f) => f.totalUs);
  const byCalls = topN(functions, (f) => f.ncalls);
  const byErrors = topN(functions, (f) => f.errors).filter((f) => f.errors > 0);

  return (
    <div className="stats-panel">
      <header className="stats-panel-headline">
        <Metric label="run duration" value={formatUs(report.durationUs)} />
        <Metric label="calls" value={report.totalCalls.toLocaleString()} />
        <Metric label="events" value={report.totalEvents.toLocaleString()} />
        <Metric label="symbols" value={functions.length.toLocaleString()} />
      </header>

      <div className="stats-panel-grid">
        <Card
          title="Top by cumulative time"
          hint="Inclusive time on the call path — wide-context hotspots."
          rows={byCumulative}
          metricLabel="cumtime"
          metric={(f) => formatUs(f.cumulativeUs)}
          emptyMsg="No matching symbols."
        />
        <Card
          title="Top by self time"
          hint="Exclusive time inside the function — the actual hot loops."
          rows={bySelf}
          metricLabel="tottime"
          metric={(f) => formatUs(f.totalUs)}
          emptyMsg="No matching symbols."
        />
        <Card
          title="Most-called functions"
          hint="High call-count entries are micro-optimization candidates."
          rows={byCalls}
          metricLabel="ncalls"
          metric={(f) => f.ncalls.toLocaleString()}
          emptyMsg="No matching symbols."
        />
        <Card
          title="Errors"
          hint="Functions that raised at least once during this run."
          rows={byErrors}
          metricLabel="errors"
          metric={(f) => f.errors.toLocaleString()}
          emptyMsg="No errors recorded."
        />
      </div>
    </div>
  );
}

function topN(
  rows: readonly EventLogFunctionStat[],
  key: (r: EventLogFunctionStat) => number,
): EventLogFunctionStat[] {
  // `slice()` so we don't mutate the parent's array — important because
  // the same array is the basis for other panels' sorts.
  return rows.slice().sort((a, b) => key(b) - key(a)).slice(0, TOP_N);
}

interface CardProps {
  title: string;
  hint: string;
  rows: readonly EventLogFunctionStat[];
  metricLabel: string;
  metric: (r: EventLogFunctionStat) => string;
  emptyMsg: string;
}

function Card({ title, hint, rows, metricLabel, metric, emptyMsg }: CardProps) {
  return (
    <section className="stats-card" aria-label={title}>
      <h3 className="stats-card-title">{title}</h3>
      <p className="stats-card-hint">{hint}</p>
      {rows.length === 0 ? (
        <div className="stats-card-empty muted">{emptyMsg}</div>
      ) : (
        <ol className="stats-card-list">
          {rows.map((f) => (
            <li key={f.qualname} className="stats-card-row">
              <span className="stats-card-name" title={f.qualname}>
                {f.qualname}
              </span>
              <span
                className="stats-card-metric"
                title={metricLabel}
                aria-label={metricLabel}
              >
                {metric(f)}
              </span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="stats-metric">
      <span className="stats-metric-label">{label}</span>
      <span className="stats-metric-value">{value}</span>
    </div>
  );
}

// Local copy of the live-scan formatter rather than importing from the
// page. The panel is a leaf component and shouldn't reach back into
// LiveScan.tsx; if/when this formatter grows a third caller it should
// move into `lib/format.ts`.
function formatUs(us: number): string {
  if (us < 1) return "0 μs";
  if (us < 1_000) return `${us.toFixed(0)} μs`;
  if (us < 1_000_000) return `${(us / 1_000).toFixed(2)} ms`;
  return `${(us / 1_000_000).toFixed(3)} s`;
}
