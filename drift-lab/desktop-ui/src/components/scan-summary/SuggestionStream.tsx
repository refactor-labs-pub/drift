import { useMemo } from "react";

import { FINDING_KIND_LABEL, SEVERITY_COLORS, type Severity } from "./types";
import type { ListedFinding } from "../../lib/tauri";
import DiffView from "./DiffView";
import { parseSuggestion } from "./parseSuggestion";

/**
 * Render the per-finding "Study this" list, GitHub-PR style.
 *
 * Each row carries the analyzer-side context up top (severity badge, file:
 * line, kind, name) and a "Study this" affordance on the right. Until the
 * user clicks Study, no LLM call has happened — the row is informational
 * only. After clicking, the model output flows into the row in three
 * possible shapes:
 *
 *   - two reasoning panels (`problem_description_reasoning:` and
 *     `solution_description_reasoning:`) explaining the issue and the fix,
 *   - the short `Why:` rationale line,
 *   - a colored unified diff (red = removed, green = added) once the
 *     ```diff fence has streamed in.
 *
 * The split is decided per-render by {@link parseSuggestion}; the renderer
 * itself holds no state.
 *
 * Streaming caret behavior:
 *   - Prose phase: caret blinks at the tail of the latest streaming text.
 *   - Diff phase: caret moves into the diff view, blinking on its own
 *     row beneath the last diff line — same "more code on the way" cue
 *     a developer sees in a CI streaming log.
 */

export interface SuggestionRowVM {
  index: number;
  source: "immediate_fix" | "refactor_candidate" | "finding_top";
  kind: string;
  severity: string;
  file: string;
  line: number;
  name: string;
  body: string;
  isStreaming: boolean;
}

interface Props {
  findings: ListedFinding[] | null;
  findingsError: string | null;
  rows: Map<number, SuggestionRowVM>;
  studying: Set<number>;
  onStudy: (index: number) => void;
  onStop: (index: number) => void;
}

export default function SuggestionStream({
  findings,
  findingsError,
  rows,
  studying,
  onStudy,
  onStop,
}: Props) {
  return (
    <div className="scan-suggestions">
      <div className="scan-suggestions-head">
        <div className="scan-suggestions-title">findings</div>
        <div className="muted">
          {findings === null && !findingsError && "loading findings…"}
          {findingsError && `error: ${findingsError}`}
          {findings && `${findings.length} actionable`}
        </div>
      </div>

      {findings && findings.length === 0 && (
        <div className="scan-empty">
          No actionable findings — the analyzer didn't surface anything in
          the top lanes. Nothing to study.
        </div>
      )}

      {findings?.map((f, i) => (
        <FindingRow
          key={i}
          finding={f}
          row={rows.get(i)}
          isStudying={studying.has(i)}
          onStudy={() => onStudy(i)}
          onStop={() => onStop(i)}
        />
      ))}
    </div>
  );
}

function FindingRow({
  finding,
  row,
  isStudying,
  onStudy,
  onStop,
}: {
  finding: ListedFinding;
  row: SuggestionRowVM | undefined;
  isStudying: boolean;
  onStudy: () => void;
  onStop: () => void;
}) {
  const sevColor = SEVERITY_COLORS[finding.severity as Severity] ?? "#999";
  const hasResult = !!row && row.body.length > 0;
  const isStreaming = isStudying && !!row?.isStreaming;
  const rowClass = isStreaming
    ? "scan-suggestion-row is-streaming"
    : "scan-suggestion-row";
  const kindLabel =
    FINDING_KIND_LABEL[finding.kind as keyof typeof FINDING_KIND_LABEL] ??
    finding.kind.replace(/_/g, " ");

  return (
    <div className={rowClass}>
      <div className="scan-suggestion-meta">
        <span className="scan-mini-badge" style={{ background: sevColor }}>
          {finding.severity}
        </span>
        <strong>{finding.name || "(unnamed)"}</strong>
        <span className="muted">· {kindLabel}</span>
        <span className="muted">·</span>
        <code className="scan-code">
          {finding.file}:{finding.line}
        </code>
        <span className="muted" style={{ marginLeft: "auto" }}>
          {finding.source.replace(/_/g, " ")}
        </span>
        {isStreaming ? (
          <button
            type="button"
            className="scan-stop-btn scan-stop-btn-inline"
            onClick={onStop}
            title="Cancel the in-flight LLM suggestion for this finding."
          >
            <span className="scan-stop-btn-icon" aria-hidden />
            Stop
          </button>
        ) : (
          <button
            type="button"
            className="scan-study-btn"
            onClick={onStudy}
            title={
              hasResult
                ? "Re-run the LLM suggestion for this finding."
                : "Ask the model to explain this finding and suggest a fix."
            }
          >
            {hasResult ? "Study again" : "Study this"}
          </button>
        )}
      </div>

      {finding.message && !hasResult && (
        <div className="scan-finding-message muted">{finding.message}</div>
      )}

      {row && <SuggestionBody body={row.body} streaming={isStreaming} />}
    </div>
  );
}

/**
 * Render the parsed suggestion. Shows the two reasoning panels first
 * (when present), then the `Why:` rationale, then the colored diff.
 */
function SuggestionBody({ body, streaming }: { body: string; streaming: boolean }) {
  const parsed = useMemo(() => parseSuggestion(body), [body]);

  // Decide where the streaming caret lives:
  //   - inside the diff, if we've entered diff mode and it's still open
  //   - at the tail of the rationale, if the rationale is the last prose surface
  //   - at the tail of the solution reasoning, if we're still streaming that
  //   - at the tail of the problem reasoning, if that's all we've seen
  const caretSite =
    streaming && !parsed.inDiff
      ? parsed.rationale
        ? "rationale"
        : parsed.solutionReasoning
          ? "solution"
          : "problem"
      : null;

  const nothingYet =
    !parsed.problemReasoning &&
    !parsed.solutionReasoning &&
    !parsed.rationale &&
    !parsed.inDiff;
  if (nothingYet) {
    return (
      <div className="scan-suggestion-prose">
        {streaming ? " " : "(no suggestion yet)"}
        {streaming && <span className="scan-suggestion-caret" aria-hidden />}
      </div>
    );
  }

  return (
    <div className="scan-suggestion-result">
      {parsed.problemReasoning && (
        <ReasoningPanel
          label="Problem"
          tone="problem"
          text={parsed.problemReasoning}
          showCaret={caretSite === "problem"}
        />
      )}
      {parsed.solutionReasoning && (
        <ReasoningPanel
          label="Solution"
          tone="solution"
          text={parsed.solutionReasoning}
          showCaret={caretSite === "solution"}
        />
      )}
      {parsed.rationale && (
        <div className="scan-suggestion-rationale">
          <span className="scan-rationale-tag">Why</span>
          {parsed.rationale}
          {caretSite === "rationale" && (
            <span className="scan-suggestion-caret" aria-hidden />
          )}
        </div>
      )}
      {parsed.inDiff && (
        <DiffView
          lines={parsed.diffLines}
          streaming={streaming && !parsed.diffComplete}
        />
      )}
    </div>
  );
}

function ReasoningPanel({
  label,
  tone,
  text,
  showCaret,
}: {
  label: string;
  tone: "problem" | "solution";
  text: string;
  showCaret: boolean;
}) {
  return (
    <div className="scan-reasoning-panel">
      <div className={`scan-reasoning-label scan-reasoning-label--${tone}`}>
        {label}
      </div>
      <div className="scan-reasoning-text">
        {text}
        {showCaret && <span className="scan-suggestion-caret" aria-hidden />}
      </div>
    </div>
  );
}
