/**
 * Final scan deliverable — the "visibility map" the user came for. Renders
 * on top of the Report page (above the step timeline + debug log) when the
 * backend emitted `run://report`.
 *
 * Three blocks:
 *   1. **Headline** — large CPU-reduction-available figure + critical/warning
 *      counts. This is the answer to "what should I do about this service?".
 *   2. **Critical issues** — up to 3 cards, each with function, category,
 *      self-time bar, and collapsible example stack.
 *   3. **Warnings** — up to 10 denser rows, same anatomy.
 *   4. **Architecture advice** — 3-5 LLM-synthesised bullets.
 */

import { useState } from "react";

import type { Issue, VisibilityMap } from "../lib/tauri";

interface Props {
  map: VisibilityMap;
}

export default function VisibilityMapPanel({ map }: Props) {
  const cpuReduction = map.estimatedCpuReductionPct;
  const totalCounts = map.critical.length + map.warnings.length;

  return (
    <div className="vis-map">
      <div className="vis-headline">
        <div className="vis-headline-figure">
          {cpuReduction >= 1 ? `~${cpuReduction.toFixed(0)}%` : "<1%"}
          <span className="vis-headline-figure-sub">CPU reduction available</span>
        </div>
        <div className="vis-headline-counts">
          <span className="vis-count vis-count-critical">
            <strong>{map.critical.length}</strong> critical
          </span>
          <span className="vis-count vis-count-warning">
            <strong>{map.warnings.length}</strong> warnings
          </span>
          <span className="vis-count">
            <strong>{totalCounts}</strong> total
          </span>
        </div>
      </div>

      {map.critical.length > 0 && (
        <section className="vis-section vis-section-critical">
          <h3 className="vis-section-title">Critical issues</h3>
          <div className="vis-issues">
            {map.critical.map((issue, i) => (
              <IssueRow key={`c-${i}`} issue={issue} dense={false} />
            ))}
          </div>
        </section>
      )}

      {map.warnings.length > 0 && (
        <section className="vis-section vis-section-warning">
          <h3 className="vis-section-title">Warnings</h3>
          <div className="vis-issues vis-issues-dense">
            {map.warnings.map((issue, i) => (
              <IssueRow key={`w-${i}`} issue={issue} dense={true} />
            ))}
          </div>
        </section>
      )}

      {map.architectureAdvice.length > 0 && (
        <section className="vis-section vis-section-advice">
          <h3 className="vis-section-title">Architecture top advice</h3>
          <ul className="vis-advice-list">
            {map.architectureAdvice.map((bullet, i) => (
              <li key={i} className="vis-advice-item">
                {bullet}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function IssueRow({ issue, dense }: { issue: Issue; dense: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const barWidth = `${Math.max(2, Math.min(100, issue.self_pct))}%`;

  return (
    <div
      className={`vis-issue vis-issue-${issue.severity}${dense ? " vis-issue-dense" : ""}`}
    >
      <div className="vis-issue-head">
        <span className="vis-issue-category">{issue.category}</span>
        <code className="vis-issue-function">{issue.function}</code>
        <span className="vis-issue-pct">{issue.self_pct.toFixed(1)}%</span>
      </div>
      <div className="vis-issue-bar">
        <div className="vis-issue-bar-fill" style={{ width: barWidth }} />
      </div>
      {!dense && (
        <button
          type="button"
          className="vis-issue-toggle"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "hide stack" : "show stack"}
        </button>
      )}
      {expanded && (
        <pre className="vis-issue-stack">{formatStack(issue.example_stack)}</pre>
      )}
    </div>
  );
}

function formatStack(stack: string): string {
  // analyse_samples produces `frame_a;frame_b;leaf`. Render leaf-first for
  // readability — the leaf is what's burning CPU.
  return stack.split(";").reverse().join("\n  ↳ ");
}
