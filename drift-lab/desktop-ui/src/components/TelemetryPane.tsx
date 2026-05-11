/**
 * Live container telemetry — paired with `ReasoningLog` in the split-view
 * running screen so the user sees what the agent is *thinking* on the left
 * and what the target container is actually *doing* on the right.
 *
 * Samples come from the backend's docker-stats poller (see
 * `src-tauri/src/telemetry.rs`). Net/block IO are cumulative counters; we
 * derive bytes/sec by diffing successive samples in the same series.
 *
 * The sparklines are hand-drawn SVG. No chart library — three reasons:
 *   1. We only need four tiny stat-line panels, not configurable axes.
 *   2. Bundling a chart lib would dwarf the rest of the UI by size.
 *   3. The dataset is bounded (<= 60 points) and the redraw cost is trivial.
 */

import { useMemo } from "react";

import type { TelemetrySample } from "../lib/tauri";
import { useRunStore } from "../store/runStore";

/** Number of samples to plot. 60 @ 500ms = 30 seconds of history. */
const WINDOW = 60;

export default function TelemetryPane() {
  const samples = useRunStore((s) => s.telemetrySamples);

  const window = useMemo(() => samples.slice(-WINDOW), [samples]);
  const latest = window[window.length - 1];

  if (!latest) {
    return (
      <div className="telemetry-pane">
        <div className="telemetry-header">
          <span className="telemetry-title">Container telemetry</span>
          <span className="telemetry-count">waiting…</span>
        </div>
        <div className="telemetry-empty">
          Waiting for the agent to attach to a container…
        </div>
      </div>
    );
  }

  // Derive rates from successive cumulative counters.
  const rates = computeRates(window);
  const cpuCritical = latest.cpuPct > 80;

  return (
    <div className="telemetry-pane">
      <div className="telemetry-header">
        <span className="telemetry-title">Container telemetry</span>
        <span className="telemetry-count">
          <code>{latest.containerId.slice(0, 12)}</code>
        </span>
      </div>

      <div className="telemetry-stats">
        <Stat
          label="CPU"
          value={`${latest.cpuPct.toFixed(1)}%`}
          critical={cpuCritical}
          series={window.map((s) => s.cpuPct)}
          maxHint={100}
        />
        <Stat
          label="Memory"
          value={`${latest.memMb.toFixed(0)} MB`}
          sub={`${latest.memPct.toFixed(1)}%`}
          series={window.map((s) => s.memMb)}
        />
        <Stat
          label="Net I/O"
          value={formatRate(rates.netTotal)}
          sub={`↓ ${formatRate(rates.netRx)} · ↑ ${formatRate(rates.netTx)}`}
          series={rates.netSeries}
        />
        <Stat
          label="Disk I/O"
          value={formatRate(rates.blockTotal)}
          sub={`R ${formatRate(rates.blockR)} · W ${formatRate(rates.blockW)}`}
          series={rates.blockSeries}
        />
      </div>
    </div>
  );
}

interface StatProps {
  label: string;
  value: string;
  sub?: string;
  critical?: boolean;
  series: number[];
  maxHint?: number;
}

function Stat({ label, value, sub, critical, series, maxHint }: StatProps) {
  return (
    <div className={`telemetry-stat${critical ? " telemetry-stat-critical" : ""}`}>
      <div className="telemetry-stat-head">
        <div className="telemetry-stat-label">{label}</div>
        <div className="telemetry-stat-value">{value}</div>
      </div>
      {sub && <div className="telemetry-stat-sub">{sub}</div>}
      <Sparkline values={series} maxHint={maxHint} critical={critical} />
    </div>
  );
}

interface SparklineProps {
  values: number[];
  maxHint?: number;
  critical?: boolean;
}

function Sparkline({ values, maxHint, critical }: SparklineProps) {
  // Use a fixed viewBox; CSS scales the SVG to the container width. The
  // viewBox aspect ratio (160 × 32) matches the .telemetry-sparkline height
  // in globals.css.
  const W = 160;
  const H = 32;
  const PAD = 1;

  if (values.length < 2) {
    return (
      <svg className="telemetry-sparkline" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
        <line x1={0} x2={W} y1={H - PAD} y2={H - PAD} className="spark-baseline" />
      </svg>
    );
  }

  const max = Math.max(maxHint ?? 0, ...values, 1);
  const min = Math.min(0, ...values);
  const span = max - min || 1;
  const step = W / Math.max(values.length - 1, 1);

  const points = values
    .map((v, i) => {
      const x = i * step;
      const y = H - PAD - ((v - min) / span) * (H - PAD * 2);
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");

  return (
    <svg
      className={`telemetry-sparkline${critical ? " spark-critical" : ""}`}
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
    >
      <polyline points={points} className="spark-line" />
    </svg>
  );
}

interface DerivedRates {
  netRx: number;
  netTx: number;
  netTotal: number;
  netSeries: number[];
  blockR: number;
  blockW: number;
  blockTotal: number;
  blockSeries: number[];
}

/**
 * Build per-sample bytes/sec rates from the cumulative counters in `window`.
 * The output series are aligned to indices 1..N (we drop the first sample
 * because there's no prior to diff against). For the latest stats line we
 * return the *most recent* derived rate.
 */
function computeRates(window: TelemetrySample[]): DerivedRates {
  const netSeries: number[] = [];
  const blockSeries: number[] = [];
  let lastNetRx = 0;
  let lastNetTx = 0;
  let lastBlockR = 0;
  let lastBlockW = 0;

  for (let i = 1; i < window.length; i++) {
    const prev = window[i - 1];
    const cur = window[i];
    const dtSec = Math.max((cur.tsMs - prev.tsMs) / 1000, 0.001);
    const netRx = Math.max(0, (cur.netRxBytes - prev.netRxBytes) / dtSec);
    const netTx = Math.max(0, (cur.netTxBytes - prev.netTxBytes) / dtSec);
    const blkR = Math.max(0, (cur.blockReadBytes - prev.blockReadBytes) / dtSec);
    const blkW = Math.max(0, (cur.blockWriteBytes - prev.blockWriteBytes) / dtSec);
    netSeries.push(netRx + netTx);
    blockSeries.push(blkR + blkW);
    lastNetRx = netRx;
    lastNetTx = netTx;
    lastBlockR = blkR;
    lastBlockW = blkW;
  }

  return {
    netRx: lastNetRx,
    netTx: lastNetTx,
    netTotal: lastNetRx + lastNetTx,
    netSeries,
    blockR: lastBlockR,
    blockW: lastBlockW,
    blockTotal: lastBlockR + lastBlockW,
    blockSeries,
  };
}

function formatRate(bytesPerSec: number): string {
  if (bytesPerSec < 1024) return `${bytesPerSec.toFixed(0)} B/s`;
  if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} kB/s`;
  if (bytesPerSec < 1024 * 1024 * 1024)
    return `${(bytesPerSec / (1024 * 1024)).toFixed(2)} MB/s`;
  return `${(bytesPerSec / (1024 * 1024 * 1024)).toFixed(2)} GB/s`;
}
