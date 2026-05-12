import { useMemo, useState } from 'react';
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
} from './types';
import { ResizeHandle, useResizableColumns, type ColumnDef } from './useResizableColumns';
import type {
  CallTreeNode,
  Category,
  Finding,
  FindingKind,
  Report,
  Severity,
} from './types';

interface Props {
  report: Report | null;
  /// Optional pre-applied filter — used by the Smells tab (step 13) to
  /// render this same component as a narrowed view.
  presetKinds?: FindingKind[];
  onJump?: (lookup: { id?: string; file?: string; line?: number; name?: string }) => void;
}

interface Row {
  node: CallTreeNode;
  finding: Finding;
}

const COLUMNS: ColumnDef[] = [
  { id: 'severity', defaultWidth: 90,  minWidth: 70 },
  { id: 'kind',     defaultWidth: 170, minWidth: 110 },
  { id: 'where',    defaultWidth: 320, minWidth: 160 },
  { id: 'message',  defaultWidth: 560, minWidth: 200 },
];

const SEV_RANK: Record<Severity, number> = { high: 2, medium: 1, low: 0 };

function collect(node: CallTreeNode, out: Row[]) {
  for (const f of node.findings ?? []) {
    out.push({ node, finding: f });
  }
  for (const c of node.children) collect(c, out);
}

export function Insights({ report, presetKinds, onJump }: Props) {
  const cols = useResizableColumns('drift.insights.cols', COLUMNS);
  const w = cols.widths;
  const [kindFilter, setKindFilter] = useState<FindingKind | 'all'>('all');
  const [sevFilter, setSevFilter] = useState<Severity | 'all'>('all');
  const [selected, setSelected] = useState<Row | null>(null);

  const allRows = useMemo<Row[]>(() => {
    if (!report) return [];
    const out: Row[] = [];
    for (const e of report.entries) collect(e, out);
    // Sort: severity DESC then by node line so order is deterministic.
    out.sort((a, b) =>
      SEV_RANK[b.finding.severity] - SEV_RANK[a.finding.severity]
      || a.node.file.localeCompare(b.node.file)
      || a.finding.line - b.finding.line
    );
    return out;
  }, [report]);

  // Visible kinds for the dropdown — restricted to the preset when present.
  const visibleKinds = useMemo(() => {
    const seen = new Set<FindingKind>();
    for (const r of allRows) {
      if (presetKinds && !presetKinds.includes(r.finding.kind)) continue;
      seen.add(r.finding.kind);
    }
    return Array.from(seen);
  }, [allRows, presetKinds]);

  const filtered = useMemo(() => {
    return allRows.filter((r) => {
      if (presetKinds && !presetKinds.includes(r.finding.kind)) return false;
      if (kindFilter !== 'all' && r.finding.kind !== kindFilter) return false;
      if (sevFilter !== 'all' && r.finding.severity !== sevFilter) return false;
      return true;
    });
  }, [allRows, presetKinds, kindFilter, sevFilter]);

  if (!report) return null;
  if (allRows.length === 0) {
    return (
      <div style={emptyStyle}>
        <div style={{ marginBottom: 6 }}>✓ no insights for this scan</div>
        <div style={{ color: '#7e8189', fontSize: 11 }}>
          Either the scan is clean, or the fixture pre-dates the insights feature
          (look for `schema_version` ≥ 1.0 with `findings_by_kind` on the summary).
        </div>
      </div>
    );
  }

  const headerCell = (id: string, label: string) => (
    <th style={{ ...thStyle, position: 'relative', width: w[id] }}>
      {label}
      <ResizeHandle onMouseDown={cols.startResize(id)} onReset={() => cols.resetColumn(id)} />
    </th>
  );

  return (
    <div style={containerStyle}>
      <div style={controlsStyle}>
        <span style={countStyle}>
          <strong style={{ color: '#d7d9dc' }}>{filtered.length}</strong>
          <span style={{ color: '#7e8189' }}> of {allRows.length} findings</span>
        </span>
        <span style={separatorStyle}>·</span>
        <label style={labelStyle}>Kind</label>
        <select
          value={kindFilter}
          onChange={(e) => setKindFilter(e.target.value as FindingKind | 'all')}
          style={selectStyle}
        >
          <option value="all">all kinds</option>
          {visibleKinds.map((k) => (
            <option key={k} value={k}>{FINDING_KIND_LABEL[k] ?? k}</option>
          ))}
        </select>
        <label style={labelStyle}>Severity</label>
        <select
          value={sevFilter}
          onChange={(e) => setSevFilter(e.target.value as Severity | 'all')}
          style={selectStyle}
        >
          <option value="all">all</option>
          <option value="high">high</option>
          <option value="medium">medium</option>
          <option value="low">low</option>
        </select>
        <span style={{ marginLeft: 'auto', color: '#5f626a', fontSize: 11 }}>
          click a row to inspect · jump button navigates to the call tree
        </span>
      </div>
      <table style={tableStyle}>
        <colgroup>
          <col style={{ width: w.severity }} />
          <col style={{ width: w.kind }} />
          <col style={{ width: w.where }} />
          <col style={{ width: w.message }} />
        </colgroup>
        <thead>
          <tr style={theadStyle}>
            {headerCell('severity', 'severity')}
            {headerCell('kind', 'kind')}
            {headerCell('where', 'where')}
            {headerCell('message', 'message')}
          </tr>
        </thead>
        <tbody>
          {filtered.map((r, i) => (
            <tr
              key={`${r.node.id}:${r.finding.kind}:${r.finding.line}:${i}`}
              style={{
                ...trStyle,
                background: selected === r ? '#2a2c30' : 'transparent',
              }}
              onClick={() => setSelected(r)}
            >
              <td style={tdStyle}>
                <span style={{ ...badgeStyle, background: SEVERITY_COLORS[r.finding.severity] }}>
                  {r.finding.severity}
                </span>
              </td>
              <td style={tdStyle}>
                <span style={{ ...kindBadgeStyle }}>
                  {FINDING_KIND_LABEL[r.finding.kind] ?? r.finding.kind}
                </span>
              </td>
              <td style={tdLocStyle}>
                <code style={codeStyle}>
                  {r.node.parent_class ? `${r.node.parent_class}.` : ''}{r.node.name}
                </code>
                <div style={{ fontSize: 10, color: '#7e8189' }}>{r.node.file}:{r.finding.line}</div>
              </td>
              <td style={tdMsgStyle}>
                {r.finding.message}
                <div style={{ marginTop: 4, fontSize: 10, color: '#7e8189' }}>
                  conf {r.finding.confidence.toFixed(2)}
                  {(r.finding.evidence?.length ?? 0) > 0 && (
                    <> · {r.finding.evidence!.length} evidence item(s)</>
                  )}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {selected && (
        <Detail row={selected} onJump={onJump} onClear={() => setSelected(null)} />
      )}
    </div>
  );
}

function Detail({
  row,
  onJump,
  onClear,
}: {
  row: Row;
  onJump?: Props['onJump'];
  onClear: () => void;
}) {
  const f = row.finding;
  return (
    <div style={detailStyle}>
      <div style={detailHeaderStyle}>
        <span style={{ ...badgeStyle, background: SEVERITY_COLORS[f.severity] }}>
          {f.severity}
        </span>
        <span style={{ ...kindBadgeStyle, marginLeft: 6 }}>
          {FINDING_KIND_LABEL[f.kind] ?? f.kind}
        </span>
        <span style={{ marginLeft: 8, color: '#d7d9dc', fontWeight: 600 }}>
          {row.node.parent_class ? `${row.node.parent_class}.` : ''}{row.node.name}
        </span>
        <span style={{ marginLeft: 8, color: '#7e8189', fontSize: 11 }}>
          {row.node.file}:{f.line} · confidence {f.confidence.toFixed(2)}
        </span>
        <span style={{ marginLeft: 'auto', display: 'inline-flex', gap: 6 }}>
          {onJump && (
            <button
              style={jumpBtnStyle}
              onClick={() => onJump({ id: row.node.id })}
            >
              Jump to node →
            </button>
          )}
          <button style={closeBtnStyle} onClick={onClear}>×</button>
        </span>
      </div>
      <div style={detailBodyStyle}>{f.message}</div>
      {(f.evidence?.length ?? 0) > 0 && (
        <div style={detailBodyStyle}>
          <div style={detailSubheadStyle}>Evidence</div>
          <ul style={evidenceListStyle}>
            {f.evidence!.map((e, j) => (
              <li key={j} style={evidenceItemStyle}>
                <code style={codeStyle}>{e.call}</code>
                <span style={{ color: '#7e8189' }}> @ :{e.line}</span>
                {e.category && (
                  <span
                    style={{
                      ...miniBadgeStyle,
                      background: CATEGORY_COLORS[e.category as Category],
                    }}
                  >
                    {e.category}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
      {f.remediation && (
        <div style={detailBodyStyle}>
          <div style={detailSubheadStyle}>Remediation</div>
          <div style={remediationStyle}>{f.remediation}</div>
        </div>
      )}
    </div>
  );
}

// ─── styles ─────────────────────────────────────────────────────────────

const containerStyle: React.CSSProperties = {
  height: '100%',
  overflow: 'auto',
  padding: 12,
  fontSize: 12,
  background: '#1e1f22',
};
const controlsStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  marginBottom: 10,
};
const countStyle: React.CSSProperties = { fontSize: 12 };
const separatorStyle: React.CSSProperties = { color: '#5f626a' };
const labelStyle: React.CSSProperties = {
  fontSize: 10,
  color: '#7e8189',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
};
const selectStyle: React.CSSProperties = {
  background: '#1e1f22',
  color: '#d7d9dc',
  border: '1px solid #3f4147',
  borderRadius: 3,
  padding: '3px 6px',
  fontSize: 11,
};
const tableStyle: React.CSSProperties = {
  borderCollapse: 'collapse',
  tableLayout: 'fixed',
  width: 'max-content',
  minWidth: '100%',
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
  boxSizing: 'border-box',
};
const trStyle: React.CSSProperties = {
  borderBottom: '1px solid #2f3136',
  cursor: 'pointer',
};
const tdStyle: React.CSSProperties = {
  padding: '6px 10px',
  verticalAlign: 'top',
  overflow: 'hidden',
  boxSizing: 'border-box',
};
const tdLocStyle: React.CSSProperties = { ...tdStyle };
const tdMsgStyle: React.CSSProperties = {
  ...tdStyle,
  color: '#d7d9dc',
  fontSize: 11,
  whiteSpace: 'normal',
  wordBreak: 'break-word',
};
const codeStyle: React.CSSProperties = {
  background: '#26282c', padding: '2px 5px', borderRadius: 3, color: '#d7d9dc',
};
const badgeStyle: React.CSSProperties = {
  display: 'inline-block',
  padding: '2px 7px',
  borderRadius: 3,
  color: '#0a0a14',
  fontSize: 10,
  fontWeight: 700,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
};
const kindBadgeStyle: React.CSSProperties = {
  display: 'inline-block',
  padding: '2px 7px',
  borderRadius: 3,
  background: '#3f4147',
  color: '#d7d9dc',
  fontSize: 10,
  fontWeight: 600,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
};
const miniBadgeStyle: React.CSSProperties = {
  ...badgeStyle,
  marginLeft: 6,
  fontSize: 9,
  padding: '1px 5px',
};
const detailStyle: React.CSSProperties = {
  marginTop: 14,
  border: '1px solid #3f4147',
  borderRadius: 4,
  background: '#26282c',
};
const detailHeaderStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 4,
  padding: '8px 10px',
  borderBottom: '1px solid #3f4147',
  background: '#1e1f22',
};
const detailBodyStyle: React.CSSProperties = {
  padding: '8px 12px',
  borderBottom: '1px solid #2f3136',
  color: '#d7d9dc',
  fontSize: 12,
};
const detailSubheadStyle: React.CSSProperties = {
  fontSize: 10,
  color: '#7e8189',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
  marginBottom: 4,
};
const evidenceListStyle: React.CSSProperties = {
  margin: 0,
  padding: 0,
  listStyle: 'none',
};
const evidenceItemStyle: React.CSSProperties = {
  padding: '3px 0',
  fontFamily: 'ui-monospace, monospace',
  fontSize: 11,
};
const remediationStyle: React.CSSProperties = {
  background: '#1e1f22',
  border: '1px solid #3f4147',
  borderRadius: 3,
  padding: 8,
  color: '#d7d9dc',
  fontSize: 12,
  lineHeight: 1.4,
};
const jumpBtnStyle: React.CSSProperties = {
  background: 'transparent',
  border: '1px solid #5b8def',
  color: '#5b8def',
  fontSize: 11,
  padding: '3px 8px',
  borderRadius: 3,
  cursor: 'pointer',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
  fontWeight: 600,
};
const closeBtnStyle: React.CSSProperties = {
  background: 'transparent',
  border: '1px solid #3f4147',
  color: '#9ca0a8',
  fontSize: 14,
  lineHeight: 1,
  padding: '2px 8px',
  borderRadius: 3,
  cursor: 'pointer',
};
const emptyStyle: React.CSSProperties = {
  padding: 20, color: '#d7d9dc', fontSize: 13,
};
