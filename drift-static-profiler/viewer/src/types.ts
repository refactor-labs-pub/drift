export type SymbolKind = 'Function' | 'Method' | 'Class';

export type Category = 'db' | 'network' | 'io' | 'cache' | 'queue' | 'log' | 'compute';

export interface ExternalCall {
  name: string;
  receiver?: string | null;
  category: Category;
  tier?: 'imported_module' | 'receiver_pattern' | 'method_signature';
  evidence?: string;
  line: number;
  in_loop?: boolean;
  in_await?: boolean;
}

export interface CallerRef {
  id: string;
  name: string;
  file: string;
  line: number;
  parent_class: string | null;
}

export interface CallTreeNode {
  id: string;
  name: string;
  kind: SymbolKind;
  file: string;
  line: number;
  depth: number;
  parent_class: string | null;
  children: CallTreeNode[];
  truncated_reason: string | null;

  callers: CallerRef[];
  callers_count: number;
  callees_count: number;
  subtree_size: number;

  category_self: Category | null;
  categories_reached: Record<string, number>;
  external_calls: ExternalCall[];

  // Phase A — per-symbol code quality
  complexity: number;
  loc: number;
  nesting_depth: number;
  parameter_count: number;
  is_async: boolean;

  // Phase B — graph-derived
  call_site_count: number;
  is_recursive: boolean;
  pagerank: number;

  // Phase C — tree percentages
  percent_total: number;
  percent_parent: number;

  // Phase D — risk flags
  n_plus_one_risk: boolean;
  blocking_in_async: boolean;
}

export interface TopSymbol {
  name: string;
  file: string;
  line: number;
  parent_class: string | null;
  count: number;
}

export interface HotPath {
  frames: string[];
  depth: number;
  terminal_category: string;
}

export interface RankedByScore {
  name: string;
  file: string;
  line: number;
  parent_class: string | null;
  score: number;
}

export interface Summary {
  languages: string[];
  files: number;
  symbols: number;
  edges: number;
  categories: Record<string, number>;
  top_callers: TopSymbol[];
  top_callees: TopSymbol[];
  hot_paths: HotPath[];
  dead_code: TopSymbol[];
  pagerank_top: RankedByScore[];
  recursive_symbols: TopSymbol[];
}

export interface Report {
  summary: Summary;
  entries: CallTreeNode[];
}

export interface FixtureSpec {
  key: string;
  label: string;
  json: string;
  description: string;
}

export interface FlameNode {
  name: string;
  value: number;
  tooltip: string;
  backgroundColor: string;
  color: string;
  id: string;
  source: CallTreeNode;
  children?: FlameNode[];
}

export const CATEGORY_COLORS: Record<Category, string> = {
  db:       '#e26d6d',
  network:  '#7e6ff0',
  io:       '#e0a458',
  cache:    '#48a999',
  queue:    '#d09bd1',
  log:      '#7e8189',
  compute:  '#5b8def',
};
