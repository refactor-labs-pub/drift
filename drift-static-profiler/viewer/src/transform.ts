import { CATEGORY_COLORS } from './types';
import type { CallTreeNode, FlameNode, SymbolKind } from './types';

const KIND_COLORS: Record<SymbolKind, { bg: string; fg: string }> = {
  Function: { bg: '#5b8def', fg: '#0a0a14' },
  Method:   { bg: '#48a999', fg: '#0a0a14' },
  Class:    { bg: '#e0a458', fg: '#0a0a14' },
};

const TRUNCATED = { bg: '#56585b', fg: '#1e1f22' };

export type FlameMode = 'kind' | 'category' | 'complexity' | 'smells';

function complexityToColor(c: number): string {
  // McCabe ranges: 1-4 simple, 5-9 moderate, 10-14 complex, 15+ untestable
  if (c >= 15) return '#b53a3a';
  if (c >= 10) return '#e26d6d';
  if (c >= 5)  return '#e0a458';
  if (c >= 3)  return '#5b8def';
  return '#48a999';
}

export function subtreeWeight(node: CallTreeNode): number {
  // Use the analyzer-supplied count if present, else compute.
  if (typeof node.subtree_size === 'number' && node.subtree_size > 0) {
    return node.subtree_size;
  }
  let count = 1;
  for (const c of node.children) count += subtreeWeight(c);
  return count;
}

export function toFlame(node: CallTreeNode, mode: FlameMode = 'kind'): FlameNode {
  const value = subtreeWeight(node);
  let bg: string;
  let fg: string;
  if (node.truncated_reason) {
    bg = TRUNCATED.bg;
    fg = TRUNCATED.fg;
  } else if (mode === 'category' && node.category_self) {
    bg = CATEGORY_COLORS[node.category_self];
    fg = '#0a0a14';
  } else if (mode === 'category') {
    const reachesDb = (node.categories_reached?.db ?? 0) > 0;
    bg = reachesDb ? '#a64545' : KIND_COLORS[node.kind].bg;
    fg = KIND_COLORS[node.kind].fg;
  } else if (mode === 'complexity') {
    bg = complexityToColor(node.complexity ?? 0);
    fg = '#0a0a14';
  } else if (mode === 'smells') {
    if (node.n_plus_one_risk || node.blocking_in_async) {
      bg = '#e26d6d';
      fg = '#0a0a14';
    } else if (node.is_recursive) {
      bg = '#d09bd1';
      fg = '#0a0a14';
    } else {
      // dim non-smelly nodes
      bg = '#3a3c40';
      fg = '#6e717a';
    }
  } else {
    bg = KIND_COLORS[node.kind].bg;
    fg = KIND_COLORS[node.kind].fg;
  }

  const parent = node.parent_class ? `${node.parent_class}.` : '';
  const truncTag = node.truncated_reason ? ` [${node.truncated_reason}]` : '';
  const catTag = node.category_self ? ` [${node.category_self}]` : '';
  const smellTag = node.n_plus_one_risk ? ' ⚠N+1' : node.blocking_in_async ? ' ⚠BLK' : '';
  return {
    name: `${parent}${node.name}${catTag}${smellTag}${truncTag}`,
    value,
    tooltip: `${parent}${node.name}\n${node.file}:${node.line}\nkind: ${node.kind}\ncomplexity: ${node.complexity ?? 0}\nsubtree: ${value}\npagerank: ${(node.pagerank ?? 0).toFixed(4)}`,
    backgroundColor: bg,
    color: fg,
    id: node.id,
    source: node,
    children: node.children.length ? node.children.map(c => toFlame(c, mode)) : undefined,
  };
}
