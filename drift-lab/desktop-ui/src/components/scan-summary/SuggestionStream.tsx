import { useMemo } from "react";

import type { ScanSuggestionPayload } from "../../lib/tauri";
import { SEVERITY_COLORS, type Severity } from "./types";

/**
 * Render the streamed list of LLM suggestions. One row per finding the
 * suggester surfaced — newest at the bottom so the user can read in scan
 * order while new items append below.
 *
 * Each row carries the analyzer-side context (kind / severity / file:line)
 * up top so the suggestion body underneath has a clear anchor. The body is
 * model output: free-form markdown with a `Why:` line followed by a fenced
 * code block. We render it as monospaced pre-wrap text, no markdown parser
 * — the system prompt asks for a compact shape and the model output stays
 * readable as plain text.
 */

interface Props {
  suggestions: ScanSuggestionPayload[];
  isDone: boolean;
  totalExpected: number | null;
}

export default function SuggestionStream({
  suggestions,
  isDone,
  totalExpected,
}: Props) {
  const sorted = useMemo(
    () => [...suggestions].sort((a, b) => a.index - b.index),
    [suggestions],
  );
  return (
    <div className="scan-suggestions">
      <div className="scan-suggestions-head">
        <div className="scan-suggestions-title">code suggestions</div>
        <div className="muted">
          {isDone
            ? `${sorted.length} suggestion${sorted.length === 1 ? "" : "s"}`
            : totalExpected != null
              ? `streaming · ${sorted.length} so far`
              : `streaming · ${sorted.length}`}
        </div>
      </div>
      {sorted.length === 0 && !isDone && (
        <div className="scan-empty">waiting for the model…</div>
      )}
      {sorted.map((s) => (
        <div key={s.index} className="scan-suggestion-row">
          <div className="scan-suggestion-meta">
            <span
              className="scan-mini-badge"
              style={{ background: SEVERITY_COLORS[s.severity as Severity] ?? "#999" }}
            >
              {s.severity}
            </span>
            <strong>{s.name || "(unnamed)"}</strong>
            <span className="muted">· {s.kind.replace(/_/g, " ")}</span>
            <span className="muted">·</span>
            <code className="scan-code">{s.file}:{s.line}</code>
            <span className="muted" style={{ marginLeft: "auto" }}>{s.source.replace(/_/g, " ")}</span>
          </div>
          <pre className="scan-suggestion-body">{s.suggestion.trim()}</pre>
        </div>
      ))}
    </div>
  );
}
