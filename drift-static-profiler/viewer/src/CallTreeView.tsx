import { useState } from 'react';
import { CATEGORY_COLORS } from './types';
import { TIPS } from './tooltips';
import { Help } from './Help';
import type { CallTreeNode, SymbolKind } from './types';

interface Props {
  root: CallTreeNode;
  search: string;
  selectedId: string | null;
  onSelect: (node: CallTreeNode) => void;
}

export function CallTreeView({ root, search, selectedId, onSelect }: Props) {
  return (
    <div style={containerStyle}>
      <HeaderRow />
      <TreeRow node={root} depth={0} search={search} selectedId={selectedId} onSelect={onSelect} initiallyOpen />
    </div>
  );
}

function HeaderRow() {
  return (
    <div style={headerRowStyle}>
      <span style={{ flex: 1 }}>symbol</span>
      <span style={{ ...colStyle, width: 56 }}><Help text={TIPS.percent_total}>%total</Help></span>
      <span style={{ ...colStyle, width: 50 }}><Help text={TIPS.percent_parent}>%par</Help></span>
      <span style={{ ...colStyle, width: 36 }}><Help text={TIPS.complexity}>cplx</Help></span>
      <span style={{ ...colStyle, width: 32 }}><Help text={TIPS.loc}>loc</Help></span>
      <span style={{ ...colStyle, width: 56 }}>
        <Help text={`${TIPS.call_site_count}\n\nDisplayed as call_site_count / callers_count.`}>calls</Help>
      </span>
      <span style={{ ...colStyle, width: 50 }}><Help text={TIPS.pagerank}>rank</Help></span>
      <span style={{ ...colStyle, width: 80 }}>
        <Help text="Detected static smells (N+1, blocking I/O, recursion). Each is a known antipattern.">smells</Help>
      </span>
      <span style={{ ...colStyle, width: 180, textAlign: 'left', paddingLeft: 8 }}>file:line</span>
    </div>
  );
}

interface RowProps {
  node: CallTreeNode;
  depth: number;
  search: string;
  selectedId: string | null;
  onSelect: (node: CallTreeNode) => void;
  initiallyOpen?: boolean;
}

function TreeRow({ node, depth, search, selectedId, onSelect, initiallyOpen }: RowProps) {
  const [open, setOpen] = useState(initiallyOpen ?? depth < 3);
  const hasChildren = node.children.length > 0;
  const isMatch = !!search && node.name.toLowerCase().includes(search.toLowerCase());
  const isSelected = selectedId === node.id;
  const reachesDb = (node.categories_reached?.db ?? 0) > 0;
  return (
    <>
      <div
        style={{
          ...rowStyle,
          paddingLeft: 8 + depth * 14,
          background: isSelected
            ? '#2f436a'
            : node.n_plus_one_risk || node.blocking_in_async
            ? '#3a2630'
            : isMatch
            ? '#3a3326'
            : 'transparent',
        }}
        onClick={() => onSelect(node)}
      >
        <span
          style={caretStyle(hasChildren)}
          onClick={(e) => { e.stopPropagation(); setOpen(o => !o); }}
        >
          {hasChildren ? (open ? '▾' : '▸') : '·'}
        </span>
        <KindBadge kind={node.kind} />
        {node.is_async && <AsyncBadge />}
        {node.category_self && (
          <span
            title={`self: ${node.category_self}`}
            style={{
              display: 'inline-block',
              width: 8,
              height: 8,
              background: CATEGORY_COLORS[node.category_self],
              borderRadius: 2,
              marginRight: 2,
            }}
          />
        )}
        <span style={{ ...nameStyle(isMatch), flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {node.parent_class ? <span style={parentStyle}>{node.parent_class}.</span> : null}
          {node.name}
          {reachesDb && <span style={dbTagStyle} title="this subtree reaches a DB call">·db</span>}
        </span>
        <span style={{ ...colStyle, width: 56 }}>{fmtPct(node.percent_total)}</span>
        <span style={{ ...colStyle, width: 50 }}>{fmtPct(node.percent_parent)}</span>
        <span style={{ ...colStyle, width: 36, color: complexityColor(node.complexity) }}>
          {node.complexity ?? 0}
        </span>
        <span style={{ ...colStyle, width: 32 }}>{node.loc ?? 0}</span>
        <span style={{ ...colStyle, width: 56 }} title={`call_site_count=${node.call_site_count}; unique callers=${node.callers_count}`}>
          {node.call_site_count}/{node.callers_count}
        </span>
        <span style={{ ...colStyle, width: 50 }} title={`PageRank ${node.pagerank?.toFixed(4)}`}>
          {node.pagerank ? node.pagerank.toFixed(3) : '—'}
        </span>
        <span style={{ ...colStyle, width: 80 }}>
          <Smells node={node} />
        </span>
        <span style={{ ...colStyle, width: 180, textAlign: 'left', paddingLeft: 8, color: '#7e8189' }}>
          {node.file}:{node.line}
        </span>
        {node.truncated_reason && <span style={truncStyle}>[{node.truncated_reason}]</span>}
      </div>
      {open && hasChildren && node.children.map(c => (
        <TreeRow
          key={c.id + depth}
          node={c}
          depth={depth + 1}
          search={search}
          selectedId={selectedId}
          onSelect={onSelect}
        />
      ))}
    </>
  );
}

function Smells({ node }: { node: CallTreeNode }) {
  const items: { label: string; color: string; title: string }[] = [];
  if (node.n_plus_one_risk) items.push({ label: 'N+1', color: '#e26d6d', title: TIPS.smell_n_plus_one });
  if (node.blocking_in_async) items.push({ label: 'BLK', color: '#ff7e7e', title: TIPS.smell_blocking });
  if (node.is_recursive) items.push({ label: 'REC', color: '#d09bd1', title: TIPS.smell_recursive });
  if (items.length === 0) return <span style={{ color: '#3f4147' }}>—</span>;
  return (
    <span style={{ display: 'flex', gap: 2, justifyContent: 'flex-end' }}>
      {items.map((i) => (
        <span key={i.label} title={i.title} style={{
          fontSize: 9,
          fontWeight: 700,
          padding: '1px 4px',
          borderRadius: 2,
          background: i.color,
          color: '#0a0a14',
        }}>{i.label}</span>
      ))}
    </span>
  );
}

function KindBadge({ kind }: { kind: SymbolKind }) {
  const map: Record<SymbolKind, { label: string; bg: string; tip: string }> = {
    Function: { label: 'fn',  bg: '#5b8def', tip: TIPS.kind_function },
    Method:   { label: 'm',   bg: '#48a999', tip: TIPS.kind_method },
    Class:    { label: 'cls', bg: '#e0a458', tip: TIPS.kind_class },
  };
  const { label, bg, tip } = map[kind];
  return <span title={tip} style={{ ...badgeStyle, background: bg, cursor: 'help' }}>{label}</span>;
}

function AsyncBadge() {
  return <span title={TIPS.kind_async_marker} style={{ ...badgeStyle, background: '#7e6ff0', cursor: 'help' }}>α</span>;
}

function fmtPct(p: number | undefined): string {
  if (p == null) return '—';
  if (p >= 99.95) return '100%';
  return `${p.toFixed(1)}%`;
}

function complexityColor(c: number | undefined): string {
  if (!c) return '#7e8189';
  if (c >= 15) return '#e26d6d';
  if (c >= 10) return '#e0a458';
  if (c >= 5)  return '#d7d9dc';
  return '#7e8189';
}

const containerStyle: React.CSSProperties = {
  overflowY: 'auto',
  height: '100%',
  background: '#1e1f22',
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
  fontSize: 11,
};

const headerRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 6,
  padding: '4px 8px',
  background: '#26282c',
  borderBottom: '1px solid #3f4147',
  position: 'sticky',
  top: 0,
  fontWeight: 700,
  fontSize: 10,
  color: '#9ca0a8',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
  zIndex: 2,
};

const rowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 6,
  padding: '3px 8px',
  cursor: 'pointer',
  borderBottom: '1px solid #25272a',
  userSelect: 'none',
};

const colStyle: React.CSSProperties = {
  textAlign: 'right',
  color: '#9ca0a8',
  fontVariantNumeric: 'tabular-nums',
};

const caretStyle = (active: boolean): React.CSSProperties => ({
  width: 12, color: active ? '#9ca0a8' : '#3f4147', cursor: active ? 'pointer' : 'default',
});

const badgeStyle: React.CSSProperties = {
  display: 'inline-block',
  minWidth: 22,
  textAlign: 'center',
  padding: '0 4px',
  borderRadius: 3,
  color: '#0a0a14',
  fontWeight: 600,
  fontSize: 10,
};

const nameStyle = (highlight: boolean): React.CSSProperties => ({
  color: highlight ? '#ffd569' : '#d7d9dc',
  fontWeight: highlight ? 600 : 400,
});

const parentStyle: React.CSSProperties = { color: '#7e8189' };
const dbTagStyle: React.CSSProperties = {
  marginLeft: 6,
  fontSize: 9,
  color: '#e26d6d',
  fontFamily: 'ui-monospace, monospace',
};
const truncStyle: React.CSSProperties = { color: '#ff7e7e', fontSize: 11 };
