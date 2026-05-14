/**
 * Minimal subset of the static-profiler viewer types — just what the desktop
 * summary cards need. Copied here verbatim from
 * `drift-static-profiler/viewer/src/types.ts` so this app can render saved
 * scan JSON without depending on the viewer package directly.
 *
 * Two reasons for the copy rather than a shared package:
 *  1. The desktop ships in its own bundle; pulling in the viewer's React
 *     entry tree would drag in `react-flame-graph` and friends we don't
 *     render here.
 *  2. The two surfaces are theme-isolated (viewer = dark, desktop = light),
 *     so having a thin local copy lets us evolve presentation without
 *     touching the source-of-truth viewer.
 *
 * The JSON shape itself remains the contract — keep the field names in
 * sync with `drift_static_profiler::report::{Summary, FindingTopRef,
 * ImmediateFix, RefactorCandidate}`.
 */

export type Severity = "low" | "medium" | "high";
export type Effort = "trivial" | "small" | "medium" | "large";

export type Category =
  | "db"
  | "network"
  | "io"
  | "cache"
  | "queue"
  | "log"
  | "compute";

export type FindingKind =
  | "n_plus_one"
  | "blocking_in_async"
  | "recursive"
  | "smelly_loop"
  | "noisy_log"
  | "outdated_package"
  | "memory_explosion"
  | "hot_zone"
  | "expensive_compute"
  | "missing_caching"
  | "log_amplification";

export interface FindingTopRef {
  node_id: string;
  kind: FindingKind;
  severity: Severity;
  line: number;
}

export interface ImmediateFix {
  node_id: string;
  name: string;
  file: string;
  line: number;
  parent_class?: string | null;
  kind: FindingKind;
  severity: Severity;
  effort: Effort;
  message: string;
}

export interface RefactorCandidate {
  node_id: string;
  name: string;
  file: string;
  line: number;
  parent_class?: string | null;
  findings_count: number;
  kinds: FindingKind[];
  worst_severity: Severity;
  max_effort: Effort;
  complexity: number;
  loc: number;
  percent_total: number;
  why: string;
}

export interface RankedByScore {
  name: string;
  file: string;
  line: number;
  parent_class: string | null;
  score: number;
}

export interface LanguageBreakdownEntry {
  language: string;
  bytes: number;
  percent: number;
  supported: boolean;
}

export interface Summary {
  languages: string[];
  files: number;
  symbols: number;
  edges: number;
  categories: Record<string, number>;
  pagerank_top?: RankedByScore[];
  findings_by_kind?: Record<string, number>;
  findings_top?: FindingTopRef[];
  immediate_fixes?: ImmediateFix[];
  refactor_candidates?: RefactorCandidate[];
  language_breakdown?: LanguageBreakdownEntry[];
  profiled_language?: string | null;
  profiled_language_percent?: number | null;
}

export interface Generator {
  tool: string;
  version: string;
  source_root?: string;
  captured_at?: string;
}

export interface CallTreeEntry {
  id: string;
  name: string;
  file: string;
  line: number;
  subtree_size: number;
  parent_class: string | null;
}

export interface Report {
  generator?: Generator;
  summary: Summary;
  entries: CallTreeEntry[];
}

/// Drift Lab is a light-theme app (vs. the viewer's dark surface). Same
/// semantic ramp — red/orange/gray for high/medium/low — but with values
/// that read on the warm off-white background defined in globals.css.
export const SEVERITY_COLORS: Record<Severity, string> = {
  high: "#e53935",
  medium: "#ff9558",
  low: "#9e9e9e",
};

export const CATEGORY_COLORS: Record<Category, string> = {
  db: "#e26d6d",
  network: "#7e6ff0",
  io: "#e0a458",
  cache: "#48a999",
  queue: "#d09bd1",
  log: "#9e9e9e",
  compute: "#5b8def",
};

export const FINDING_KIND_LABEL: Record<FindingKind, string> = {
  n_plus_one: "N+1",
  blocking_in_async: "BLOCKING IN ASYNC",
  recursive: "RECURSIVE",
  smelly_loop: "SMELLY LOOP",
  noisy_log: "NOISY LOG",
  outdated_package: "OUTDATED PKG",
  memory_explosion: "MEMORY EXPLOSION",
  hot_zone: "HOT ZONE",
  expensive_compute: "EXPENSIVE COMPUTE",
  missing_caching: "MISSING CACHING",
  log_amplification: "LOG AMPLIFICATION",
};
