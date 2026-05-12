import { TIPS } from './tooltips';
import { Help } from './Help';
import type { Summary } from './types';

type Jump = (lookup: { file?: string; line?: number; name?: string }) => void;

interface Props {
  summary: Summary | null;
  onJump?: Jump;
}

export function Statistics({ summary, onJump }: Props) {
  if (!summary) return null;
  const clickItem = (r: { name: string; file: string; line: number; parent_class: string | null }) =>
    onJump?.({ file: r.file, line: r.line, name: (r.parent_class ? `${r.parent_class}.` : '') + r.name });
  return (
    <div style={containerStyle}>
      <div style={gridStyle}>
        <Panel title={`top by PageRank · α=0.85`} tip={TIPS.pagerank_top}>
          {summary.pagerank_top.length === 0 ? (
            <Empty />
          ) : (
            <ol style={listStyle}>
              {summary.pagerank_top.map((r, i) => (
                <li key={i} style={liButtonStyle} onClick={() => clickItem(r)} title="jump to symbol">
                  <code style={codeStyle}>
                    {r.parent_class ? <span style={dimStyle}>{r.parent_class}.</span> : null}
                    {r.name}
                  </code>
                  <span style={scoreStyle}>{r.score.toFixed(4)}</span>
                  <span style={locStyle}>{r.file}:{r.line}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <Panel title={`most-called symbols (fan-in)`} tip={TIPS.top_callers}>
          {summary.top_callers.length === 0 ? (
            <Empty msg="no inbound edges (single-file project?)" />
          ) : (
            <ol style={listStyle}>
              {summary.top_callers.map((r, i) => (
                <li key={i} style={liButtonStyle} onClick={() => clickItem(r)} title="jump to symbol">
                  <code style={codeStyle}>
                    {r.parent_class ? <span style={dimStyle}>{r.parent_class}.</span> : null}
                    {r.name}
                  </code>
                  <span style={countStyle}>×{r.count}</span>
                  <span style={locStyle}>{r.file}:{r.line}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <Panel title={`high-fan-out symbols (callees)`} tip={TIPS.top_callees}>
          {summary.top_callees.length === 0 ? (
            <Empty />
          ) : (
            <ol style={listStyle}>
              {summary.top_callees.map((r, i) => (
                <li key={i} style={liButtonStyle} onClick={() => clickItem(r)} title="jump to symbol">
                  <code style={codeStyle}>
                    {r.parent_class ? <span style={dimStyle}>{r.parent_class}.</span> : null}
                    {r.name}
                  </code>
                  <span style={countStyle}>→{r.count}</span>
                  <span style={locStyle}>{r.file}:{r.line}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <Panel title={`dead code · 0 callers, not pinned`} tip={TIPS.dead_code}>
          {summary.dead_code.length === 0 ? (
            <Empty msg="no dead code detected" />
          ) : (
            <ol style={listStyle}>
              {summary.dead_code.map((r, i) => (
                <li key={i} style={liButtonStyle} onClick={() => clickItem(r)} title="jump to symbol">
                  <code style={codeStyle}>
                    {r.parent_class ? <span style={dimStyle}>{r.parent_class}.</span> : null}
                    {r.name}
                  </code>
                  <span style={locStyle}>{r.file}:{r.line}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <Panel title={`recursive symbols · SCC > 1`} tip={TIPS.recursive_symbols}>
          {summary.recursive_symbols.length === 0 ? (
            <Empty msg="no recursion cycles found" />
          ) : (
            <ol style={listStyle}>
              {summary.recursive_symbols.map((r, i) => (
                <li key={i} style={liButtonStyle} onClick={() => clickItem(r)} title="jump to symbol">
                  <code style={codeStyle}>
                    {r.parent_class ? <span style={dimStyle}>{r.parent_class}.</span> : null}
                    {r.name}
                  </code>
                  <span style={locStyle}>{r.file}:{r.line}</span>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <Panel title={`languages · files · edges`}>
          <div style={{ padding: 8 }}>
            <Row k="languages" v={summary.languages.join(', ')} />
            <Row k="files" v={String(summary.files)} />
            <Row k="symbols" v={String(summary.symbols)} />
            <Row k="edges (calls)" v={String(summary.edges)} />
            <Row
              k="categories"
              v={Object.entries(summary.categories)
                .filter(([, n]) => n > 0)
                .map(([c, n]) => `${c} ${n}`)
                .join('  ·  ') || 'none'}
            />
          </div>
        </Panel>
      </div>
    </div>
  );
}

function Panel({ title, children, tip }: { title: string; children: React.ReactNode; tip?: string }) {
  return (
    <div style={panelStyle}>
      <div style={panelHeaderStyle}>
        {title}
        {tip && <Help text={tip} />}
      </div>
      <div style={{ overflow: 'auto', maxHeight: 220 }}>{children}</div>
    </div>
  );
}

function Empty({ msg }: { msg?: string }) {
  return <div style={{ padding: 14, color: '#7e8189', fontSize: 11, fontStyle: 'italic' }}>{msg ?? '—'}</div>;
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div style={{ display: 'flex', padding: '3px 0', borderBottom: '1px solid #2f3136', fontSize: 11 }}>
      <span style={{ width: 100, color: '#7e8189' }}>{k}</span>
      <span style={{ color: '#d7d9dc' }}>{v}</span>
    </div>
  );
}

const containerStyle: React.CSSProperties = {
  height: '100%',
  overflow: 'auto',
  padding: 10,
  background: '#1e1f22',
};
const gridStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))',
  gap: 10,
};
const panelStyle: React.CSSProperties = {
  background: '#26282c',
  border: '1px solid #3f4147',
  borderRadius: 4,
  overflow: 'hidden',
};
const panelHeaderStyle: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 700,
  textTransform: 'uppercase',
  letterSpacing: 0.4,
  color: '#9ca0a8',
  padding: '6px 10px',
  background: '#1e1f22',
  borderBottom: '1px solid #3f4147',
};
const listStyle: React.CSSProperties = {
  margin: 0,
  padding: 0,
  listStyle: 'none',
  fontFamily: 'ui-monospace, monospace',
  fontSize: 11,
};
const liStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '4px 10px',
  borderBottom: '1px solid #2f3136',
};
const liButtonStyle: React.CSSProperties = {
  ...liStyle,
  cursor: 'pointer',
};
const codeStyle: React.CSSProperties = { color: '#d7d9dc', whiteSpace: 'nowrap' };
const dimStyle: React.CSSProperties = { color: '#7e8189' };
const scoreStyle: React.CSSProperties = { color: '#5b8def', minWidth: 60, textAlign: 'right' };
const countStyle: React.CSSProperties = { color: '#48a999', minWidth: 40, textAlign: 'right' };
const locStyle: React.CSSProperties = { marginLeft: 'auto', color: '#7e8189', fontSize: 10 };
