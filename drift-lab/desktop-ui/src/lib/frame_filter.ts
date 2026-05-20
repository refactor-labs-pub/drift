// Copyright 2026 RefactorLabs
//
// Licensed under the Apache License, Version 2.0 (the "License").
//
// Tiny filter DSL for live-profile views. The same syntax fills in both
// the per-scan input on the Active Scan page and (later) the Rust-side
// server prefilter in `event_source_commands.rs`. Keeping ONE canonical
// parser here avoids the renderer / backend drifting on subtle parsing
// edge cases.
//
// Syntax (Linear / Slack / GitHub-search-style key:value):
//
//   file:/app/orders.py       file substring match
//   name:create               function-name substring match
//   file:/app/ name:create    AND of both
//   foo                       free-text — matches name OR file
//   !file:/site-packages/     leading `!` negates (drop matches)
//
// Multiple positive terms AND together; multiple negative terms also AND
// (every negative term must NOT match). A row is kept iff every positive
// term matches AND no negative term matches.
//
// This is a deliberately tiny grammar — no quoting, no regex, no
// boolean OR. The complexity budget for the input is "user types a few
// tokens and gets what they want"; richer queries can land later if
// users actually ask.
//
// ## Why substring instead of glob or regex
// Substring is what users type by default. `/app/orders.py` already
// matches `/app/orders.py` and `app/orders.py` and `/app/orders.py:23`
// without any escaping rules. Glob (`*.py`) and regex (`\\.py$`) both
// require the user to think about edge cases of the matching language
// — substring doesn't.
//
// ## What "name" and "file" map to in our data
// The aggregator's `EventLogFunctionStat` has `qualname` and `file`
// fields. The qualname is the rich form (e.g. `OrderService.create`);
// we match `name:` against `qualname` so users can write either
// `name:create` (matches any qualname containing "create") or
// `name:OrderService.create` (matches the full thing). `file:` always
// matches against the file path.

/** One parsed term. `field=null` means free text — match either name or file. */
export interface FrameFilterTerm {
  field: "name" | "file" | null;
  value: string;
  negate: boolean;
}

/** Parsed filter expression. Empty arrays = filter is a no-op. */
export interface FrameFilter {
  positive: FrameFilterTerm[];
  negative: FrameFilterTerm[];
  /** True if `positive.length + negative.length === 0`. Cheap pre-check
   *  so callers can skip the per-row match cost when nothing's set. */
  empty: boolean;
}

/** The subset of an `EventLogFunctionStat` the filter looks at. Kept
 *  narrow so callers don't have to round-trip through the full type.
 *  `file` is nullable to match `EventLogFunctionStat.file: string | null`
 *  — frames synthesized from tracer-only events (no sampler stack)
 *  don't have a file path. We treat null as the empty string for
 *  matching, so `file:foo` never matches and only `name:` / free-text
 *  terms can succeed against them. */
export interface FrameLike {
  qualname: string;
  file: string | null;
}

const EMPTY_FILTER: FrameFilter = {
  positive: [],
  negative: [],
  empty: true,
};

/** Parse a user-typed filter string into a `FrameFilter`. Whitespace
 *  separates terms; empty / all-whitespace input → no-op filter. */
export function parseFrameFilter(input: string): FrameFilter {
  const trimmed = input.trim();
  if (!trimmed) return EMPTY_FILTER;

  const positive: FrameFilterTerm[] = [];
  const negative: FrameFilterTerm[] = [];

  // Split on whitespace. Note we don't try to support quoted strings —
  // see module docstring for the rationale (file paths don't have
  // spaces in any realistic Python codebase).
  for (const raw of trimmed.split(/\s+/)) {
    let token = raw;
    let negate = false;
    if (token.startsWith("!")) {
      negate = true;
      token = token.slice(1);
      if (!token) continue;
    }
    let field: FrameFilterTerm["field"] = null;
    let value = token;
    const colon = token.indexOf(":");
    if (colon > 0) {
      const head = token.slice(0, colon);
      if (head === "name" || head === "file") {
        field = head;
        value = token.slice(colon + 1);
      }
    }
    if (!value) continue;
    const term: FrameFilterTerm = { field, value, negate };
    if (negate) negative.push(term);
    else positive.push(term);
  }

  return {
    positive,
    negative,
    empty: positive.length === 0 && negative.length === 0,
  };
}

/** Test whether one term matches a (qualname, file) pair. Substring,
 *  case-sensitive. Free-text (`field=null`) matches if EITHER qualname
 *  or file contains the value. */
function matchTerm(term: FrameFilterTerm, frame: FrameLike): boolean {
  const v = term.value;
  const file = frame.file ?? "";
  if (term.field === "name") return frame.qualname.includes(v);
  if (term.field === "file") return file.includes(v);
  // Free text — OR across the two columns the user most likely cares
  // about. Matching against just one would surprise users who typed a
  // file path.
  return frame.qualname.includes(v) || file.includes(v);
}

/** Returns `true` if the frame passes the filter. A frame passes iff
 *  every positive term matches AND no negative term matches. Empty
 *  filter passes everything. */
export function matchFrameFilter(
  filter: FrameFilter,
  frame: FrameLike,
): boolean {
  if (filter.empty) return true;
  for (const t of filter.positive) {
    if (!matchTerm(t, frame)) return false;
  }
  for (const t of filter.negative) {
    if (matchTerm(t, frame)) return false;
  }
  return true;
}
