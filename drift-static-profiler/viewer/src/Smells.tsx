import { TIPS } from './tooltips';
import { Help } from './Help';
import type { CallTreeNode } from './types';

interface Props {
  root: CallTreeNode | null;
  onSelect: (node: CallTreeNode) => void;
}

interface Smell {
  node: CallTreeNode;
  kinds: ('n_plus_one_risk' | 'blocking_in_async' | 'is_recursive')[];
}

function collect(node: CallTreeNode, out: Smell[]) {
  const kinds: Smell['kinds'] = [];
  if (node.n_plus_one_risk) kinds.push('n_plus_one_risk');
  if (node.blocking_in_async) kinds.push('blocking_in_async');
  if (node.is_recursive) kinds.push('is_recursive');
  if (kinds.length) out.push({ node, kinds });
  for (const c of node.children) collect(c, out);
}

const KIND_META = {
  n_plus_one_risk: { label: 'N+1 RISK', color: '#e26d6d', detail: TIPS.smell_n_plus_one },
  blocking_in_async: { label: 'BLOCKING IN ASYNC', color: '#ff7e7e', detail: TIPS.smell_blocking },
  is_recursive: { label: 'RECURSIVE', color: '#d09bd1', detail: TIPS.smell_recursive },
};

export function Smells({ root, onSelect }: Props) {
  if (!root) return null;
  const items: Smell[] = [];
  collect(root, items);

  if (items.length === 0) {
    return (
      <div style={emptyStyle}>
        <div style={{ marginBottom: 6 }}>✓ no smells detected in this entry's subtree</div>
        <div style={{ color: '#7e8189', fontSize: 11 }}>
          (analyzer looked for: N+1 queries, blocking I/O in async, mutual recursion)
        </div>
      </div>
    );
  }

  return (
    <div style={containerStyle}>
      <div style={summaryRowStyle}>
        Found <strong>{items.length}</strong> symbol(s) with smells. Click a row to inspect.
        <Help text="Each row is one antipattern detected in the entry's subtree. Hover the smell badge for the definition + how to fix it." />
      </div>
      <table style={tableStyle}>
        <thead>
          <tr style={theadStyle}>
            <th style={thStyle} title="The antipattern detected. Hover the badge to see the definition.">smell</th>
            <th style={thStyle} title="The function/method where the smell occurs.">symbol</th>
            <th style={thStyle} title="file:line of the symbol definition.">location</th>
            <th style={thStyle} title="Why we flagged it — receiver + method names, in-loop/awaited flags.">evidence</th>
          </tr>
        </thead>
        <tbody>
          {items.flatMap((s) =>
            s.kinds.map((kind) => (
              <tr
                key={`${s.node.id}:${kind}`}
                style={trStyle}
                onClick={() => onSelect(s.node)}
              >
                <td style={tdStyle}>
                  <span
                    style={{ ...badgeStyle, background: KIND_META[kind].color, cursor: 'help' }}
                    title={KIND_META[kind].detail}
                  >
                    {KIND_META[kind].label}
                  </span>
                </td>
                <td style={tdStyle}>
                  <code style={codeStyle}>
                    {s.node.parent_class ? `${s.node.parent_class}.` : ''}
                    {s.node.name}
                  </code>
                </td>
                <td style={tdLocStyle}>
                  {s.node.file}:{s.node.line}
                </td>
                <td style={tdEvidenceStyle}>
                  {evidenceFor(kind, s.node)}
                </td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}

function evidenceFor(kind: Smell['kinds'][number], node: CallTreeNode): string {
  switch (kind) {
    case 'n_plus_one_risk': {
      const inLoopCalls = (node.external_calls || []).filter((e) => e.in_loop);
      if (inLoopCalls.length === 0) return 'category-reaching call inside a loop';
      return `${inLoopCalls.length} in-loop call(s): ${inLoopCalls.map((c) => `${c.receiver ? c.receiver + '.' : ''}${c.name}() @ :${c.line}`).join(', ')}`;
    }
    case 'blocking_in_async': {
      const blockers = (node.external_calls || []).filter(
        (e) => !e.in_await && (e.category === 'db' || e.category === 'network' || e.category === 'io'),
      );
      if (blockers.length === 0) return 'sync I/O call in async body without await';
      return `${blockers.length} non-awaited I/O call(s): ${blockers.map((c) => `${c.receiver ? c.receiver + '.' : ''}${c.name}() [${c.category}] @ :${c.line}`).join(', ')}`;
    }
    case 'is_recursive':
      return `symbol participates in a cycle (SCC size > 1)`;
  }
}

const containerStyle: React.CSSProperties = {
  height: '100%',
  overflow: 'auto',
  padding: 12,
  fontSize: 12,
  background: '#1e1f22',
};
const summaryRowStyle: React.CSSProperties = {
  marginBottom: 12, color: '#9ca0a8', fontSize: 12,
};
const tableStyle: React.CSSProperties = {
  borderCollapse: 'collapse',
  width: '100%',
  fontFamily: 'ui-monospace, monospace',
};
const theadStyle: React.CSSProperties = {
  background: '#26282c',
  color: '#9ca0a8',
  textTransform: 'uppercase',
  fontSize: 10,
  letterSpacing: 0.4,
};
const thStyle: React.CSSProperties = {
  textAlign: 'left',
  padding: '6px 10px',
  borderBottom: '1px solid #3f4147',
  fontWeight: 700,
};
const trStyle: React.CSSProperties = {
  borderBottom: '1px solid #2f3136',
  cursor: 'pointer',
};
const tdStyle: React.CSSProperties = { padding: '6px 10px', verticalAlign: 'top' };
const tdLocStyle: React.CSSProperties = { ...tdStyle, color: '#7e8189', fontSize: 11, whiteSpace: 'nowrap' };
const tdEvidenceStyle: React.CSSProperties = { ...tdStyle, color: '#d7d9dc', fontSize: 11 };
const codeStyle: React.CSSProperties = { background: '#26282c', padding: '2px 5px', borderRadius: 3, color: '#d7d9dc' };
const badgeStyle: React.CSSProperties = {
  display: 'inline-block',
  padding: '2px 7px',
  borderRadius: 3,
  color: '#0a0a14',
  fontSize: 10,
  fontWeight: 700,
  whiteSpace: 'nowrap',
};
const emptyStyle: React.CSSProperties = {
  padding: 20, color: '#d7d9dc', fontSize: 13,
};
