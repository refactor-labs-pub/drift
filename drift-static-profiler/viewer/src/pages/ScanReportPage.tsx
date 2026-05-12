import { useMemo } from 'react';
import { Link } from 'react-router-dom';
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
} from '../types';
import type {
  CallTreeNode,
  Category,
  FindingKind,
  Severity,
} from '../types';
import { flattenFindings, useReport } from './useReport';

/**
 * Full-page dedicated Scan Report — distinct from the in-tab
 * `ScanReport.tsx` component. This is its own route at
 * `/scan/:fixtureKey/report` and treats the JSON as a first-class
 * document.
 *
 * Everything that can be clicked is a `<Link>` to a real URL:
 *  - Findings → /scan/:fixtureKey/finding/:findingIdx
 *  - Hot zones / entry points → /scan/:fixtureKey/node/:nodeId
 *  - Category badges → in-tab dashboard with filter
 *  - Raw JSON link in the header
 *  - Breadcrumb back to the fixture index
 */
export function ScanReportPage() {
  const { report, fixture, fixtureKey, error, loading } = useReport();

  // Walk every entry tree once to produce a flat, deterministically-indexed
  // list of findings. The viewer routes finding detail pages by index, so
  // this is the canonical numbering.
  const allFindings = useMemo(() => flattenFindings(report), [report]);

  if (!fixtureKey) {
    return <ErrorScreen message="no fixture key in URL" />;
  }
  if (loading) {
    return <LoadingScreen />;
  }
  if (error || !report || !fixture) {
    return <ErrorScreen message={error ?? 'no report'} fixtureKey={fixtureKey} />;
  }

  const summary = report.summary;
  const findingsByKind = summary.findings_by_kind ?? {};
  const findingsTop = summary.findings_top ?? [];
  const totalFindings = Object.values(findingsByKind).reduce((a, b) => a + b, 0);

  const sevCounts: Record<Severity, number> = useMemo(() => {
    const counts: Record<Severity, number> = { high: 0, medium: 0, low: 0 };
    for (const t of allFindings) counts[t.finding.severity]++;
    return counts;
  }, [allFindings]);

  const healthScore = useMemo(() => {
    let s = 10 - sevCounts.high * 0.5 - sevCounts.medium * 0.2 - sevCounts.low * 0.05;
    return Math.max(0, s);
  }, [sevCounts]);

  const cats = Object.entries(summary.categories ?? {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);

  const langBreakdown = summary.language_breakdown ?? [];
  const entries = report.entries;

  return (
    <div style={pageStyle}>
      <header style={headerBarStyle}>
        <nav style={breadcrumbStyle}>
          <Link to="/" style={crumbLinkStyle}>scans</Link>
          <span style={crumbSepStyle}>/</span>
          <Link to={`/scan/${fixtureKey}`} style={crumbLinkStyle}>{fixture.label}</Link>
          <span style={crumbSepStyle}>/</span>
          <span style={crumbCurrentStyle}>report</span>
        </nav>
        <div style={headerActionsStyle}>
          <Link to={`/scan/${fixtureKey}`} style={secondaryBtnStyle}>
            Open dashboard →
          </Link>
          <a
            href={fixture.json}
            target="_blank"
            rel="noreferrer"
            style={secondaryBtnStyle}
            title="Open the raw scan JSON in a new tab — bookmarkable, copy-pasteable."
          >
            View raw JSON ↗
          </a>
        </div>
      </header>

      <section style={titleBlockStyle}>
        <h1 style={titleStyle}>
          Scan report — <span style={titleHighlightStyle}>{fixture.label}</span>
        </h1>
        <p style={subtitleStyle}>{fixture.description}</p>
        <div style={metaRowStyle}>
          <Meta k="profiled" v={summary.profiled_language ?? '—'} />
          <Meta k="files" v={String(summary.files)} />
          <Meta k="symbols" v={String(summary.symbols)} />
          <Meta k="edges" v={String(summary.edges)} />
          <Meta k="entry points" v={String(entries.length)} />
          <Meta
            k="findings"
            v={`${totalFindings}`}
            link={totalFindings > 0 ? `/scan/${fixtureKey}` : undefined}
          />
          {report.generator?.tool && (
            <Meta k="generator" v={`${report.generator.tool} ${report.generator.version ?? ''}`.trim()} />
          )}
        </div>
      </section>

      <main style={gridStyle}>
        <HealthCard score={healthScore} sevCounts={sevCounts} total={totalFindings} />
        <FindingsCard
          byKind={findingsByKind}
          total={totalFindings}
          allFindings={allFindings}
          fixtureKey={fixtureKey}
        />
        <CategoriesCard cats={cats} fixtureKey={fixtureKey} />
        <LanguagesCard languages={langBreakdown} />
        <TopFindingsCard
          findingsTop={findingsTop}
          allFindings={allFindings}
          fixtureKey={fixtureKey}
        />
        <EntryPointsCard entries={entries} fixtureKey={fixtureKey} />
      </main>
    </div>
  );
}

// ─── Header subcomponents ───────────────────────────────────────────────

function Meta({ k, v, link }: { k: string; v: string; link?: string }) {
  const body = (
    <>
      <span style={metaKeyStyle}>{k}</span>
      <span style={metaValueStyle}>{v}</span>
    </>
  );
  if (link) {
    return (
      <Link to={link} style={{ ...metaStyle, textDecoration: 'none' }} title={`Open ${k}`}>
        {body}
      </Link>
    );
  }
  return <div style={metaStyle}>{body}</div>;
}

// ─── Health card ────────────────────────────────────────────────────────

function HealthCard({
  score, sevCounts, total,
}: { score: number; sevCounts: Record<Severity, number>; total: number }) {
  return (
    <Card title="health score" hint="composite — for trend tracking, not a benchmark">
      <div style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
        <div style={gaugeOuterStyle}>
          <div style={{ ...gaugeFillStyle, width: `${(score / 10) * 100}%` }} />
        </div>
        <div style={{ fontSize: 26, fontWeight: 700, color: '#d7d9dc', minWidth: 80 }}>
          {score.toFixed(1)}
          <span style={{ fontSize: 13, color: '#7e8189', fontWeight: 400 }}> / 10</span>
        </div>
      </div>
      <div style={{ marginTop: 12, display: 'flex', gap: 14, fontSize: 12, color: '#9ca0a8' }}>
        <SevPill sev="high" count={sevCounts.high} />
        <SevPill sev="medium" count={sevCounts.medium} />
        <SevPill sev="low" count={sevCounts.low} />
        <span style={{ marginLeft: 'auto', color: '#7e8189' }}>{total} total</span>
      </div>
      <div style={{ marginTop: 8, fontSize: 10, color: '#5f626a' }}>
        10 − (high × 0.5 + medium × 0.2 + low × 0.05), floored at 0
      </div>
    </Card>
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

// ─── Findings by kind ───────────────────────────────────────────────────

function FindingsCard({
  byKind, total, allFindings, fixtureKey,
}: {
  byKind: Record<string, number>;
  total: number;
  allFindings: { idx: number; finding: { kind: FindingKind } }[];
  fixtureKey: string;
}) {
  const rows = Object.entries(byKind)
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
  const max = rows.reduce((m, [, v]) => Math.max(m, v), 1);

  // Pre-compute the FIRST finding index for each kind so the row link
  // jumps straight into that kind's first finding detail page.
  const firstIdxOfKind = new Map<string, number>();
  for (const f of allFindings) {
    if (!firstIdxOfKind.has(f.finding.kind)) {
      firstIdxOfKind.set(f.finding.kind, f.idx);
    }
  }

  return (
    <Card title={`findings by kind · ${total} total`}>
      {rows.length === 0 ? (
        <Empty msg="no findings — clean scan or detectors disabled" />
      ) : (
        <ul style={listStyle}>
          {rows.map(([kind, n]) => {
            const idx = firstIdxOfKind.get(kind);
            const row = (
              <li style={liStyle}>
                <span style={{ ...kindBadgeStyle, minWidth: 140 }}>
                  {FINDING_KIND_LABEL[kind as FindingKind] ?? kind}
                </span>
                <span style={{ flex: 1, marginLeft: 8 }}>
                  <span style={barOuterStyle}>
                    <span style={{ ...barFillStyle, width: `${(n / max) * 100}%` }} />
                  </span>
                </span>
                <span style={countNumStyle}>{n}</span>
              </li>
            );
            return idx !== undefined ? (
              <Link
                key={kind}
                to={`/scan/${fixtureKey}/finding/${idx}`}
                style={rowLinkStyle}
                title={`Open the first ${kind} finding`}
              >
                {row}
              </Link>
            ) : (
              <div key={kind}>{row}</div>
            );
          })}
        </ul>
      )}
    </Card>
  );
}

// ─── Categories ─────────────────────────────────────────────────────────

function CategoriesCard({
  cats, fixtureKey,
}: { cats: [string, number][]; fixtureKey: string }) {
  const max = cats.reduce((m, [, v]) => Math.max(m, v), 1);
  return (
    <Card title="category reach">
      {cats.length === 0 ? (
        <Empty msg="no resource calls detected" />
      ) : (
        <ul style={listStyle}>
          {cats.map(([cat, n]) => (
            // Link to the dashboard for that fixture; the flame graph
            // there can be category-filtered. We don't currently support
            // a category query param in the URL, but the path is the
            // contract.
            <Link
              key={cat}
              to={`/scan/${fixtureKey}`}
              style={rowLinkStyle}
              title={`Open ${cat} calls in the flame view`}
            >
              <li style={liStyle}>
                <span style={{ ...miniBadgeStyle, background: CATEGORY_COLORS[cat as Category], minWidth: 70 }}>
                  {cat}
                </span>
                <span style={{ flex: 1, marginLeft: 8 }}>
                  <span style={barOuterStyle}>
                    <span style={{ ...barFillStyle, width: `${(n / max) * 100}%`, background: CATEGORY_COLORS[cat as Category] }} />
                  </span>
                </span>
                <span style={countNumStyle}>{n}</span>
              </li>
            </Link>
          ))}
        </ul>
      )}
    </Card>
  );
}

// ─── Languages ──────────────────────────────────────────────────────────

function LanguagesCard({
  languages,
}: { languages: { language: string; percent: number }[] }) {
  if (languages.length === 0) return <Card title="language breakdown"><Empty msg="—" /></Card>;
  return (
    <Card title="language breakdown">
      <ul style={listStyle}>
        {languages.slice(0, 8).map((l, i) => (
          <li key={i} style={liStyle}>
            <span style={{ width: 100, color: '#d7d9dc' }}>{l.language}</span>
            <span style={{ flex: 1 }}>
              <span style={barOuterStyle}>
                <span style={{ ...barFillStyle, width: `${l.percent}%` }} />
              </span>
            </span>
            <span style={countNumStyle}>{l.percent.toFixed(1)}%</span>
          </li>
        ))}
      </ul>
    </Card>
  );
}

// ─── Top findings (linkable to deep finding pages) ──────────────────────

function TopFindingsCard({
  findingsTop, allFindings, fixtureKey,
}: {
  findingsTop: { kind: FindingKind; severity: Severity; line: number; node_id: string }[];
  allFindings: { idx: number; finding: { kind: FindingKind; line: number }; node: { id: string } }[];
  fixtureKey: string;
}) {
  // Resolve each FindingTopRef to the flattened index so we can deep-link.
  const idxOf = (ref: { kind: FindingKind; line: number; node_id: string }): number | null => {
    const hit = allFindings.find(
      (f) => f.finding.kind === ref.kind && f.finding.line === ref.line && f.node.id === ref.node_id,
    );
    return hit ? hit.idx : null;
  };

  if (findingsTop.length === 0) {
    return <Card title="top findings"><Empty msg="—" /></Card>;
  }
  return (
    <Card title={`top findings · ${findingsTop.length}`}>
      <ul style={listStyle}>
        {findingsTop.slice(0, 10).map((t, i) => {
          const idx = idxOf(t);
          const label = lastSegment(t.node_id);
          const inner = (
            <li style={liStyle}>
              <span style={{ ...miniBadgeStyle, background: SEVERITY_COLORS[t.severity], minWidth: 60 }}>
                {t.severity}
              </span>
              <span style={{ ...kindBadgeStyle, marginLeft: 6 }}>
                {FINDING_KIND_LABEL[t.kind] ?? t.kind}
              </span>
              <code style={{ ...codeStyle, marginLeft: 8 }}>{label}</code>
              <span style={locStyle}>:{t.line}</span>
            </li>
          );
          return idx !== null ? (
            <Link
              key={i}
              to={`/scan/${fixtureKey}/finding/${idx}`}
              style={rowLinkStyle}
              title="Open finding detail page"
            >
              {inner}
            </Link>
          ) : (
            <div key={i}>{inner}</div>
          );
        })}
      </ul>
    </Card>
  );
}

function lastSegment(id: string): string {
  const parts = id.split('::');
  if (parts.length >= 3) {
    const cls = parts[parts.length - 2];
    const name = parts[parts.length - 1];
    return cls ? `${cls}.${name}` : name;
  }
  return id;
}

// ─── Entry points ──────────────────────────────────────────────────────

function EntryPointsCard({
  entries, fixtureKey,
}: { entries: CallTreeNode[]; fixtureKey: string }) {
  const top = [...entries].sort((a, b) => b.subtree_size - a.subtree_size).slice(0, 10);
  return (
    <Card title={`entry points · ${entries.length}`}>
      {entries.length === 0 ? (
        <Empty msg="—" />
      ) : (
        <ul style={listStyle}>
          {top.map((e) => (
            <Link
              key={e.id}
              to={`/scan/${fixtureKey}/node/${encodeURIComponent(e.id)}`}
              style={rowLinkStyle}
              title={`Open ${e.name} detail page`}
            >
              <li style={liStyle}>
                <code style={codeStyle}>
                  {e.parent_class ? <span style={{ color: '#7e8189' }}>{e.parent_class}.</span> : null}
                  {e.name}
                </code>
                <span style={{ marginLeft: 'auto', color: '#7e8189', fontSize: 10 }}>
                  reach {e.subtree_size}
                </span>
                <span style={locStyle}>{e.file}:{e.line}</span>
              </li>
            </Link>
          ))}
        </ul>
      )}
    </Card>
  );
}

// ─── Common: Card / Empty / Loading / Error ─────────────────────────────

function Card({
  title, children, hint,
}: { title: string; children: React.ReactNode; hint?: string }) {
  return (
    <div style={cardStyle}>
      <div style={cardHeaderStyle} title={hint}>{title}</div>
      <div style={cardBodyStyle}>{children}</div>
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

function LoadingScreen() {
  return (
    <div style={pageStyle}>
      <div style={{ padding: 60, textAlign: 'center', color: '#7e8189' }}>loading…</div>
    </div>
  );
}

function ErrorScreen({ message, fixtureKey }: { message: string; fixtureKey?: string }) {
  return (
    <div style={pageStyle}>
      <header style={headerBarStyle}>
        <nav style={breadcrumbStyle}>
          <Link to="/" style={crumbLinkStyle}>scans</Link>
          <span style={crumbSepStyle}>/</span>
          <span style={crumbCurrentStyle}>{fixtureKey ?? 'unknown'}</span>
        </nav>
      </header>
      <div style={{ padding: 24 }}>
        <div style={{ color: '#ff7e7e', fontFamily: 'monospace' }}>error: {message}</div>
        <Link to="/" style={{ color: '#5b8def', marginTop: 12, display: 'inline-block' }}>← back to fixture index</Link>
      </div>
    </div>
  );
}

// ─── styles ─────────────────────────────────────────────────────────────

const pageStyle: React.CSSProperties = {
  minHeight: '100vh', background: '#1e1f22', color: '#d7d9dc',
};
const headerBarStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', justifyContent: 'space-between',
  padding: '10px 24px', background: '#26282c', borderBottom: '1px solid #3f4147',
};
const breadcrumbStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 6, fontSize: 12,
};
const crumbLinkStyle: React.CSSProperties = {
  color: '#9ca0a8', textDecoration: 'none', cursor: 'pointer',
};
const crumbCurrentStyle: React.CSSProperties = {
  color: '#d7d9dc', fontWeight: 600,
};
const crumbSepStyle: React.CSSProperties = { color: '#5f626a' };
const headerActionsStyle: React.CSSProperties = {
  display: 'flex', gap: 8,
};
const secondaryBtnStyle: React.CSSProperties = {
  display: 'inline-block',
  textDecoration: 'none',
  background: 'transparent',
  border: '1px solid #3f4147',
  color: '#9ca0a8',
  fontSize: 11,
  padding: '4px 10px',
  borderRadius: 3,
  textTransform: 'uppercase',
  letterSpacing: 0.4,
  fontWeight: 600,
};
const titleBlockStyle: React.CSSProperties = {
  maxWidth: 1200, margin: '0 auto', padding: '22px 24px 14px',
};
const titleStyle: React.CSSProperties = {
  fontSize: 22, fontWeight: 600, color: '#d7d9dc', margin: 0,
};
const titleHighlightStyle: React.CSSProperties = {
  color: '#5b8def',
};
const subtitleStyle: React.CSSProperties = {
  fontSize: 12, color: '#9ca0a8', marginTop: 6,
};
const metaRowStyle: React.CSSProperties = {
  display: 'flex', flexWrap: 'wrap', gap: 14, marginTop: 16,
};
const metaStyle: React.CSSProperties = {
  display: 'inline-flex', flexDirection: 'column', color: 'inherit',
};
const metaKeyStyle: React.CSSProperties = {
  fontSize: 10, color: '#7e8189', textTransform: 'uppercase', letterSpacing: 0.5,
};
const metaValueStyle: React.CSSProperties = {
  fontSize: 14, color: '#d7d9dc', fontWeight: 600,
};
const gridStyle: React.CSSProperties = {
  display: 'grid', maxWidth: 1200, margin: '4px auto 24px', padding: '0 24px',
  gridTemplateColumns: 'repeat(auto-fit, minmax(360px, 1fr))',
  gap: 14,
};
const cardStyle: React.CSSProperties = {
  background: '#26282c', border: '1px solid #3f4147', borderRadius: 4, overflow: 'hidden',
};
const cardHeaderStyle: React.CSSProperties = {
  fontSize: 10, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.4,
  color: '#9ca0a8', padding: '8px 12px', background: '#1e1f22',
  borderBottom: '1px solid #3f4147',
};
const cardBodyStyle: React.CSSProperties = { padding: 12 };
const listStyle: React.CSSProperties = {
  margin: 0, padding: 0, listStyle: 'none',
  fontFamily: 'ui-monospace, monospace', fontSize: 11,
};
const liStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 6, padding: '6px 6px',
  borderBottom: '1px solid #2f3136',
};
const rowLinkStyle: React.CSSProperties = {
  textDecoration: 'none', color: 'inherit', display: 'block', cursor: 'pointer',
};
const codeStyle: React.CSSProperties = {
  background: '#1e1f22', padding: '2px 6px', borderRadius: 3, color: '#d7d9dc',
  whiteSpace: 'nowrap',
};
const countNumStyle: React.CSSProperties = {
  minWidth: 40, textAlign: 'right', color: '#d7d9dc',
};
const locStyle: React.CSSProperties = {
  marginLeft: 'auto', color: '#7e8189', fontSize: 10,
};
const miniBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '1px 6px', borderRadius: 3, color: '#0a0a14',
  fontSize: 9, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.3,
  textAlign: 'center',
};
const kindBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '2px 7px', borderRadius: 3,
  background: '#3f4147', color: '#d7d9dc', fontSize: 10, fontWeight: 600,
  textTransform: 'uppercase', letterSpacing: 0.3,
};
const barOuterStyle: React.CSSProperties = {
  display: 'inline-block', width: '100%', height: 8, background: '#1e1f22',
  borderRadius: 2, position: 'relative', overflow: 'hidden',
};
const barFillStyle: React.CSSProperties = {
  display: 'block', height: '100%', background: '#5b8def',
};
const gaugeOuterStyle: React.CSSProperties = {
  flex: 1, height: 22, background: '#1e1f22', borderRadius: 2,
  border: '1px solid #3f4147', overflow: 'hidden',
};
const gaugeFillStyle: React.CSSProperties = {
  height: '100%',
  background: 'linear-gradient(90deg, #e26d6d 0%, #e0a458 50%, #48a999 100%)',
};
