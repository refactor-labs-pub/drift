/**
 * Vertical timeline that focuses the operator on the *current* stage.
 *
 *   - Done / Error stages collapse to a one-line tick row (status + title +
 *     duration). Skimmable history, not a wall of cards.
 *   - The active stage expands to show the model's current sentence in
 *     prose. This is "what I'm doing right now".
 *   - Pending (not-yet-reached) stages stay hidden until the agent gets to
 *     them. They re-appear into the timeline as the run progresses.
 *
 * The component is presentational only — it reads `steps` from the parent.
 */

import type { StepState } from "../store/runStore";
import { CheckIcon, XIcon } from "./icons";

interface Props {
  steps: StepState[];
}

export default function CollapsingTimeline({ steps }: Props) {
  const activeIndex = steps.findIndex((s) => s.status === "active");
  const lastTouchedIndex = lastNonPending(steps);

  return (
    <div className="ctl">
      {steps.map((step, i) => {
        if (step.status === "pending") {
          // Don't render future stages — they're noise until reached.
          // (The exception: if NO step has been touched yet, show the first
          // one as a "what's coming" hint.)
          if (lastTouchedIndex === -1 && i === 0) {
            return <UpcomingRow key={i} step={step} />;
          }
          return null;
        }
        if (step.status === "active") {
          return <ActiveCard key={i} step={step} index={i} total={steps.length} />;
        }
        // Done / Error → collapsed row.
        return (
          <CollapsedRow
            key={i}
            step={step}
            isLastBefore={activeIndex === -1 ? false : i === activeIndex - 1}
          />
        );
      })}
    </div>
  );
}

function ActiveCard({
  step,
  index,
  total,
}: {
  step: StepState;
  index: number;
  total: number;
}) {
  return (
    <div className="ctl-active">
      <div className="ctl-active-head">
        <span className="ctl-active-counter">
          Step {index + 1} <span className="ctl-counter-of">of {total}</span>
        </span>
        <span className="ctl-active-title">{step.title}</span>
      </div>
      <div className="ctl-active-detail">{step.detail}</div>
    </div>
  );
}

function CollapsedRow({ step, isLastBefore }: { step: StepState; isLastBefore: boolean }) {
  const icon = step.status === "error" ? <XIcon /> : <CheckIcon />;
  const duration =
    step.durationMs != null ? `${(step.durationMs / 1000).toFixed(1)}s` : null;
  return (
    <div
      className={`ctl-row ctl-row-${step.status}${isLastBefore ? " ctl-row-last" : ""}`}
    >
      <div className="ctl-row-icon">{icon}</div>
      <div className="ctl-row-title">{step.title}</div>
      {duration && <div className="ctl-row-time">{duration}</div>}
    </div>
  );
}

function UpcomingRow({ step }: { step: StepState }) {
  return (
    <div className="ctl-row ctl-row-upcoming">
      <div className="ctl-row-icon">○</div>
      <div className="ctl-row-title">{step.title}</div>
    </div>
  );
}

function lastNonPending(steps: StepState[]): number {
  for (let i = steps.length - 1; i >= 0; i--) {
    if (steps[i].status !== "pending") return i;
  }
  return -1;
}
