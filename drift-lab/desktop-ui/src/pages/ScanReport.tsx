import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import Orbs from "../components/Orbs";
import ScanSummary from "../components/scan-summary/ScanSummary";
import SuggestionStream from "../components/scan-summary/SuggestionStream";
import type { Report } from "../components/scan-summary/types";
import {
  loadStaticScan,
  onScanSuggestion,
  onScanSuggestionDone,
  startScanSuggestions,
  type ScanSuggestionDone,
  type ScanSuggestionPayload,
} from "../lib/tauri";

/**
 * Static-scan report — loads a saved scan from `~/.drift/scans/<scanId>.json`
 * and renders the summary cards. The "Generate code suggestions" button
 * kicks off the per-finding LLM driver and streams suggestions inline.
 *
 * This page is reached from Home after `scan://complete` fires. The URL is
 * bookmarkable — the report can be re-opened any time because the scan is
 * on disk.
 *
 * The LLM provider must be configured for the suggestion phase. If it
 * isn't, `startScanSuggestions` rejects with `"backend not configured"` and
 * we render the error inline next to the button.
 */
export default function ScanReportPage() {
  const { scanId } = useParams<{ scanId: string }>();
  const navigate = useNavigate();

  const [report, setReport] = useState<Report | null>(null);
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [suggestions, setSuggestions] = useState<ScanSuggestionPayload[]>([]);
  const [suggestionDone, setSuggestionDone] = useState<ScanSuggestionDone | null>(null);
  const [suggestionsStarted, setSuggestionsStarted] = useState(false);
  const [suggestionError, setSuggestionError] = useState<string | null>(null);

  // Pin the active scan id for the event filters — even though the URL
  // param shouldn't change while this page is mounted, the listener pattern
  // is uniform with Home and avoids any closure-staleness surprise.
  const scanIdRef = useRef<string | undefined>(scanId);
  useEffect(() => {
    scanIdRef.current = scanId;
  }, [scanId]);

  // Load the saved scan once on mount (or on scanId change). Failure is
  // surfaced inline — typically means the user navigated to a deleted id.
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
    return () => {
      cancelled = true;
    };
  }, [scanId]);

  // Suggestion-stream subscriptions, installed for the page lifetime.
  useEffect(() => {
    const cleanup: Array<() => void> = [];
    const isMine = (id: string) => scanIdRef.current === id;
    (async () => {
      cleanup.push(
        await onScanSuggestion((s) => {
          if (!isMine(s.scanId)) return;
          setSuggestions((prev) =>
            prev.some((p) => p.index === s.index) ? prev : [...prev, s],
          );
        }),
      );
      cleanup.push(
        await onScanSuggestionDone((d) => {
          if (!isMine(d.scanId)) return;
          setSuggestionDone(d);
        }),
      );
    })();
    return () => {
      cleanup.forEach((fn) => fn());
    };
  }, []);

  const handleStartSuggestions = useCallback(async () => {
    if (!scanId || suggestionsStarted) return;
    setSuggestions([]);
    setSuggestionDone(null);
    setSuggestionError(null);
    setSuggestionsStarted(true);
    try {
      await startScanSuggestions(scanId);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setSuggestionError(msg);
      setSuggestionsStarted(false);
    }
  }, [scanId, suggestionsStarted]);

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

        {report && (
          <>
            <ScanSummary report={report} />

            <div className="scan-suggestions-cta">
              <button
                type="button"
                className="scan-start-btn"
                onClick={handleStartSuggestions}
                disabled={suggestionsStarted}
                title="Reads findings_top, immediate_fixes, refactor_candidates from the saved scan, opens a ±30-line code window per finding, and asks the LLM for one concrete fix per item."
              >
                {suggestionsStarted
                  ? suggestionDone
                    ? "Suggestions complete"
                    : "Generating…"
                  : "Generate code suggestions"}
              </button>
              {suggestionError && (
                <span style={{ color: "#c82626", fontSize: 12 }}>
                  {suggestionError}
                </span>
              )}
            </div>

            {suggestionsStarted && (
              <SuggestionStream
                suggestions={suggestions}
                isDone={!!suggestionDone}
                totalExpected={suggestionDone?.total ?? null}
              />
            )}
          </>
        )}
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
