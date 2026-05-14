/**
 * Pure, partial-tolerant parser for the suggester's output format.
 *
 * The LLM emits, strictly:
 *   problem_description_reasoning:
 *   <2–4 sentences about WHAT the problem is and WHY it matters>
 *
 *   solution_description_reasoning:
 *   <2–4 sentences about WHY the proposed change resolves it>
 *
 *   Why: <one-sentence terse summary>
 *
 *   ```diff
 *   @@ -34,7 +34,9 @@
 *    fn get_user(id: u64) -> User {
 *   -    let user = db.fetch(id);
 *   +    USER_CACHE.lock().get_or_insert(id)
 *    }
 *   ```
 *
 * The body grows token-by-token as the model streams. This parser runs on
 * *every accumulated body*, so any prefix of the contract must parse
 * cleanly:
 *
 *   1. `problem_description_reasoning:` (header only)           → empty problem reasoning
 *   2. `problem_description_reasoning:\n<text>` (no solution yet) → problem reasoning streaming
 *   3. ...up through `Why: ...` + open `\`\`\`diff` fence
 *   4. ...up through closed ``` fence (full body)
 *
 * The renderer uses {@link ParsedSuggestion.inDiff} and
 * {@link ParsedSuggestion.diffComplete} to decide where to put the
 * streaming caret (in the prose vs at the bottom of the diff).
 *
 * Robustness:
 *   - Missing reasoning sections collapse to empty strings — the renderer
 *     just doesn't render that panel.
 *   - If the model omits the diff entirely, the whole tail becomes
 *     `rationale` and the UI falls back to the prose render.
 *   - If the model picks a different fence language (e.g. ```rust), we
 *     deliberately *don't* match it — the prose fallback renders it raw
 *     instead of pretending it's a diff and miscoloring everything.
 */

export type DiffLineKind = "context" | "remove" | "add" | "hunk" | "meta";

export interface DiffLine {
  kind: DiffLineKind;
  /** Line text *with the prefix character stripped* — for diff lines a
   *  leading `+`/`-`/` ` is removed so the renderer can place the marker
   *  in its own gutter column. Hunk headers (`@@ ... @@`) and meta lines
   *  (`--- a/x.rs`, `+++ b/x.rs`) keep their full text. */
  text: string;
}

export interface ParsedSuggestion {
  /** Body of the `problem_description_reasoning:` section — what the
   *  problem is and why it matters. Empty when the model hasn't emitted
   *  that header yet, or when the section is missing. */
  problemReasoning: string;
  /** Body of the `solution_description_reasoning:` section — why the
   *  proposed change resolves the problem. */
  solutionReasoning: string;
  /** Everything between the (optional) `Why:` line and the ```diff fence.
   *  In the strict format this is the `Why: <one sentence>` summary; in
   *  degraded outputs it absorbs whatever prose the model wrote ahead of
   *  the diff. Trimmed. */
  rationale: string;
  /** Parsed diff lines, in order. Empty when no diff fence has streamed
   *  in yet. */
  diffLines: DiffLine[];
  /** True once we've seen the opening ```diff fence. */
  inDiff: boolean;
  /** True once we've seen the closing ``` fence after the diff content. */
  diffComplete: boolean;
}

// Allow optional whitespace + `diff` after the opening triple-backtick.
// Strict on the `diff` tag — we WON'T match ```rust or ```ts because those
// shouldn't be parsed as diffs (they'd render every line as "context" and
// silently mislead the user).
const DIFF_FENCE_OPEN = /```\s*diff\s*\r?\n/;
const FENCE_CLOSE_AT_LINE_START = /\r?\n```/;

// Section headers are case-insensitive at start-of-line. We allow optional
// trailing whitespace after the colon so a model that emits
// `problem_description_reasoning : ` still parses.
const PROBLEM_HEADER = /(^|\n)\s*problem_description_reasoning\s*:\s*/i;
const SOLUTION_HEADER = /(^|\n)\s*solution_description_reasoning\s*:\s*/i;

export function parseSuggestion(body: string): ParsedSuggestion {
  // Step 1: split off the diff fence so the prose chunk above it is
  // self-contained. Everything between the last diff fence and the start
  // of the body is what we slice into named sections below.
  const openMatch = body.match(DIFF_FENCE_OPEN);
  let prose: string;
  let diffLines: DiffLine[] = [];
  let inDiff = false;
  let diffComplete = false;

  if (openMatch && openMatch.index !== undefined) {
    prose = body.slice(0, openMatch.index);
    const afterFence = body.slice(openMatch.index + openMatch[0].length);
    const closeMatch = afterFence.match(FENCE_CLOSE_AT_LINE_START);
    const diffContent =
      closeMatch && closeMatch.index !== undefined
        ? afterFence.slice(0, closeMatch.index)
        : afterFence;
    diffLines = classifyDiff(diffContent);
    inDiff = true;
    diffComplete = closeMatch !== null;
  } else {
    prose = body;
  }

  // Step 2: locate the two reasoning headers in the prose. Anything before
  // the first header is dropped (the model shouldn't emit a preamble, but
  // if it does we don't want to muddle the `Why:` summary with it).
  const problemMatch = prose.match(PROBLEM_HEADER);
  const solutionMatch = prose.match(SOLUTION_HEADER);

  let problemReasoning = "";
  let solutionReasoning = "";
  let tail = prose;

  if (problemMatch && problemMatch.index !== undefined) {
    const start = problemMatch.index + problemMatch[0].length;
    const end = solutionMatch && solutionMatch.index !== undefined
      ? solutionMatch.index
      : prose.length;
    problemReasoning = prose.slice(start, end).trim();
    tail = prose.slice(end);
  }
  if (solutionMatch && solutionMatch.index !== undefined) {
    const start = solutionMatch.index + solutionMatch[0].length;
    // The `Why:` line, if present, terminates the solution-reasoning
    // section. Otherwise it runs to the end of the prose block.
    const whyIdx = findWhyIndex(prose, start);
    const end = whyIdx >= 0 ? whyIdx : prose.length;
    solutionReasoning = prose.slice(start, end).trim();
    tail = prose.slice(end);
  }

  // Step 3: whatever is left over is the `Why:` / rationale chunk.
  const rationale = stripWhyPrefix(tail).trim();

  return {
    problemReasoning,
    solutionReasoning,
    rationale,
    diffLines,
    inDiff,
    diffComplete,
  };
}

/** Locate `Why: ` as a line-start anchor at or after `from`. Returns -1 if
 *  no such anchor exists — keeps `prose.indexOf` semantics. */
function findWhyIndex(s: string, from: number): number {
  const re = /(^|\n)\s*Why\s*:/i;
  const slice = s.slice(from);
  const m = slice.match(re);
  if (!m || m.index === undefined) return -1;
  // m.index is into `slice`; m[1] is the leading newline (if any). We want
  // the position of the `Why` token, not the newline before it.
  const newlineLen = m[1] ? m[1].length : 0;
  return from + m.index + newlineLen;
}

/** Strip a leading `Why:` prefix so the rendered rationale doesn't include
 *  the literal token (the UI labels it visually instead). */
function stripWhyPrefix(s: string): string {
  return s.replace(/^\s*Why\s*:\s*/i, "");
}

function classifyDiff(content: string): DiffLine[] {
  if (content.length === 0) return [];
  // Drop a trailing empty line that comes from a final newline before the
  // closing fence — it would render as a blank "context" row otherwise.
  const lines = content.split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.map(classify);
}

function classify(line: string): DiffLine {
  if (line.startsWith("@@")) return { kind: "hunk", text: line };
  if (line.startsWith("+++") || line.startsWith("---")) {
    return { kind: "meta", text: line };
  }
  if (line.startsWith("+")) return { kind: "add", text: line.slice(1) };
  if (line.startsWith("-")) return { kind: "remove", text: line.slice(1) };
  // Context: most diffs prefix with a single space. Strip exactly one if
  // present; some models drop it on whitespace-only lines, so we keep
  // those untouched.
  if (line.startsWith(" ")) return { kind: "context", text: line.slice(1) };
  return { kind: "context", text: line };
}
