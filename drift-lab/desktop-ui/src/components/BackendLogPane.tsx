/**
 * Mirror of the backend's `tracing` pipeline. Same lines that land on stderr
 * (with `RUST_LOG=info,drift=debug`) show up here so the user can see what
 * the Rust side is doing without opening a terminal.
 *
 * Slotted as the second tab on the running screen's right pane (alongside
 * `TelemetryPane`). Auto-scrolls to the bottom as new lines arrive, but
 * only if the user is already at the bottom — pinning to a specific line
 * if they've scrolled up to read something.
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { useRunStore } from "../store/runStore";

export default function BackendLogPane() {
  const lines = useRunStore((s) => s.backendLog);
  const containerRef = useRef<HTMLDivElement | null>(null);
  // Stick to the bottom unless the user explicitly scrolled up — same idea
  // as a terminal: new output should appear unless you're reading history.
  const [pinned, setPinned] = useState(true);

  useLayoutEffect(() => {
    if (!pinned || !containerRef.current) return;
    containerRef.current.scrollTop = containerRef.current.scrollHeight;
  }, [lines, pinned]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onScroll = () => {
      // 24px tolerance so the pin doesn't flicker when scrollbars
      // re-measure during a flood of new lines.
      const atBottom =
        el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      setPinned(atBottom);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  if (lines.length === 0) {
    return (
      <div className="backend-log">
        <div className="backend-log-empty">
          No backend log lines yet — `tracing::info!` events will stream in
          here once a scan starts.
        </div>
      </div>
    );
  }

  return (
    <div className="backend-log" ref={containerRef}>
      {lines.map((l, i) => (
        <LogRow key={`${l.tsMs}-${i}`} line={l} />
      ))}
    </div>
  );
}

function LogRow({ line }: { line: { tsMs: number; level: string; target: string; message: string } }) {
  const levelClass = `bl-level bl-level-${line.level.toLowerCase()}`;
  return (
    <div className="bl-row">
      <span className="bl-ts">{formatTime(line.tsMs)}</span>
      <span className={levelClass}>{line.level}</span>
      <span className="bl-target">{shortenTarget(line.target)}</span>
      <span className="bl-msg">{line.message}</span>
    </div>
  );
}

/** HH:MM:SS.mmm — the seconds resolution is what you usually want when
 *  correlating against a tool call. */
function formatTime(ms: number): string {
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const fff = String(d.getMilliseconds()).padStart(3, "0");
  return `${hh}:${mm}:${ss}.${fff}`;
}

/** `drift_lab_lib::agent::workflow` is noisy; trim to the leaf. */
function shortenTarget(target: string): string {
  const parts = target.split("::");
  if (parts.length <= 2) return target;
  return parts.slice(-2).join("::");
}
