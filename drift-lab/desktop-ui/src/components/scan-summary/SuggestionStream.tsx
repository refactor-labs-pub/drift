import { useMemo } from "react";

import { FINDING_KIND_LABEL, SEVERITY_COLORS, type Severity } from "./types";
import type { ListedFinding, SavedSuggestion } from "../../lib/tauri";
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
  /// What gets rendered right now. Drives the SuggestionBody view.
  /// During a live stream, this is the accumulating delta buffer.
  /// Otherwise, it's `versions[cursor]?.suggestion` projected here for
  /// rendering. Keeping a single string here means the renderer doesn't
  /// have to know about version state — clean separation.
  body: string;
  isStreaming: boolean;
  /// Full version history for this finding, newest first. Empty if
  /// nothing has ever been studied (no saved file on disk and no live
  /// stream has finalized yet). When non-empty, `cursor` indexes into
  /// this array to pick which version `body` mirrors.
  ///
  /// `versions[0]` is always the newest. The UI's "← / →" controls move
  /// `cursor` up (older) or down (newer); `cursor === 0` means "showing
  /// the latest version".
  versions: SavedSuggestion[];
  /// Active version index. Defaults to 0 (newest). Ignored while
  /// `isStreaming` is true — the live buffer is shown instead.
  cursor: number;
}

interface Props {
  findings: ListedFinding[] | null;
  findingsError: string | null;
  rows: Map<number, SuggestionRowVM>;
  studying: Set<number>;
  onStudy: (index: number) => void;
  /// Step the version cursor for one row. `direction = -1` shows older;
  /// `+1` shows newer. The parent owns the rowsRef so the mutation lives
  /// there, not in this presentation-only component.
  onCursorStep: (index: number, direction: -1 | 1) => void;
  onStop: (index: number) => void;
}

export default function SuggestionStream({
  findings,
  findingsError,
  rows,
  studying,
  onStudy,
  onStop,
  onCursorStep,
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
          onCursorStep={(dir) => onCursorStep(i, dir)}
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
  onCursorStep,
}: {
  finding: ListedFinding;
  row: SuggestionRowVM | undefined;
  isStudying: boolean;
  onStudy: () => void;
  onStop: () => void;
  onCursorStep: (direction: -1 | 1) => void;
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
  const versionCount = row?.versions.length ?? 0;
  const cursor = row?.cursor ?? 0;
  // "v3 of 5" — cursor=0 is the newest, so the displayed version number
  // counts down: versionCount - cursor. We only show the indicator when
  // there's actual history (>=2 versions) to avoid clutter on the
  // first-ever study.
  const showVersionNav = !isStreaming && versionCount >= 2;
  const currentVersionLabel = versionCount > 0
    ? `v${versionCount - cursor}/${versionCount}`
    : null;
  const currentSavedAt = row?.versions[cursor]?.savedAt;

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
        <div className="scan-suggestion-meta-actions">
          <span className="muted">
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
                  ? "Re-run the LLM suggestion for this finding (appends a new version, doesn't replace history)."
                  : "Ask the model to explain this finding and suggest a fix."
              }
            >
              {hasResult ? "Study again" : "Study this"}
            </button>
          )}
        </div>
      </div>

      {showVersionNav && (
        <div className="scan-suggestion-version-nav muted">
          <button
            type="button"
            className="scan-version-step"
            onClick={() => onCursorStep(1)}
            disabled={cursor + 1 >= versionCount}
            title="Show the previous (older) version"
            aria-label="Previous version"
          >
            ←
          </button>
          <span className="scan-version-label">{currentVersionLabel}</span>
          <button
            type="button"
            className="scan-version-step"
            onClick={() => onCursorStep(-1)}
            disabled={cursor === 0}
            title="Show the next (newer) version"
            aria-label="Next version"
          >
            →
          </button>
          {currentSavedAt && (
            <span className="scan-version-savedat" title={currentSavedAt}>
              · saved {formatRelativeShort(currentSavedAt)}
            </span>
          )}
        </div>
      )}

      {finding.message && !hasResult && (
        <div className="scan-finding-message muted">{finding.message}</div>
      )}

      {row && <SuggestionBody body={row.body} streaming={isStreaming} />}
    </div>
  );
}

/// Compact "Xm ago" / "Xh ago" formatter for the version-nav timestamp.
/// Kept inline (no shared date util in the codebase) — it's small and
/// only used here.
function formatRelativeShort(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const sec = Math.max(0, Math.floor((Date.now() - t) / 1000));
  if (sec < 60) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day}d ago`;
  return new Date(t).toLocaleDateString();
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
