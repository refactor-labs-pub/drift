import { useMemo } from 'react';
// @ts-expect-error - react-flame-graph ships no types
import { FlameGraph } from 'react-flame-graph';
import { toFlame } from './transform';
import type { FlameMode } from './transform';
import type { CallTreeNode, FlameNode } from './types';

interface Props {
  root: CallTreeNode;
  search: string;
  mode: FlameMode;
  categoryFilter?: string | null;
  onSelect: (node: CallTreeNode) => void;
  height: number;
  width: number;
}

const DIM = '#3a3c40';
const DIM_FG = '#6e717a';

function applyFilters(node: FlameNode, q: string, cat: string | null): FlameNode {
  const nameMatch = !q || node.name.toLowerCase().includes(q.toLowerCase());
  const src = node.source;
  const catMatch =
    !cat ||
    src.category_self === cat ||
    (src.categories_reached?.[cat] ?? 0) > 0;
  const match = nameMatch && catMatch;
  return {
    ...node,
    backgroundColor: match ? node.backgroundColor : DIM,
    color: match ? node.color : DIM_FG,
    children: node.children?.map(c => applyFilters(c, q, cat)),
  };
}

export function FlameView({ root, search, mode, categoryFilter, onSelect, height, width }: Props) {
  const data = useMemo(
    () => applyFilters(toFlame(root, mode), search, categoryFilter ?? null),
    [root, mode, search, categoryFilter],
  );
  return (
    <FlameGraph
      data={data}
      height={height}
      width={width}
      onChange={(node: any) => {
        // react-flame-graph wraps our FlameNode: node.source = FlameNode, node.source.source = CallTreeNode
        const tree = node?.source?.source as CallTreeNode | undefined;
        if (tree) onSelect(tree);
      }}
    />
  );
}
