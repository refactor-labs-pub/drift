import { useEffect, useMemo, useRef, useState } from 'react';
import { FIXTURES } from './fixtures';
import { FlameView } from './FlameView';
import { CallTreeView } from './CallTreeView';
import { DetailsPane } from './DetailsPane';
import { SummaryBar } from './SummaryBar';
import { HotPaths } from './HotPaths';
import { Smells } from './Smells';
import { Statistics } from './Statistics';
import { subtreeWeight } from './transform';
import { TIPS } from './tooltips';
import { Help } from './Help';
import type { CallTreeNode, Report } from './types';

type FlameMode = 'kind' | 'category' | 'complexity' | 'smells';
type BottomTab = 'tree' | 'hot' | 'smells' | 'stats';

export function App() {
  const [fixtureKey, setFixtureKey] = useState(FIXTURES[0].key);
  const [report, setReport] = useState<Report | null>(null);
  const [activeRootId, setActiveRootId] = useState<string | null>(null);
  const [selected, setSelected] = useState<CallTreeNode | null>(null);
  const [search, setSearch] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [flameMode, setFlameMode] = useState<FlameMode>('kind');
  const [bottomTab, setBottomTab] = useState<BottomTab>('tree');
  const [categoryFilter, setCategoryFilter] = useState<string | null>(null);

  const fixture = FIXTURES.find(f => f.key === fixtureKey)!;

  useEffect(() => {
    setError(null);
    setSelected(null);
    fetch(fixture.json)
      .then(r => r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`)))
      .then((data: Report) => {
        // Sort entry points by subtree size, biggest first.
        const sorted = [...data.entries].sort((a, b) => subtreeWeight(b) - subtreeWeight(a));
        setReport({ ...data, entries: sorted });
        setActiveRootId(sorted[0]?.id ?? null);
      })
      .catch(e => setError(String(e)));
  }, [fixture.json]);

  const activeRoot = useMemo(
    () => report?.entries.find(r => r.id === activeRootId) ?? null,
    [report, activeRootId],
  );

  // Cross-root index: for every reachable node, remember (root id, node ref).
  // Lets us jump from Statistics/HotPaths into the right entry-point tree.
  const nodeIndex = useMemo(() => {
    const byId = new Map<string, { rootId: string; node: CallTreeNode }>();
    const byFileLine = new Map<string, { rootId: string; node: CallTreeNode }>();
    const byName = new Map<string, { rootId: string; node: CallTreeNode }>();
    if (!report) return { byId, byFileLine, byName };
    for (const root of report.entries) {
      const walk = (n: CallTreeNode) => {
        if (!byId.has(n.id)) byId.set(n.id, { rootId: root.id, node: n });
        const fl = `${n.file}:${n.line}`;
        if (!byFileLine.has(fl)) byFileLine.set(fl, { rootId: root.id, node: n });
        const fullName = (n.parent_class ? `${n.parent_class}.` : '') + n.name;
        if (!byName.has(fullName)) byName.set(fullName, { rootId: root.id, node: n });
        if (!byName.has(n.name)) byName.set(n.name, { rootId: root.id, node: n });
        for (const c of n.children) walk(c);
      };
      walk(root);
    }
    return { byId, byFileLine, byName };
  }, [report]);

  const flameRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 800, h: 320 });
  useEffect(() => {
    if (!flameRef.current) return;
    const ro = new ResizeObserver(entries => {
      for (const e of entries) {
        const r = e.contentRect;
        setSize({ w: Math.max(320, Math.floor(r.width)), h: Math.max(180, Math.floor(r.height)) });
      }
    });
    ro.observe(flameRef.current);
    return () => ro.disconnect();
  }, []);

  // Jump by id, falling back to (file:line) or name; switches active root if needed.
  const jump = (lookup: { id?: string; file?: string; line?: number; name?: string }) => {
    let hit: { rootId: string; node: CallTreeNode } | undefined;
    if (lookup.id) hit = nodeIndex.byId.get(lookup.id);
    if (!hit && lookup.file && typeof lookup.line === 'number') {
      hit = nodeIndex.byFileLine.get(`${lookup.file}:${lookup.line}`);
    }
    if (!hit && lookup.name) hit = nodeIndex.byName.get(lookup.name);
    if (!hit) return;
    if (hit.rootId !== activeRootId) setActiveRootId(hit.rootId);
    setSelected(hit.node);
  };

  const jumpTo = (id: string) => jump({ id });

  return (
    <div style={appStyle}>
      <Toolbar
        fixtureKey={fixtureKey}
        setFixtureKey={setFixtureKey}
        roots={report?.entries ?? []}
        activeRootId={activeRootId}
        setActiveRootId={setActiveRootId}
        search={search}
        setSearch={setSearch}
        flameMode={flameMode}
        setFlameMode={setFlameMode}
        description={fixture.description}
      />
      <SummaryBar
        summary={report?.summary ?? null}
        activeCategory={categoryFilter}
        onToggleCategory={(c) => setCategoryFilter(prev => (prev === c ? null : c))}
      />
      <div style={bodyStyle}>
        <div style={mainStyle}>
          <div style={paneHeaderStyle}>
            <Help text={TIPS.flame_graph}>FLAME GRAPH</Help>
            <span style={{ marginLeft: 12, color: '#6e717a', fontWeight: 400 }}>
              · <Help
                  text={flameMode === 'kind' ? TIPS.flame_mode_kind
                    : flameMode === 'category' ? TIPS.flame_mode_category
                    : flameMode === 'complexity' ? TIPS.flame_mode_complexity
                    : TIPS.flame_mode_smells}
                >
                  color by {flameMode === 'kind' ? 'symbol kind'
                    : flameMode === 'category' ? 'resource category'
                    : flameMode === 'complexity' ? 'complexity'
                    : 'smells'}
                </Help>
            </span>
            {categoryFilter && (
              <span style={filterChipStyle}>
                filter: {categoryFilter}
                <button onClick={() => setCategoryFilter(null)} style={chipCloseStyle} title="clear filter">×</button>
              </span>
            )}
            {selected && (
              <button onClick={() => setSelected(activeRoot)} style={resetBtnStyle} title="show entry-point root in details">
                ← back to root
              </button>
            )}
          </div>
          <div ref={flameRef} style={flamePanelStyle}>
            {error && <div style={errorStyle}>load error: {error}</div>}
            {!error && activeRoot && (
              <FlameView
                root={activeRoot}
                search={search}
                mode={flameMode}
                categoryFilter={categoryFilter}
                onSelect={setSelected}
                height={size.h}
                width={size.w}
              />
            )}
            {!error && !activeRoot && <div style={emptyStyle}>no data</div>}
          </div>
          <div style={tabsStyle}>
            <Tab active={bottomTab === 'tree'} onClick={() => setBottomTab('tree')}>Call Tree</Tab>
            <Tab active={bottomTab === 'hot'} onClick={() => setBottomTab('hot')}>
              Hot Paths{report?.summary.hot_paths.length ? ` (${report.summary.hot_paths.length})` : ''}
            </Tab>
            <Tab active={bottomTab === 'smells'} onClick={() => setBottomTab('smells')}>
              Smells{smellsCount(activeRoot) ? ` (${smellsCount(activeRoot)})` : ''}
            </Tab>
            <Tab active={bottomTab === 'stats'} onClick={() => setBottomTab('stats')}>Statistics</Tab>
          </div>
          <div style={bottomPanelStyle}>
            {bottomTab === 'tree' && activeRoot && (
              <CallTreeView
                root={activeRoot}
                search={search}
                selectedId={selected?.id ?? null}
                onSelect={setSelected}
              />
            )}
            {bottomTab === 'hot' && (
              <HotPaths paths={report?.summary.hot_paths ?? []} onJump={(name) => jump({ name })} />
            )}
            {bottomTab === 'smells' && (
              <Smells root={activeRoot} onSelect={setSelected} />
            )}
            {bottomTab === 'stats' && (
              <Statistics summary={report?.summary ?? null} onJump={jump} />
            )}
          </div>
        </div>
        <div style={sidebarStyle}>
          <DetailsPane node={selected ?? activeRoot} onJumpTo={jumpTo} onJumpExternal={(file, line) => jump({ file, line })} />
        </div>
      </div>
    </div>
  );
}

function smellsCount(node: CallTreeNode | null): number {
  if (!node) return 0;
  let n = 0;
  const visit = (x: CallTreeNode) => {
    if (x.n_plus_one_risk) n++;
    if (x.blocking_in_async) n++;
    if (x.is_recursive) n++;
    for (const c of x.children) visit(c);
  };
  visit(node);
  return n;
}

function findInTree(node: CallTreeNode, id: string): CallTreeNode | null {
  if (node.id === id) return node;
  for (const c of node.children) {
    const hit = findInTree(c, id);
    if (hit) return hit;
  }
  return null;
}

function Toolbar(props: {
  fixtureKey: string;
  setFixtureKey: (k: string) => void;
  roots: CallTreeNode[];
  activeRootId: string | null;
  setActiveRootId: (id: string) => void;
  search: string;
  setSearch: (s: string) => void;
  flameMode: FlameMode;
  setFlameMode: (m: FlameMode) => void;
  description: string;
}) {
  const { fixtureKey, setFixtureKey, roots, activeRootId, setActiveRootId, search, setSearch, flameMode, setFlameMode, description } = props;
  return (
    <div style={toolbarStyle}>
      <div style={brandStyle}>drift · static profiler</div>
      <label style={labelStyle}>Fixture</label>
      <select value={fixtureKey} onChange={e => setFixtureKey(e.target.value)} style={selectStyle}>
        {FIXTURES.map(f => <option key={f.key} value={f.key}>{f.label}</option>)}
      </select>
      <label style={labelStyle}>Entry</label>
      <select
        value={activeRootId ?? ''}
        onChange={e => setActiveRootId(e.target.value)}
        style={{ ...selectStyle, minWidth: 280 }}
      >
        {roots.map(r => (
          <option key={r.id} value={r.id}>
            {(r.parent_class ? `${r.parent_class}.` : '') + r.name} — {r.file}:{r.line}
          </option>
        ))}
      </select>
      <label style={labelStyle}>Color</label>
      <select value={flameMode} onChange={e => setFlameMode(e.target.value as FlameMode)} style={selectStyle}>
        <option value="kind" title={TIPS.flame_mode_kind}>by kind</option>
        <option value="category" title={TIPS.flame_mode_category}>by category</option>
        <option value="complexity" title={TIPS.flame_mode_complexity}>by complexity</option>
        <option value="smells" title={TIPS.flame_mode_smells}>smells only</option>
      </select>
      <input
        type="text"
        value={search}
        onChange={e => setSearch(e.target.value)}
        placeholder="search…"
        style={inputStyle}
      />
      <span style={descStyle}>{description}</span>
    </div>
  );
}

function Tab({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button onClick={onClick} style={{ ...tabStyle, ...(active ? tabActiveStyle : {}) }}>
      {children}
    </button>
  );
}

// --- styles ---

const appStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateRows: 'auto auto 1fr',
  height: '100vh',
  background: '#1e1f22',
  color: '#d7d9dc',
};
const bodyStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: '1fr 340px',
  overflow: 'hidden',
};
const mainStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateRows: 'auto 1fr auto 1fr',
  overflow: 'hidden',
};
const sidebarStyle: React.CSSProperties = { overflow: 'hidden' };
const toolbarStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  padding: '8px 16px',
  background: '#26282c',
  borderBottom: '1px solid #3f4147',
  flexWrap: 'wrap',
};
const brandStyle: React.CSSProperties = { fontWeight: 600, color: '#9ca0a8', letterSpacing: 0.3, marginRight: 8 };
const labelStyle: React.CSSProperties = { color: '#7e8189', fontSize: 10, textTransform: 'uppercase', letterSpacing: 0.5 };
const selectStyle: React.CSSProperties = {
  background: '#1e1f22',
  color: '#d7d9dc',
  border: '1px solid #3f4147',
  borderRadius: 4,
  padding: '4px 6px',
  fontSize: 12,
};
const inputStyle: React.CSSProperties = { ...selectStyle, width: 160 };
const descStyle: React.CSSProperties = { marginLeft: 'auto', color: '#7e8189', fontSize: 11, fontStyle: 'italic' };
const paneHeaderStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center',
  fontSize: 10, fontWeight: 700, letterSpacing: 0.8, color: '#7e8189',
  padding: '6px 12px', background: '#26282c', borderBottom: '1px solid #3f4147',
  textTransform: 'uppercase',
};
const flamePanelStyle: React.CSSProperties = { background: '#1e1f22', overflow: 'hidden', minHeight: 180 };
const tabsStyle: React.CSSProperties = {
  display: 'flex', gap: 0, background: '#26282c', borderTop: '1px solid #3f4147', borderBottom: '1px solid #3f4147',
};
const tabStyle: React.CSSProperties = {
  padding: '6px 14px', background: 'transparent', border: 'none', color: '#9ca0a8',
  fontSize: 11, textTransform: 'uppercase', letterSpacing: 0.5, cursor: 'pointer', fontWeight: 600,
};
const tabActiveStyle: React.CSSProperties = { color: '#d7d9dc', borderBottom: '2px solid #5b8def' };
const bottomPanelStyle: React.CSSProperties = { overflow: 'hidden', background: '#1e1f22' };
const errorStyle: React.CSSProperties = { color: '#ff7e7e', padding: 16, fontFamily: 'monospace' };
const emptyStyle: React.CSSProperties = { color: '#6e717a', padding: 16, fontStyle: 'italic' };
const filterChipStyle: React.CSSProperties = {
  marginLeft: 12, padding: '2px 8px', borderRadius: 3, background: '#3a3326',
  color: '#ffd569', fontSize: 10, textTransform: 'uppercase', letterSpacing: 0.4,
  display: 'inline-flex', alignItems: 'center', gap: 6,
};
const chipCloseStyle: React.CSSProperties = {
  background: 'transparent', border: 'none', color: '#ffd569', cursor: 'pointer',
  fontSize: 14, lineHeight: 1, padding: 0,
};
const resetBtnStyle: React.CSSProperties = {
  marginLeft: 'auto', background: 'transparent', border: '1px solid #3f4147',
  color: '#9ca0a8', fontSize: 10, padding: '2px 8px', borderRadius: 3,
  cursor: 'pointer', textTransform: 'uppercase', letterSpacing: 0.4,
};
