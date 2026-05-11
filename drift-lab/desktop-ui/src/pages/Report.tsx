import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import Orbs from "../components/Orbs";
import type { Entry as AgentLogEntry } from "../components/ReasoningLog";
import VisibilityMapPanel from "../components/VisibilityMapPanel";
import { CheckIcon, STEP_ICONS, XIcon } from "../components/icons";
import { startAgentRun } from "../lib/tauri";
import { useRunStore } from "../store/runStore";

/**
 * Post-run summary. Reads the most-recent run out of the store — there is no
 * persistence layer yet, so navigating to a stale runId after a refresh will
 * show "no data". The page renders three sections plus actions:
 *
 *   1. Header — project + goal + run id + clock.
 *   2. Steps — the same five-stage timeline from the live view, but each step
 *      now shows its full `detail` (the model's prose + tool-output summary).
 *   3. Agent log (debug) — every `AgentEvent` the workflow forwarded:
 *      reasoning text, tool dispatch with arguments, full tool output JSON.
 *      Tool outputs are collapsible so the page stays scannable but the raw
 *      data is one click away when something looks off.
 *
 * Rerun re-fires the same scan via `startAgentRun` with the persisted
 * `runParams`, then navigates back to Home where the live timeline takes over.
 */
export default function ReportPage() {
  const { runId: routeRunId } = useParams<{ runId: string }>();
  const navigate = useNavigate();

  const {
    runId,
    runParams,
    result,
    error,
    steps,
    logEntries,
    visibilityMap,
    startedAt,
    endedAt,
    beginRun,
    setLogEntries,
  } = useRunStore();

  // The store keeps only the most-recent run. If the URL points at a run that
  // doesn't match what's currently in the store, we don't have data for it.
  const haveData = runId != null && runId === routeRunId;

  const handleRerun = async () => {
    if (!runParams) return;
    try {
      setLogEntries([]); // clear stale entries before the new POST returns
      const id = await startAgentRun(runParams.projectPath, {
        mode: runParams.mode,
        goalPrompt: runParams.goalPrompt,
      });
      beginRun(id, runParams);
      navigate("/");
    } catch (e) {
      // Surfacing the error in this page is fine; the user is already here.
      // We don't have a `failRun` slot on the store from here without a
      // fresh begin, so just log + alert.
      console.error("rerun failed", e);
      alert(`Rerun failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  return (
    <div className="report-page">
      <Orbs />
      <div className="report-card">
        <div className="report-head">
          <div>
            <h1>Run report</h1>
            <div className="muted report-meta">
              <span>Run · <code>{routeRunId}</code></span>
              {runParams && <span>Path · <code>{runParams.projectPath}</code></span>}
              {startedAt && <span>{new Date(startedAt).toLocaleTimeString()}{endedAt ? ` → ${new Date(endedAt).toLocaleTimeString()}` : ""}</span>}
            </div>
            {runParams?.goalPrompt && (
              <div className="report-goal">Goal · {runParams.goalPrompt}</div>
            )}
          </div>
          <div className="report-actions">
            <button
              type="button"
              className="primary-btn"
              onClick={handleRerun}
              disabled={!runParams}
              title={runParams ? "Rerun the same scan" : "No run in memory"}
            >
              ↻ Rerun
            </button>
            <button type="button" className="ghost-btn" onClick={() => navigate("/")}>
              ← Back
            </button>
          </div>
        </div>

        {!haveData && (
          <div className="report-empty">
            No data for this run. The store only keeps the most-recent scan;
            run a new one from the home screen.
          </div>
        )}

        {haveData && (
          <>
            {visibilityMap ? (
              <VisibilityMapPanel map={visibilityMap} />
            ) : (
              result && (
                <div className="report-summary">
                  <div>
                    <strong>{result.issuesFound}</strong> issues found
                    {" · "}
                    <strong>{result.criticalCount}</strong> critical
                  </div>
                </div>
              )
            )}
            {error && <div className="report-error">Error: {error}</div>}

            <h2 className="report-section-title">Steps</h2>
            <ReportSteps steps={steps} />

            <h2 className="report-section-title">
              Agent log <span className="muted">({logEntries.length} events)</span>
            </h2>
            <DebugLog entries={logEntries} />
          </>
        )}
      </div>
    </div>
  );
}

function ReportSteps({ steps }: { steps: ReturnType<typeof useRunStore.getState>["steps"] }) {
  return (
    <div className="report-steps">
      {steps.map((s, i) => {
        const stateClass = s.status === "pending" ? "" : s.status;
        return (
          <div key={i} className={`report-step ${stateClass}`}>
            <div className="report-step-icon">
              {s.status === "done"
                ? <CheckIcon />
                : s.status === "error"
                  ? <XIcon />
                  : STEP_ICONS[i]}
            </div>
            <div className="report-step-body">
              <div className="report-step-title">
                {s.title}
                <span className="report-step-status">{s.status}</span>
              </div>
              <div className="report-step-detail">{s.detail}</div>
            </div>
            <div className="report-step-time">
              {s.durationMs != null ? `${(s.durationMs / 1000).toFixed(1)}s` : "—"}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** Full agent log with collapsible tool outputs. The live `ReasoningLog`
 *  component truncates content to keep the running UI scannable; here we
 *  show full content because the user is on the report specifically to
 *  debug what the agent did. */
function DebugLog({ entries }: { entries: AgentLogEntry[] }) {
  if (entries.length === 0) {
    return (
      <div className="report-empty-log">
        No agent events captured. The scan may have failed before the model
        emitted any output.
      </div>
    );
  }
  return (
    <div className="report-log">
      {entries.map((e) => (
        <DebugLogRow key={e.id} entry={e} />
      ))}
    </div>
  );
}

function DebugLogRow({ entry }: { entry: AgentLogEntry }) {
  const [expanded, setExpanded] = useState(false);

  switch (entry.kind) {
    case "thinking":
      return (
        <div className="report-log-row report-log-thinking">
          <span className="report-log-icon">💭</span>
          <pre className="report-log-text">{entry.text}</pre>
        </div>
      );
    case "tool": {
      const elapsedMs = entry.result ? entry.result.finishedAt - entry.startedAt : null;
      const status = entry.result
        ? entry.result.isError
          ? "error"
          : "ok"
        : "pending";
      return (
        <div className={`report-log-row report-log-tool report-log-tool-${status}`}>
          <span className="report-log-icon">
            {status === "pending" ? "⚙" : status === "ok" ? "✓" : "✗"}
          </span>
          <div className="report-log-tool-body">
            <div className="report-log-tool-line">
              <span className="report-log-tool-name">{entry.name}</span>
              <span className="report-log-tool-args">{entry.args}</span>
              {elapsedMs != null && (
                <span className="report-log-elapsed">{(elapsedMs / 1000).toFixed(2)}s</span>
              )}
              {entry.result && (
                <button
                  type="button"
                  className="report-log-toggle"
                  onClick={() => setExpanded((v) => !v)}
                >
                  {expanded ? "hide output" : "show output"}
                </button>
              )}
            </div>
            {entry.result && expanded && (
              <pre className="report-log-result">
                {entry.result.isError ? "error: " : ""}
                {prettify(entry.result.content)}
              </pre>
            )}
          </div>
        </div>
      );
    }
    case "approval":
      return (
        <div className="report-log-row report-log-approval">
          <span className="report-log-icon">⚠</span>
          <span className="report-log-text">
            <strong>{entry.name}</strong> needs approval — {entry.args}
          </span>
        </div>
      );
    case "error":
      return (
        <div className="report-log-row report-log-error">
          <span className="report-log-icon">✗</span>
          <pre className="report-log-text">{entry.message}</pre>
        </div>
      );
    case "done":
      return (
        <div className="report-log-row report-log-done">
          <span className="report-log-icon">●</span>
          <span className="report-log-text">scan complete</span>
        </div>
      );
  }
}

/** Pretty-print JSON when content parses, otherwise return the raw string.
 *  Keeps tool outputs readable on the report page without an external dep. */
function prettify(content: string): string {
  const trimmed = content.trim();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      return JSON.stringify(JSON.parse(trimmed), null, 2);
    } catch {
      // fall through
    }
  }
  return content;
}

