import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import ActiveModelBadge from "../components/ActiveModelBadge";
import MagicOrb from "../components/MagicOrb";
import Orbs from "../components/Orbs";
import ScanSummary from "../components/scan-summary/ScanSummary";
import SuggestionStream, {
  type SuggestionRowVM,
} from "../components/scan-summary/SuggestionStream";
import type { Report } from "../components/scan-summary/types";
import {
  listScanFindings,
  loadStaticScan,
  onScanSuggestion,
  onScanSuggestionDelta,
  onScanSuggestionDone,
  onScanSuggestionStart,
  startScanFindingSuggestion,
  stopScanFindingSuggestion,
  type ListedFinding,
  type ScanSuggestionDeltaPayload,
  type ScanSuggestionDone,
  type ScanSuggestionPayload,
  type ScanSuggestionStartPayload,
} from "../lib/tauri";

/**
 * Static-scan report — loads a saved scan from `~/.drift/scans/<scanId>.json`
 * and renders the summary cards. **No automatic suggestion stream**: each
 * finding row carries a "Study this" button; the user opts into the LLM
 * round-trip per-finding, and multiple findings can be in flight at once
 * (each one is its own `(scan_id, index)` stream on the Rust side).
 *
 * ## Mount-time flow
 *
 *   1. Page mounts → `loadStaticScan(scanId)` + `listScanFindings(scanId)`
 *      fire in parallel; show loading orb.
 *   2. Data lands → fade in `<ScanSummary>` and the findings list. No LLM
 *      activity yet.
 *   3. User clicks "Study this" on a row → backend opens a per-finding
 *      stream; events keyed by `index` populate the matching row.
 *   4. Stream finishes (or user clicks Stop on that row) → the row's
 *      `isStreaming` flag clears.
 *
 * ## Streaming architecture
 *
 *   - `scan://suggestion-start` → row metadata pushed into `rowsRef`.
 *   - `scan://suggestion-delta` → text appended to the row in the ref.
 *   - `scan://suggestion`       → final body, clears `isStreaming`.
 *   - `scan://suggestion-done`  → per-(scan,index) completion; flips the
 *                                 "is this row currently studying" flag.
 *
 * All four handlers mutate the same `Map` ref synchronously and schedule a
 * single `requestAnimationFrame` flush via the tick reducer — one paint
 * per frame regardless of token rate.
 */
export default function ScanReportPage() {
  const { scanId } = useParams<{ scanId: string }>();
  const navigate = useNavigate();

  const [report, setReport] = useState<Report | null>(null);
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [findings, setFindings] = useState<ListedFinding[] | null>(null);
  const [findingsError, setFindingsError] = useState<string | null>(null);

  // Pin the active scan id for the event filters.
  const scanIdRef = useRef<string | undefined>(scanId);
  useEffect(() => {
    scanIdRef.current = scanId;
  }, [scanId]);

  // Streaming row state — ref-backed, RAF-flushed. Keyed on the finding
  // index (matches the backend's per-row identity).
  const rowsRef = useRef<Map<number, SuggestionRowVM>>(new Map());
  const [, bumpTick] = useReducer((n: number) => (n + 1) | 0, 0);
  const flushScheduled = useRef(false);
  const scheduleFlush = useCallback(() => {
    if (flushScheduled.current) return;
    flushScheduled.current = true;
    requestAnimationFrame(() => {
      flushScheduled.current = false;
      bumpTick();
    });
  }, []);

  // Per-row request flags so we can disable Study This while a stream is
  // mid-flight (and show Stop in its place). The set lives outside the
  // RAF-flushed map so a click flips immediately, not on the next frame.
  const [studying, setStudying] = useState<Set<number>>(new Set());

  // Load the saved scan + canonical finding list once on mount. Both
  // requests are cheap (single file read on the Rust side) and the two
  // surfaces don't depend on each other, so they fire in parallel.
  useEffect(() => {
    if (!scanId) return;
    let cancelled = false;
    (async () => {
      try {
        const stored = await loadStaticScan(scanId);
        if (cancelled) return;
        setReport(stored.report as Report);
        setSavedAt(stored.savedAt);
        setLoadError(null);
      } catch (e) {
        if (cancelled) return;
        setLoadError(e instanceof Error ? e.message : String(e));
      }
    })();
    (async () => {
      try {
        const f = await listScanFindings(scanId);
        if (cancelled) return;
        setFindings(f);
        setFindingsError(null);
      } catch (e) {
        if (cancelled) return;
        setFindingsError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [scanId]);

  // Suggestion-stream subscriptions, installed once for the page lifetime.
  useEffect(() => {
    const cleanup: Array<() => void> = [];
    const isMine = (id: string) => scanIdRef.current === id;
    (async () => {
      cleanup.push(
        await onScanSuggestionStart((s: ScanSuggestionStartPayload) => {
          if (!isMine(s.scanId)) return;
          rowsRef.current.set(s.index, {
            index: s.index,
            source: s.source,
            kind: s.kind,
            severity: s.severity,
            file: s.file,
            line: s.line,
            name: s.name,
            body: "",
            isStreaming: true,
          });
          scheduleFlush();
        }),
      );
      cleanup.push(
        await onScanSuggestionDelta((d: ScanSuggestionDeltaPayload) => {
          if (!isMine(d.scanId)) return;
          const row = rowsRef.current.get(d.index);
          if (!row) return;
          row.body += d.delta;
          scheduleFlush();
        }),
      );
      cleanup.push(
        await onScanSuggestion((s: ScanSuggestionPayload) => {
          if (!isMine(s.scanId)) return;
          rowsRef.current.set(s.index, {
            index: s.index,
            source: s.source,
            kind: s.kind,
            severity: s.severity,
            file: s.file,
            line: s.line,
            name: s.name,
            body: s.suggestion,
            isStreaming: false,
          });
          scheduleFlush();
        }),
      );
      cleanup.push(
        await onScanSuggestionDone((d: ScanSuggestionDone) => {
          if (!isMine(d.scanId)) return;
          // The done event doesn't carry the index (it counts total/failed
          // for the run). For a single-finding run the row's own
          // `scan://suggestion` event already cleared `isStreaming`; we
          // just need to drop the per-row "studying" flag for whichever
          // index just finalized. The row body itself is authoritative —
          // any row that isn't streaming is no longer "studying".
          setStudying((prev) => {
            if (prev.size === 0) return prev;
            const next = new Set(prev);
            for (const idx of prev) {
              const row = rowsRef.current.get(idx);
              if (row && !row.isStreaming) next.delete(idx);
            }
            return next;
          });
        }),
      );
    })();
    return () => {
      cleanup.forEach((fn) => fn());
    };
  }, [scheduleFlush]);

  const handleStudy = useCallback(
    async (index: number) => {
      if (!scanId) return;
      if (studying.has(index)) return;
      // Reset any prior body for this index — the user is re-running. Seed
      // a minimal row so the UI shows the streaming spinner instantly,
      // before the backend emits its first `suggestion-start` event.
      const seed = findings?.[index];
      if (seed) {
        rowsRef.current.set(index, {
          index,
          source: seed.source,
          kind: seed.kind,
          severity: seed.severity,
          file: seed.file,
          line: seed.line,
          name: seed.name,
          body: "",
          isStreaming: true,
        });
        scheduleFlush();
      }
      setStudying((prev) => {
        const next = new Set(prev);
        next.add(index);
        return next;
      });
      try {
        await startScanFindingSuggestion(scanId, index);
      } catch (e) {
        // Surface the error inline on the row and drop the studying flag.
        const msg = e instanceof Error ? e.message : String(e);
        const row = rowsRef.current.get(index);
        if (row) {
          row.body = `⚠ ${msg}`;
          row.isStreaming = false;
          scheduleFlush();
        }
        setStudying((prev) => {
          const next = new Set(prev);
          next.delete(index);
          return next;
        });
      }
    },
    [scanId, studying, findings, scheduleFlush],
  );

  const handleStopStudy = useCallback(
    async (index: number) => {
      if (!scanId) return;
      try {
        await stopScanFindingSuggestion(scanId, index);
      } catch {
        // Best-effort — the suggestion-done event still flips the flag.
      }
    },
    [scanId],
  );

  // Snapshot rows for the render — read once per RAF flush.
  const rows = rowsRef.current;

  return (
    <div className="scan-page">
      <Orbs />
      <div className="scan-page-card">
        <div className="scan-page-head">
          <div>
            <h1>Scan report</h1>
            <div className="muted">
              {scanId && <>scan id <code>{scanId.slice(0, 8)}…</code></>}
              {savedAt && (
                <>
                  {" · saved "}
                  <span title={savedAt}>{formatSavedAt(savedAt)}</span>
                </>
              )}
            </div>
          </div>
          <div className="scan-page-actions">
            <ActiveModelBadge compact />
            <button type="button" className="ghost-btn" onClick={() => navigate("/")}>
              ← Home
            </button>
          </div>
        </div>

        {loadError && (
          <div className="report-error" style={{ marginTop: 18 }}>
            {loadError}
          </div>
        )}

        {!report && !loadError && <ReportLoading />}

        {report && (
          <div className="scan-report-body">
            <ScanSummary report={report} />

            <SuggestionStream
              findings={findings}
              findingsError={findingsError}
              rows={rows}
              studying={studying}
              onStudy={handleStudy}
              onStop={handleStopStudy}
            />
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Loading affordance shown while the saved scan JSON is being fetched.
 * Centered orb with a pulsing aria-live label — modern, clean, matches the
 * Home running view's visual register (orb + caption). Fades in over 280ms
 * so the transition from navigate→render→content feels intentional rather
 * than flashing.
 */
function ReportLoading() {
  return (
    <div className="scan-report-loading" role="status" aria-live="polite">
      <MagicOrb />
      <div className="scan-report-loading-label">Loading scan…</div>
      <div className="scan-report-loading-sub muted">
        Reading the saved analysis from disk. Click "Study this" on any
        finding to ask the model for a fix.
      </div>
    </div>
  );
}

function formatSavedAt(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
