import { useMemo, useState } from 'react';
import { CATEGORY_COLORS } from './types';
import { Help } from './Help';
import { TIPS } from './tooltips';
import type { CallTreeNode, Category } from './types';

interface Props {
  roots: CallTreeNode[];
  activeRootId: string | null;
  onSelect: (id: string) => void;
}

type SortKey = 'reach' | 'complexity' | 'pagerank' | 'name' | 'smells';
type SortDir = 'asc' | 'desc';

// Mirrors the static-analysis "root view" pattern from Chrome DevTools'
// Top-Down (root activities), pprof's `top -cum`, Speedscope's Sandwich,
// and IntelliJ's Method List: a sortable table of entry-point symbols
// ranked by cumulative reach. Clicking a row drills the rest of the
// viewer into that root's flame graph + call tree.
export function RootsView({ roots, activeRootId, onSelect }: Props) {
  const [sortKey, setSortKey] = useState<SortKey>('reach');
  const [sortDir, setSortDir] = useState<SortDir>('desc');

  const sorted = useMemo(() => {
    const rows = roots.map(r => ({
      root: r,
      smells: countSmells(r),
    }));
    rows.sort((a, b) => {
      const av = pluck(a, sortKey);
      const bv = pluck(b, sortKey);
      if (typeof av === 'string' && typeof bv === 'string') {
        return sortDir === 'asc' ? av.localeCompare(bv) : bv.localeCompare(av);
      }
      const an = Number(av), bn = Number(bv);
      return sortDir === 'asc' ? an - bn : bn - an;
    });
    return rows;
  }, [roots, sortKey, sortDir]);

  if (roots.length === 0) {
    return (
      <div style={emptyStyle}>
        No roots in this report. Generate one with{' '}
        <code style={codeInlineStyle}>make scan-roots SCAN_PATH=…</code>.
      </div>
    );
  }

  const flip = (k: SortKey) => {
    if (k === sortKey) {
      setSortDir(d => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortKey(k);
      setSortDir(k === 'name' ? 'asc' : 'desc');
    }
  };

  const arrow = (k: SortKey) =>
    sortKey === k ? (sortDir === 'asc' ? '▲' : '▼') : '';

  return (
    <div style={containerStyle}>
      <div style={hintStyle}>
        Auto-discovered root entry points (symbols with no in-graph caller). Click a row to
        drill into its flame graph and call tree. Ranked like pprof's <code style={codeInlineStyle}>top -cum</code>{' '}
        and Chrome DevTools' Top-Down view. <Help text={TIPS.subtree_size} />
      </div>
      <div style={tableStyle}>
        <div style={headRowStyle}>
          <div style={{ ...cellStyle, width: 36 }}>#</div>
          <Th label="Name" sortable onClick={() => flip('name')} arrow={arrow('name')} flex={2} />
          <div style={{ ...cellStyle, flex: 1.4, color: '#7e8189' }}>File</div>
          <div style={{ ...cellStyle, width: 64, color: '#7e8189' }}>Kind</div>
          <Th label="Reach" sortable onClick={() => flip('reach')} arrow={arrow('reach')} width={64} title={TIPS.subtree_size} />
          <Th label="Cx" sortable onClick={() => flip('complexity')} arrow={arrow('complexity')} width={48} title={TIPS.complexity} />
          <div style={{ ...cellStyle, flex: 1.1, color: '#7e8189' }}>Categories</div>
          <Th label="Smells" sortable onClick={() => flip('smells')} arrow={arrow('smells')} width={62} title="N+1 + blocking-in-async + recursion counts in this root's subtree." />
          <Th label="PR" sortable onClick={() => flip('pagerank')} arrow={arrow('pagerank')} width={56} title={TIPS.pagerank} />
        </div>
        <div style={bodyStyle}>
          {sorted.map((row, i) => {
            const r = row.root;
            const isActive = r.id === activeRootId;
            const fullName = (r.parent_class ? `${r.parent_class}.` : '') + r.name;
            return (
              <div
                key={r.id}
                style={{ ...rowStyle, ...(isActive ? activeRowStyle : {}) }}
                onClick={() => onSelect(r.id)}
                title={`Click to focus the flame graph + call tree on ${fullName}`}
              >
                <div style={{ ...cellStyle, width: 36, color: '#7e8189' }}>{i + 1}</div>
                <div style={{ ...cellStyle, flex: 2, fontFamily: 'ui-monospace, monospace' }}>
                  <span style={nameStyle}>{fullName}</span>
                  {r.is_async && <span style={asyncBadgeStyle} title={TIPS.is_async}>async</span>}
                </div>
                <div style={{ ...cellStyle, flex: 1.4, color: '#9ca0a8', fontFamily: 'ui-monospace, monospace', fontSize: 11 }}>
                  {r.file}:{r.line}
                </div>
                <div style={{ ...cellStyle, width: 64 }}>
                  <span style={kindStyle(r.kind)}>{r.kind.toLowerCase()}</span>
                </div>
                <div style={{ ...cellStyle, width: 64, fontVariantNumeric: 'tabular-nums' }}>{r.subtree_size}</div>
                <div style={{ ...cellStyle, width: 48, fontVariantNumeric: 'tabular-nums', color: cxColor(r.complexity) }}>
                  {r.complexity}
                </div>
                <div style={{ ...cellStyle, flex: 1.1 }}>
                  <CategoryChips reached={r.categories_reached} />
                </div>
                <div style={{ ...cellStyle, width: 62, fontVariantNumeric: 'tabular-nums' }}>
                  <SmellBadge count={row.smells} />
                </div>
                <div style={{ ...cellStyle, width: 56, fontVariantNumeric: 'tabular-nums', color: '#9ca0a8' }}>
                  {r.pagerank ? r.pagerank.toFixed(3) : '—'}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function pluck(row: { root: CallTreeNode; smells: number }, k: SortKey): number | string {
  const r = row.root;
  switch (k) {
    case 'reach': return r.subtree_size;
    case 'complexity': return r.complexity;
    case 'pagerank': return r.pagerank ?? 0;
    case 'smells': return row.smells;
    case 'name': return (r.parent_class ? `${r.parent_class}.` : '') + r.name;
  }
}

function countSmells(root: CallTreeNode): number {
  let n = 0;
  const visit = (x: CallTreeNode) => {
    if (x.n_plus_one_risk) n++;
    if (x.blocking_in_async) n++;
    if (x.is_recursive) n++;
    for (const c of x.children) visit(c);
  };
  visit(root);
  return n;
}

function Th(props: {
  label: string;
  sortable: boolean;
  onClick: () => void;
  arrow: string;
  width?: number;
  flex?: number;
  title?: string;
}) {
  const { label, onClick, arrow, width, flex, title } = props;
  return (
    <div
      style={{
        ...cellStyle,
        ...(width !== undefined ? { width } : {}),
        ...(flex !== undefined ? { flex } : {}),
        cursor: 'pointer',
        userSelect: 'none',
        color: '#9ca0a8',
        fontWeight: 600,
      }}
      onClick={onClick}
      title={title}
    >
      {label} <span style={{ color: '#5b8def' }}>{arrow}</span>
    </div>
  );
}

function CategoryChips({ reached }: { reached: Record<string, number> }) {
  const entries = Object.entries(reached).sort((a, b) => b[1] - a[1]);
  if (entries.length === 0) return <span style={{ color: '#6e717a' }}>—</span>;
  return (
    <span style={{ display: 'inline-flex', gap: 4, flexWrap: 'wrap' }}>
      {entries.map(([cat, n]) => (
        <span
          key={cat}
          style={{
            background: CATEGORY_COLORS[cat as Category] ?? '#3a3f47',
            color: '#0a0a14',
            padding: '1px 5px',
            borderRadius: 3,
            fontSize: 10,
            fontWeight: 700,
            letterSpacing: 0.3,
          }}
          title={TIPS[`category_${cat}`] ?? cat}
        >
          {cat}:{n}
        </span>
      ))}
    </span>
  );
}

function SmellBadge({ count }: { count: number }) {
  if (count === 0) return <span style={{ color: '#6e717a' }}>0</span>;
  return (
    <span
      style={{
        background: count > 3 ? '#7a2f2f' : '#3a3326',
        color: count > 3 ? '#ffb0b0' : '#ffd569',
        padding: '1px 6px',
        borderRadius: 3,
        fontSize: 11,
        fontWeight: 700,
      }}
    >
      {count}
    </span>
  );
}

function cxColor(c: number): string {
  if (c >= 15) return '#ff7e7e';
  if (c >= 10) return '#e26d6d';
  if (c >= 5) return '#e0a458';
  return '#d7d9dc';
}

function kindStyle(kind: string): React.CSSProperties {
  const colors: Record<string, string> = {
    Function: '#5b8def',
    Method:   '#48a999',
    Class:    '#e0a458',
  };
  return {
    color: colors[kind] ?? '#9ca0a8',
    fontSize: 10,
    textTransform: 'uppercase',
    letterSpacing: 0.4,
    fontWeight: 700,
  };
}

const containerStyle: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  height: '100%',
  fontSize: 12,
};
const hintStyle: React.CSSProperties = {
  color: '#7e8189',
  fontSize: 11,
  padding: '6px 12px',
  borderBottom: '1px solid #2f3136',
  fontStyle: 'italic',
};
const tableStyle: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  flex: 1,
  overflow: 'hidden',
};
const headRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  padding: '4px 8px',
  background: '#26282c',
  borderBottom: '1px solid #3f4147',
  fontSize: 10,
  textTransform: 'uppercase',
  letterSpacing: 0.5,
  fontWeight: 700,
  flexShrink: 0,
};
const bodyStyle: React.CSSProperties = {
  flex: 1,
  overflowY: 'auto',
};
const rowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  padding: '5px 8px',
  borderBottom: '1px solid #2a2c30',
  cursor: 'pointer',
};
const activeRowStyle: React.CSSProperties = {
  background: '#2b3a5a',
};
const cellStyle: React.CSSProperties = {
  padding: '0 6px',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
};
const nameStyle: React.CSSProperties = {
  color: '#d7d9dc',
};
const asyncBadgeStyle: React.CSSProperties = {
  marginLeft: 6,
  padding: '0 5px',
  borderRadius: 3,
  background: '#3a2647',
  color: '#d09bd1',
  fontSize: 9,
  fontWeight: 700,
  letterSpacing: 0.4,
  textTransform: 'uppercase',
};
const emptyStyle: React.CSSProperties = {
  padding: 16,
  color: '#6e717a',
  fontStyle: 'italic',
  fontSize: 12,
};
const codeInlineStyle: React.CSSProperties = {
  background: '#2f3136',
  padding: '1px 5px',
  borderRadius: 2,
  fontFamily: 'ui-monospace, monospace',
  color: '#d7d9dc',
};
