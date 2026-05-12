import { useMemo } from 'react';
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
} from './types';
import type {
  CallTreeNode,
  Category,
  FindingKind,
  Report,
  Severity,
} from './types';

interface Props {
  report: Report | null;
  /// Same shape App.tsx uses for the existing Hot Paths / Statistics tabs.
  /// `id` is the preferred key (matches `nodeIndex.byId`); name/file/line
  /// are fallbacks.
  onJump?: (lookup: { id?: string; file?: string; line?: number; name?: string }) => void;
  /// When the user clicks a kind in the findings breakdown, the parent
  /// should flip to the Insights tab pre-filtered. App.tsx wires this.
  onShowKind?: (kind: FindingKind) => void;
  /// Flip to the Tree tab and select the given root.
  onPickRoot?: (id: string) => void;
}

export function ScanReport({ report, onJump, onShowKind, onPickRoot }: Props) {
  if (!report) {
    return <div style={emptyStyle}>no report loaded</div>;
  }
  const summary = report.summary;
  const findingsByKind = summary.findings_by_kind ?? {};
  const findingsTop = summary.findings_top ?? [];
  const totalFindings = Object.values(findingsByKind).reduce((a, b) => a + b, 0);

  const sevCounts = useMemo(() => {
    const counts: Record<Severity, number> = { high: 0, medium: 0, low: 0 };
    for (const t of findingsTop) counts[t.severity]++;
    // findings_top is capped at 50 in Rust; for the full picture we recount
    // by walking the tree if needed. For now use the capped count; cap is
    // generous and severity distribution is what matters for the gauge.
    return counts;
  }, [findingsTop]);

  const healthScore = useMemo(() => {
    // Composite, deliberately rough — see ScanReport spec in INSIGHTS_PLAN.md.
    // 10 − weighted-sum-of-findings, floored at 0.
    let s = 10;
    s -= sevCounts.high * 0.5;
    s -= sevCounts.medium * 0.2;
    s -= sevCounts.low * 0.05;
    return Math.max(0, s);
  }, [sevCounts]);

  const entries = report.entries;
  const cats = Object.entries(summary.categories ?? {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);

  const langBreakdown = (summary as { language_breakdown?: { language: string; percent: number }[] })
    .language_breakdown ?? [];

  const topHotZones = useMemo(() => {
    // Prefer explicit HotZone findings if present; otherwise fall back to
    // the top-pagerank symbols as a proxy (older fixtures, or scans run
    // before the hot_zone detector lands in step 11).
    const hz = findingsTop.filter((t) => t.kind === 'hot_zone').slice(0, 5);
    if (hz.length > 0) return hz;
    return summary.pagerank_top.slice(0, 5).map((r) => ({
      kind: 'hot_zone' as FindingKind,
      severity: 'low' as Severity,
      node_id: '',
      line: r.line,
      name: r.name,
      file: r.file,
      parent_class: r.parent_class,
      score: r.score,
    }));
  }, [findingsTop, summary.pagerank_top]);

  return (
    <div style={containerStyle}>
      <Header report={report} />
      <div style={gridStyle}>
        <HealthCard score={healthScore} sevCounts={sevCounts} totalFindings={totalFindings} />
        <FindingsBreakdownCard
          byKind={findingsByKind}
          totalFindings={totalFindings}
          onShowKind={onShowKind}
        />
        <CategoriesCard cats={cats} />
        <LanguagesCard languages={langBreakdown} />
        <HotZonesCard zones={topHotZones} onJump={onJump} />
        <EntryPointsCard entries={entries} onPickRoot={onPickRoot} />
      </div>
    </div>
  );
}

// ─── Header ──────────────────────────────────────────────────────────────

function Header({ report }: { report: Report }) {
  const root = report.generator?.source_root ?? '';
  const base = root ? root.replace(/[/\\]+$/, '').split(/[/\\]/).pop() : null;
  return (
    <div style={headerStyle}>
      <div style={headerTitleStyle}>
        scan report{base ? ` — .../${base}` : ''}
      </div>
      <div style={headerSubStyle}>
        {report.generator?.tool ?? 'drift-static-profiler'} {report.generator?.version ?? ''}
        {' · '}
        {report.summary.profiled_language ?? '—'}
        {report.summary.profiled_language_percent !== undefined && report.summary.profiled_language_percent !== null
          ? ` ${report.summary.profiled_language_percent.toFixed(0)}%`
          : ''}
        {' · '}
        {report.summary.files} files · {report.summary.symbols} symbols · {report.summary.edges} edges
      </div>
    </div>
  );
}

// ─── Health gauge ────────────────────────────────────────────────────────

function HealthCard({
  score,
  sevCounts,
  totalFindings,
}: {
  score: number;
  sevCounts: Record<Severity, number>;
  totalFindings: number;
}) {
  const pct = score / 10; // 0..1
  return (
    <Panel title="health score" tip="composite — for trend tracking">
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div style={gaugeOuterStyle}>
          <div style={{ ...gaugeFillStyle, width: `${pct * 100}%` }} />
        </div>
        <div style={{ fontSize: 22, fontWeight: 700, color: '#d7d9dc', minWidth: 64 }}>
          {score.toFixed(1)}
          <span style={{ fontSize: 12, color: '#7e8189', fontWeight: 400 }}> / 10</span>
        </div>
      </div>
      <div style={{ marginTop: 10, display: 'flex', gap: 12, fontSize: 11, color: '#9ca0a8' }}>
        <SevPill sev="high" count={sevCounts.high} />
        <SevPill sev="medium" count={sevCounts.medium} />
        <SevPill sev="low" count={sevCounts.low} />
        <span style={{ marginLeft: 'auto', color: '#7e8189' }}>{totalFindings} total findings</span>
      </div>
      <div style={{ marginTop: 8, fontSize: 10, color: '#5f626a' }}>
        weighted sum: 10 − (high × 0.5 + medium × 0.2 + low × 0.05), floored at 0
      </div>
    </Panel>
  );
}

function SevPill({ sev, count }: { sev: Severity; count: number }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
      <span style={{ ...miniBadgeStyle, background: SEVERITY_COLORS[sev] }}>{sev}</span>
      <strong style={{ color: '#d7d9dc' }}>{count}</strong>
    </span>
  );
}

// ─── Findings breakdown ─────────────────────────────────────────────────

function FindingsBreakdownCard({
  byKind,
  totalFindings,
  onShowKind,
}: {
  byKind: Record<string, number>;
  totalFindings: number;
  onShowKind?: (kind: FindingKind) => void;
}) {
  const rows = Object.entries(byKind)
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
  const max = rows.reduce((m, [, v]) => Math.max(m, v), 1);
  return (
    <Panel title={`findings by kind · ${totalFindings} total`}>
      {rows.length === 0 ? (
        <Empty msg="no findings yet" />
      ) : (
        <ul style={listStyle}>
          {rows.map(([kind, n]) => (
            <li
              key={kind}
              style={liButtonStyle}
              onClick={() => onShowKind?.(kind as FindingKind)}
              title={`Show ${n} ${kind} finding(s) in the Insights tab.`}
            >
              <span style={{ ...kindBadgeStyle, minWidth: 130 }}>
                {FINDING_KIND_LABEL[kind as FindingKind] ?? kind}
              </span>
              <span style={{ flex: 1, marginLeft: 8 }}>
                <span style={{ ...barOuterStyle }}>
                  <span style={{ ...barFillStyle, width: `${(n / max) * 100}%` }} />
                </span>
              </span>
              <span style={countNumStyle}>{n}</span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

// ─── Category reach ─────────────────────────────────────────────────────

function CategoriesCard({ cats }: { cats: [string, number][] }) {
  const max = cats.reduce((m, [, v]) => Math.max(m, v), 1);
  return (
    <Panel title="category reach">
      {cats.length === 0 ? (
        <Empty msg="no resource calls detected" />
      ) : (
        <ul style={listStyle}>
          {cats.map(([cat, n]) => (
            <li key={cat} style={liStyle}>
              <span style={{ ...miniBadgeStyle, background: CATEGORY_COLORS[cat as Category], minWidth: 60 }}>
                {cat}
              </span>
              <span style={{ flex: 1, marginLeft: 8 }}>
                <span style={{ ...barOuterStyle }}>
                  <span style={{ ...barFillStyle, width: `${(n / max) * 100}%`, background: CATEGORY_COLORS[cat as Category] }} />
                </span>
              </span>
              <span style={countNumStyle}>{n}</span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

// ─── Languages ──────────────────────────────────────────────────────────

function LanguagesCard({
  languages,
}: {
  languages: { language: string; percent: number }[];
}) {
  if (languages.length === 0) {
    return (
      <Panel title="language breakdown">
        <Empty msg="—" />
      </Panel>
    );
  }
  return (
    <Panel title="language breakdown">
      <ul style={listStyle}>
        {languages.slice(0, 8).map((l, i) => (
          <li key={i} style={liStyle}>
            <span style={{ width: 100, color: '#d7d9dc' }}>{l.language}</span>
            <span style={{ flex: 1 }}>
              <span style={{ ...barOuterStyle }}>
                <span style={{ ...barFillStyle, width: `${l.percent}%` }} />
              </span>
            </span>
            <span style={countNumStyle}>{l.percent.toFixed(1)}%</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

// ─── Top hot zones ──────────────────────────────────────────────────────

function HotZonesCard({
  zones,
  onJump,
}: {
  zones: (
    | {
        kind: FindingKind;
        severity: Severity;
        node_id: string;
        line: number;
        name?: string;
        file?: string;
        parent_class?: string | null;
        score?: number;
      }
  )[];
  onJump?: Props['onJump'];
}) {
  if (zones.length === 0) {
    return (
      <Panel title="top hot zones">
        <Empty msg="—" />
      </Panel>
    );
  }
  return (
    <Panel title="top hot zones · pagerank">
      <ul style={listStyle}>
        {zones.map((z, i) => {
          // For HotZone findings we only have node_id; we let the parent's
          // existing jump() handle id-based lookup. For pagerank fallback
          // entries we pass file/line/name like Statistics.tsx does.
          const hasId = !!z.node_id;
          return (
            <li
              key={i}
              style={liButtonStyle}
              onClick={() =>
                hasId
                  ? onJump?.({ id: z.node_id })
                  : onJump?.({ file: z.file, line: z.line, name: (z.parent_class ? `${z.parent_class}.` : '') + (z.name ?? '') })
              }
            >
              <span style={{ ...miniBadgeStyle, background: SEVERITY_COLORS[z.severity], minWidth: 50 }}>
                {z.severity}
              </span>
              <code style={{ ...codeStyle, marginLeft: 8 }}>
                {hasId ? lastSegment(z.node_id) : (z.parent_class ? `${z.parent_class}.` : '') + (z.name ?? '')}
              </code>
              <span style={locStyle}>
                {hasId ? fileLineFromId(z.node_id, z.line) : `${z.file}:${z.line}`}
                {z.score !== undefined && (
                  <span style={{ marginLeft: 6, color: '#5b8def' }}>· {z.score.toFixed(3)}</span>
                )}
              </span>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

function lastSegment(id: string): string {
  // id is `file::class::name` — show "class.name" or "name".
  const parts = id.split('::');
  if (parts.length >= 3) {
    const cls = parts[parts.length - 2];
    const name = parts[parts.length - 1];
    return cls ? `${cls}.${name}` : name;
  }
  return id;
}

function fileLineFromId(id: string, line: number): string {
  const parts = id.split('::');
  return `${parts[0]}:${line}`;
}

// ─── Entry points ──────────────────────────────────────────────────────

function EntryPointsCard({
  entries,
  onPickRoot,
}: {
  entries: CallTreeNode[];
  onPickRoot?: (id: string) => void;
}) {
  const top = useMemo(
    () => [...entries].sort((a, b) => b.subtree_size - a.subtree_size).slice(0, 8),
    [entries],
  );
  return (
    <Panel title={`entry points · ${entries.length}`}>
      {entries.length === 0 ? (
        <Empty msg="—" />
      ) : (
        <ul style={listStyle}>
          {top.map((e) => (
            <li
              key={e.id}
              style={liButtonStyle}
              onClick={() => onPickRoot?.(e.id)}
              title={`Switch to the Tree tab and select ${e.name}`}
            >
              <code style={codeStyle}>
                {e.parent_class ? <span style={{ color: '#7e8189' }}>{e.parent_class}.</span> : null}
                {e.name}
              </code>
              <span style={{ marginLeft: 'auto', color: '#7e8189', fontSize: 10 }}>
                reach {e.subtree_size}
              </span>
              <span style={locStyle}>{e.file}:{e.line}</span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

// ─── Panel + shared bits ────────────────────────────────────────────────

function Panel({
  title,
  children,
  tip,
}: {
  title: string;
  children: React.ReactNode;
  tip?: string;
}) {
  return (
    <div style={panelStyle}>
      <div style={panelHeaderStyle} title={tip}>{title}</div>
      <div style={panelBodyStyle}>{children}</div>
    </div>
  );
}

function Empty({ msg }: { msg?: string }) {
  return (
    <div style={{ padding: 14, color: '#7e8189', fontSize: 11, fontStyle: 'italic' }}>
      {msg ?? '—'}
    </div>
  );
}

// ─── styles ─────────────────────────────────────────────────────────────

const containerStyle: React.CSSProperties = {
  height: '100%',
  overflow: 'auto',
  padding: 14,
  background: '#1e1f22',
};
const headerStyle: React.CSSProperties = {
  marginBottom: 14,
  paddingBottom: 10,
  borderBottom: '1px solid #3f4147',
};
const headerTitleStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 600,
  color: '#d7d9dc',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
};
const headerSubStyle: React.CSSProperties = {
  fontSize: 11,
  color: '#7e8189',
  marginTop: 4,
};
const gridStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(360px, 1fr))',
  gap: 12,
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
const panelBodyStyle: React.CSSProperties = {
  padding: 10,
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
  gap: 6,
  padding: '4px 4px',
  borderBottom: '1px solid #2f3136',
};
const liButtonStyle: React.CSSProperties = {
  ...liStyle,
  cursor: 'pointer',
};
const codeStyle: React.CSSProperties = {
  background: '#1e1f22',
  padding: '2px 6px',
  borderRadius: 3,
  color: '#d7d9dc',
  whiteSpace: 'nowrap',
};
const countNumStyle: React.CSSProperties = {
  minWidth: 40,
  textAlign: 'right',
  color: '#d7d9dc',
};
const locStyle: React.CSSProperties = {
  marginLeft: 'auto',
  color: '#7e8189',
  fontSize: 10,
};
const miniBadgeStyle: React.CSSProperties = {
  display: 'inline-block',
  padding: '1px 6px',
  borderRadius: 3,
  color: '#0a0a14',
  fontSize: 9,
  fontWeight: 700,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
  textAlign: 'center',
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
const barOuterStyle: React.CSSProperties = {
  display: 'inline-block',
  width: '100%',
  height: 8,
  background: '#1e1f22',
  borderRadius: 2,
  position: 'relative',
  overflow: 'hidden',
};
const barFillStyle: React.CSSProperties = {
  display: 'block',
  height: '100%',
  background: '#5b8def',
};
const gaugeOuterStyle: React.CSSProperties = {
  flex: 1,
  height: 18,
  background: '#1e1f22',
  borderRadius: 2,
  border: '1px solid #3f4147',
  overflow: 'hidden',
};
const gaugeFillStyle: React.CSSProperties = {
  height: '100%',
  background: 'linear-gradient(90deg, #e26d6d 0%, #e0a458 50%, #48a999 100%)',
};
const emptyStyle: React.CSSProperties = {
  padding: 20,
  color: '#d7d9dc',
  fontSize: 13,
};
