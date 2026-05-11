/**
 * Modal shown when the agent calls `ask_user` mid-run. The run is parked
 * server-side waiting on a oneshot; submitting the form unparks it and the
 * agent loop continues with the answer as the tool's content.
 *
 * The textarea is the entire interaction — the agent already explained *why*
 * it's asking (via the question text) and *what kind of answer* it expects.
 * We deliberately do NOT offer multiple-choice or cancel: the operator can
 * always answer "skip this", "I don't know", or close the modal via Esc to
 * cancel the run from the top bar if they need to.
 */

import { useEffect, useRef, useState } from "react";

import { answerBlockedQuestion, type BlockedQuestion } from "../lib/tauri";

interface Props {
  question: BlockedQuestion;
  /** Called after a successful submit so the parent can clear `blockedQuestion`. */
  onAnswered: () => void;
}

export default function BlockedModal({ question, onAnswered }: Props) {
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Focus the textarea on mount and whenever the question changes — the
  // user just got blocked, the answer field should be ready to type into.
  useEffect(() => {
    textareaRef.current?.focus();
  }, [question.id]);

  async function handleSubmit() {
    if (submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await answerBlockedQuestion(text.trim());
      onAnswered();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSubmitting(false);
    }
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    // Cmd/Ctrl+Enter submits — matches the convention from most chat boxes.
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      void handleSubmit();
    }
  }

  return (
    <div className="blocked-overlay" role="dialog" aria-modal="true">
      <div className="blocked-modal">
        <div className="blocked-modal-head">
          <span className="blocked-modal-flag">⚠ Agent is blocked</span>
          <span className="blocked-modal-hint">Cmd+Enter to submit</span>
        </div>
        <div className="blocked-modal-question">{question.question}</div>
        <textarea
          ref={textareaRef}
          className="blocked-modal-input"
          rows={4}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Type your answer…"
          disabled={submitting}
        />
        {error && <div className="blocked-modal-error">{error}</div>}
        <div className="blocked-modal-actions">
          <button
            type="button"
            className="primary-btn"
            onClick={handleSubmit}
            disabled={submitting}
          >
            {submitting ? "Sending…" : "Submit answer"}
          </button>
        </div>
      </div>
    </div>
  );
}
