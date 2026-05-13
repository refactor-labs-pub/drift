import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import {
  CATEGORY_COLORS,
  ENTRY_KIND_LABEL,
  EFFORT_LABEL,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
  buildEntryPointFilterOptions,
  entryFamily,
  filterEntriesByDeclSource,
} from '../types';
import type {
  CallTreeNode,
  Category,
  EntryDecl,
  EntryFamily,
  Effort,
  FindingKind,
  ImmediateFix,
  RefactorCandidate,
  RootOverview,
  Severity,
} from '../types';
import { filterAndSortEntries, formatCapturedAt } from '../ScanReport';
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

  // ── Rule-of-hooks: every hook below MUST run before any conditional
  // return. `flattenFindings` returns [] for a null report, so the
  // memoized values are well-defined even while `useReport` is still
  // loading — that's what lets us call them up here.
  const allFindings = useMemo(() => flattenFindings(report), [report]);
  const sevCounts: Record<Severity, number> = useMemo(() => {
    const counts: Record<Severity, number> = { high: 0, medium: 0, low: 0 };
    for (const t of allFindings) counts[t.finding.severity]++;
    return counts;
  }, [allFindings]);
  const healthScore = useMemo(() => {
    const s = 10 - sevCounts.high * 0.5 - sevCounts.medium * 0.2 - sevCounts.low * 0.05;
    return Math.max(0, s);
  }, [sevCounts]);

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

  const cats = Object.entries(summary.categories ?? {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);

  const langBreakdown = summary.language_breakdown ?? [];
  const entries = report.entries;
  const rootsOverview = summary.roots_overview ?? [];
  const immediateFixes = summary.immediate_fixes ?? [];
  const refactorCandidates = summary.refactor_candidates ?? [];
  const entryDecls = summary.entry_declarations ?? [];

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
          {report.generator?.captured_at && (
            <Meta
              k="scanned"
              v={formatCapturedAt(report.generator.captured_at)}
              titleAttr={report.generator.captured_at}
            />
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
        <EntryPointsCard
          entries={entries}
          entryDecls={entryDecls}
          fixtureKey={fixtureKey}
        />
        <EntryDeclarationsCard
          entryDecls={entryDecls}
          callTreeEntries={entries}
          fixtureKey={fixtureKey}
        />
      </main>

      {/* Immediate Fixes: high-severity × trivial/small-effort findings.
          SonarQube-style "quick wins" lane. Each row is a `<Link>` to
          the corresponding finding detail page. */}
      {immediateFixes.length > 0 && (
        <section style={fullWidthSectionStyle}>
          <ImmediateFixesCard
            fixes={immediateFixes}
            allFindings={allFindings}
            fixtureKey={fixtureKey}
          />
        </section>
      )}

      {/* Refactor Candidates: nodes with finding clusters, Large-effort
          findings, or god-function bodies. Each card links to that
          node's detail page. */}
      {refactorCandidates.length > 0 && (
        <section style={fullWidthSectionStyle}>
          <RefactorCandidatesCard candidates={refactorCandidates} fixtureKey={fixtureKey} />
        </section>
      )}

      {/* Initial Roots: per-entry-point rollup (subtree share, categories,
          findings-by-severity, first callees, callers). Inspired by
          pprof's `top -cum` at root granularity. Each row is a `<Link>`
          to the node detail page. Falls back to nothing when older
          fixtures omit `roots_overview`. */}
      {rootsOverview.length > 0 && (
        <section style={fullWidthSectionStyle}>
          <InitialRootsCard roots={rootsOverview} fixtureKey={fixtureKey} />
        </section>
      )}
    </div>
  );
}

// ─── Immediate Fixes card ──────────────────────────────────────────────

function ImmediateFixesCard({
  fixes, allFindings, fixtureKey,
}: {
  fixes: ImmediateFix[];
  allFindings: { idx: number; finding: { kind: FindingKind; line: number }; node: { id: string } }[];
  fixtureKey: string;
}) {
  // Resolve each ImmediateFix back to its FindingDetailPage idx so the
  // row links to the same canonical /finding/:idx URL the rest of the
  // report uses.
  const idxOf = (fx: ImmediateFix): number | null => {
    const hit = allFindings.find(
      (f) => f.finding.kind === fx.kind && f.finding.line === fx.line && f.node.id === fx.node_id,
    );
    return hit ? hit.idx : null;
  };
  return (
    <Card
      title={`immediate fixes · ${fixes.length}`}
      hint="High-severity findings with trivial/small effort. The PR-sized fixes you can do today."
    >
      <ul style={listStyle}>
        {fixes.map((fx, i) => {
          const idx = idxOf(fx);
          const row = (
            <li style={fixRowStyle}>
              <span style={{ ...miniBadgeStyle, background: SEVERITY_COLORS[fx.severity], minWidth: 56 }}>
                {fx.severity}
              </span>
              <span style={{ ...effortPillStyle, background: effortBg(fx.effort), marginLeft: 6 }}>
                {EFFORT_LABEL[fx.effort]}
              </span>
              <span style={{ ...kindBadgeStyle, marginLeft: 8 }}>
                {FINDING_KIND_LABEL[fx.kind] ?? fx.kind}
              </span>
              <code style={{ ...codeStyle, marginLeft: 10 }}>
                {fx.parent_class && <span style={{ color: '#7e8189' }}>{fx.parent_class}.</span>}
                {fx.name}
              </code>
              <span style={locStyle}>{fx.file}:{fx.line}</span>
              <div style={fixMessageStyle}>{fx.message}</div>
            </li>
          );
          return idx !== null ? (
            <Link
              key={`${fx.node_id}:${fx.kind}:${fx.line}:${i}`}
              to={`/scan/${fixtureKey}/finding/${idx}`}
              style={rowLinkStyle}
              title="Open finding detail page"
            >
              {row}
            </Link>
          ) : (
            <div key={i}>{row}</div>
          );
        })}
      </ul>
    </Card>
  );
}

// ─── Refactor Candidates card ──────────────────────────────────────────

function RefactorCandidatesCard({
  candidates, fixtureKey,
}: { candidates: RefactorCandidate[]; fixtureKey: string }) {
  return (
    <Card
      title={`refactor candidates · ${candidates.length}`}
      hint="Symbols that need a fuller rewrite: multiple findings, large effort, or god-function bodies."
    >
      <ul style={listStyle}>
        {candidates.map((c) => (
          <Link
            key={c.node_id}
            to={`/scan/${fixtureKey}/node/${encodeURIComponent(c.node_id)}`}
            style={rowLinkStyle}
            title={`Open ${c.name} detail page`}
          >
            <li style={refactorRowStyle}>
              <div style={refactorHeaderStyle}>
                <span style={{ ...miniBadgeStyle, background: SEVERITY_COLORS[c.worst_severity], minWidth: 56 }}>
                  {c.worst_severity}
                </span>
                <span style={{ ...effortPillStyle, background: effortBg(c.max_effort), marginLeft: 6 }}>
                  {EFFORT_LABEL[c.max_effort]}
                </span>
                <code style={{ ...codeStyle, marginLeft: 8 }}>
                  {c.parent_class && <span style={{ color: '#7e8189' }}>{c.parent_class}.</span>}
                  {c.name}
                </code>
                <span style={locStyle}>{c.file}:{c.line}</span>
              </div>
              <div style={refactorWhyStyle}>{c.why}</div>
              <div style={refactorMetricsRowStyle}>
                <Metric k="findings" v={String(c.findings_count)} />
                <Metric k="complexity" v={String(c.complexity)} />
                <Metric k="loc" v={String(c.loc)} />
                <Metric k="reach" v={`${c.percent_total.toFixed(1)}%`} />
                <span style={{ display: 'flex', gap: 4, flexWrap: 'wrap', alignItems: 'center' }}>
                  {c.kinds.map((k) => (
                    <span key={k} style={kindBadgeStyle}>
                      {FINDING_KIND_LABEL[k] ?? k}
                    </span>
                  ))}
                </span>
              </div>
            </li>
          </Link>
        ))}
      </ul>
    </Card>
  );
}

/// Color stops for the effort pill — Trivial/Small are "go" green/blue,
/// Medium amber, Large red. Reuses existing category palette so no new
/// colors get introduced.
function effortBg(e: Effort): string {
  switch (e) {
    case 'trivial': return '#48a999';
    case 'small':   return '#5b8def';
    case 'medium':  return '#e0a458';
    case 'large':   return '#e26d6d';
  }
}

function Metric({ k, v }: { k: string; v: string }) {
  return (
    <span style={metricInlineStyle}>
      <span style={metricKeyStyle}>{k}</span>
      <span style={metricValueStyle}>{v}</span>
    </span>
  );
}

// ─── Initial Roots card ─────────────────────────────────────────────────

function InitialRootsCard({
  roots, fixtureKey,
}: { roots: RootOverview[]; fixtureKey: string }) {
  return (
    <Card title={`initial roots · ${roots.length}`} hint="One row per entry point. Click to open that root's node detail page.">
      <ul style={listStyle}>
        {roots.map((r) => (
          <RootRow key={r.node_id} root={r} fixtureKey={fixtureKey} />
        ))}
      </ul>
    </Card>
  );
}

function RootRow({ root, fixtureKey }: { root: RootOverview; fixtureKey: string }) {
  const high = root.findings_by_severity?.high ?? 0;
  const medium = root.findings_by_severity?.medium ?? 0;
  const low = root.findings_by_severity?.low ?? 0;
  const cats = Object.entries(root.categories_reached ?? {})
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
  const callees = root.first_callees ?? [];
  const callers = root.callers ?? [];

  return (
    <li style={rootRowStyle}>
      <Link
        to={`/scan/${fixtureKey}/node/${encodeURIComponent(root.node_id)}`}
        style={rootHeaderLinkStyle}
        title={`Open ${root.name} detail page`}
      >
        <span style={rootKindStyle}>{root.kind}</span>
        <code style={rootNameStyle}>
          {root.parent_class && <span style={{ color: '#7e8189' }}>{root.parent_class}.</span>}
          {root.name}
        </code>
        <span style={rootLocStyle}>{root.file}:{root.line}</span>
      </Link>

      <div style={rootReachRowStyle}>
        <span style={{ ...rootLabelStyle }}>reach</span>
        <span style={rootReachBarOuterStyle}>
          <span style={{ ...rootReachBarFillStyle, width: `${Math.min(100, root.percent_of_all_roots)}%` }} />
        </span>
        <span style={rootReachValueStyle}>
          {root.subtree_size}
          <span style={{ color: '#7e8189', marginLeft: 4 }}>· {root.percent_of_all_roots.toFixed(1)}%</span>
        </span>
      </div>

      <div style={rootChipsRowStyle}>
        <span style={rootLabelStyle}>findings</span>
        {root.findings_total === 0 ? (
          <span style={{ ...rootDimStyle, marginLeft: 6 }}>clean</span>
        ) : (
          <>
            <span style={rootFindingsTotalStyle}>{root.findings_total}</span>
            {high > 0 && (
              <Link
                to={`/scan/${fixtureKey}/node/${encodeURIComponent(root.node_id)}`}
                title="See high-severity findings on this root"
                style={{ textDecoration: 'none' }}
              >
                <span style={{ ...sevPillStyle, background: SEVERITY_COLORS.high }}>{high} high</span>
              </Link>
            )}
            {medium > 0 && (
              <span style={{ ...sevPillStyle, background: SEVERITY_COLORS.medium }}>{medium} med</span>
            )}
            {low > 0 && (
              <span style={{ ...sevPillStyle, background: SEVERITY_COLORS.low }}>{low} low</span>
            )}
          </>
        )}
      </div>

      {cats.length > 0 && (
        <div style={rootChipsRowStyle}>
          <span style={rootLabelStyle}>reaches</span>
          {cats.slice(0, 6).map(([cat, n]) => (
            <span
              key={cat}
              style={{ ...catChipStyle, background: CATEGORY_COLORS[cat as Category] }}
              title={`${n} ${cat} call(s) reachable from this root`}
            >
              {cat} <strong style={{ marginLeft: 3 }}>{n}</strong>
            </span>
          ))}
        </div>
      )}

      {callees.length > 0 && (
        <div style={rootChipsRowStyle}>
          <span style={rootLabelStyle}>first calls</span>
          {callees.map((c) => (
            <Link
              key={c.node_id}
              to={`/scan/${fixtureKey}/node/${encodeURIComponent(c.node_id)}`}
              style={{ textDecoration: 'none' }}
              title={`Drill into ${c.name} (reach ${c.subtree_size})`}
            >
              <span style={calleeChipStyle}>
                {c.parent_class ? <span style={{ color: '#7e8189' }}>{c.parent_class}.</span> : null}
                {c.name}
                <span style={{ color: '#7e8189', marginLeft: 4 }}>·{c.subtree_size}</span>
              </span>
            </Link>
          ))}
        </div>
      )}

      {callers.length > 0 && (
        <div style={rootChipsRowStyle}>
          <span style={rootLabelStyle}>callers</span>
          {callers.slice(0, 4).map((c) => (
            <Link
              key={c.node_id}
              to={`/scan/${fixtureKey}/node/${encodeURIComponent(c.node_id)}`}
              style={{ textDecoration: 'none' }}
              title={`Jump to caller ${c.name}`}
            >
              <span style={callerChipStyle}>
                {c.parent_class ? <span style={{ color: '#7e8189' }}>{c.parent_class}.</span> : null}
                {c.name}
              </span>
            </Link>
          ))}
          {callers.length > 4 && (
            <span style={{ ...rootDimStyle, marginLeft: 4 }}>+{callers.length - 4} more</span>
          )}
        </div>
      )}
    </li>
  );
}

// ─── Header subcomponents ───────────────────────────────────────────────

function Meta({
  k, v, link, titleAttr,
}: { k: string; v: string; link?: string; titleAttr?: string }) {
  const body = (
    <>
      <span style={metaKeyStyle}>{k}</span>
      <span style={metaValueStyle}>{v}</span>
    </>
  );
  if (link) {
    return (
      <Link to={link} style={{ ...metaStyle, textDecoration: 'none' }} title={titleAttr ?? `Open ${k}`}>
        {body}
      </Link>
    );
  }
  return <div style={metaStyle} title={titleAttr}>{body}</div>;
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
// Same "filter by entry source" dropdown the in-tab card carries (see
// `EntryPointsCard` in `../ScanReport.tsx`). Options are dynamic — only
// declaration kinds that resolved to at least one of the entries in this
// scan are surfaced.

function EntryPointsCard({
  entries, entryDecls, fixtureKey,
}: { entries: CallTreeNode[]; entryDecls: EntryDecl[]; fixtureKey: string }) {
  const filterOptions = useMemo(
    () => buildEntryPointFilterOptions(entries, entryDecls),
    [entries, entryDecls],
  );
  const [filterValue, setFilterValue] = useState<string>('all');
  const activeFilter = useMemo(
    () => filterOptions.find((o) => o.value === filterValue) ?? filterOptions[0],
    [filterOptions, filterValue],
  );
  useEffect(() => {
    if (!filterOptions.some((o) => o.value === filterValue)) {
      setFilterValue('all');
    }
  }, [filterOptions, filterValue]);

  const filtered = useMemo(
    () =>
      activeFilter
        ? filterEntriesByDeclSource(entries, activeFilter.filter, entryDecls)
        : entries,
    [entries, activeFilter, entryDecls],
  );
  const sorted = useMemo(
    () => [...filtered].sort((a, b) => b.subtree_size - a.subtree_size),
    [filtered],
  );
  const showFilter = filterOptions.length > 1;

  return (
    <Card
      title={`entry points · ${filtered.length}${
        filtered.length !== entries.length ? ` / ${entries.length}` : ''
      }`}
      hint="Use the source filter to narrow to entries launched by a specific manifest (Dockerfile, package.json, pyproject, etc.). Options are derived from this scan; categories with no matched root are hidden."
    >
      {entries.length === 0 ? (
        <Empty msg="—" />
      ) : (
        <div style={scrollListStyle}>
          {showFilter && (
            <div style={pageEntryPointFilterRowStyle}>
              <label style={pageEntryPointFilterLabelStyle} htmlFor="page-entry-point-filter">
                filter by source
              </label>
              <select
                id="page-entry-point-filter"
                value={filterValue}
                onChange={(e) => setFilterValue(e.target.value)}
                style={pageEntryPointFilterSelectStyle}
                aria-label="Filter entry points by source manifest"
              >
                {filterOptions.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </div>
          )}
          {sorted.length === 0 ? (
            <Empty msg="no entries match the selected source filter" />
          ) : (
            <ul style={listStyle}>
              {sorted.map((e) => (
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
        </div>
      )}
    </Card>
  );
}

const pageEntryPointFilterRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 6,
  padding: '6px 4px',
  borderBottom: '1px solid #2f3136',
};
const pageEntryPointFilterLabelStyle: React.CSSProperties = {
  fontSize: 10,
  color: '#7e8189',
  textTransform: 'uppercase',
  letterSpacing: 0.5,
  flexShrink: 0,
};
const pageEntryPointFilterSelectStyle: React.CSSProperties = {
  flex: 1,
  minWidth: 0,
  background: '#1e1f22',
  color: '#d7d9dc',
  border: '1px solid #3f4147',
  borderRadius: 3,
  padding: '3px 6px',
  fontSize: 11,
  fontFamily: 'inherit',
  cursor: 'pointer',
};

// ─── Docker entry points ────────────────────────────────────────────────
// Mirror of the in-tab `EntryDeclarationsCard` in `../ScanReport.tsx`,
// but using react-router <Link>s so every clickable row is a deep URL
// on this dedicated page (same pattern as the surrounding
// `EntryPointsCard`). Filter logic is shared via `filterAndSortEntries`
// so the two surfaces stay in lockstep.
function EntryDeclarationsCard({
  entryDecls, callTreeEntries, fixtureKey,
}: {
  entryDecls: EntryDecl[];
  callTreeEntries: CallTreeNode[];
  fixtureKey: string;
}) {
  const knownIds = useMemo(
    () => new Set(callTreeEntries.map((e) => e.id)),
    [callTreeEntries],
  );
  const [query, setQuery] = useState('');
  const [familyFilter, setFamilyFilter] = useState<EntryFamily | null>(null);

  const filtered = useMemo(
    () => filterAndSortEntries(entryDecls, query, familyFilter),
    [entryDecls, query, familyFilter],
  );
  const counts = useMemo(() => {
    let container = 0;
    let manifest = 0;
    for (const e of entryDecls) {
      if (entryFamily(e.kind) === 'container') container++;
      else manifest++;
    }
    return { container, manifest };
  }, [entryDecls]);

  return (
    <Card title={`entry declarations · ${entryDecls.length}`}
      hint="Container-deployment declarations AND language-manifest entries (package.json scripts/bin/main, pyproject.toml scripts, deno tasks, Cargo [[bin]]). Rows with a resolved symbol link into that root's node-detail page.">
      {entryDecls.length === 0 ? (
        <Empty msg="no Dockerfile / compose / package manifest entries detected" />
      ) : (
        <div style={scrollListStyle}>
          <div style={pageEntryFilterRowStyle}>
            <input
              type="search"
              placeholder="filter by argv / service / symbol / file…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              style={pageEntrySearchInputStyle}
              aria-label="Filter entry declarations"
            />
            <PageFilterChip active={familyFilter === null} onClick={() => setFamilyFilter(null)}>
              all · {entryDecls.length}
            </PageFilterChip>
            <PageFilterChip
              active={familyFilter === 'container'}
              onClick={() => setFamilyFilter('container')}
            >
              container · {counts.container}
            </PageFilterChip>
            <PageFilterChip
              active={familyFilter === 'manifest'}
              onClick={() => setFamilyFilter('manifest')}
            >
              manifest · {counts.manifest}
            </PageFilterChip>
          </div>
          {filtered.length === 0 ? (
            <Empty msg="no entries match the current filter" />
          ) : (
            <ul style={listStyle}>
              {filtered.map((e, i) => {
                const canJump = !!(e.matched && knownIds.has(e.matched.symbol_id));
                const row = (
                  <li
                    style={liStyle}
                    title={
                      e.matched
                        ? `Matched (${e.matched.confidence}) → ${e.matched.symbol_name} · ${e.matched.evidence}`
                        : 'No in-graph symbol resolved — opaque command (e.g. `java -jar`, `./bin/server`, `pytest`)'
                    }
                  >
                    <span style={entryKindBadgeStyle(e.kind)}>
                      {ENTRY_KIND_LABEL[e.kind]}
                      {e.service ? `:${e.service}` : ''}
                    </span>
                    <code style={entryRawStyle} title={e.raw}>
                      {truncateMiddle(e.raw, 60)}
                    </code>
                    <span style={entrySpacerStyle}>
                      <span style={entryConfBadgeStyle(e.matched?.confidence)}>
                        {e.matched ? e.matched.confidence : 'unmatched'}
                      </span>
                      {e.matched && (
                        <span style={entrySymbolStyle(canJump)}>
                          → {e.matched.symbol_name}
                        </span>
                      )}
                    </span>
                    <span style={locStyle}>{e.file}:{e.line}</span>
                  </li>
                );
                const key = `${e.file}:${e.line}:${i}`;
                return canJump ? (
                  <Link
                    key={key}
                    to={`/scan/${fixtureKey}/node/${encodeURIComponent(e.matched!.symbol_id)}`}
                    style={rowLinkStyle}
                  >
                    {row}
                  </Link>
                ) : (
                  <div key={key}>{row}</div>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </Card>
  );
}

function PageFilterChip({
  active, onClick, children,
}: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        ...pageEntryFilterChipStyle,
        background: active ? '#3b3f44' : 'transparent',
        color: active ? '#d7d9dc' : '#9ca0a8',
        borderColor: active ? '#5b8def' : '#3f4147',
      }}
    >
      {children}
    </button>
  );
}

const pageEntryFilterRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 6,
  padding: '6px 4px',
  borderBottom: '1px solid #2f3136',
  flexWrap: 'wrap',
};
const pageEntrySearchInputStyle: React.CSSProperties = {
  flex: '1 1 200px',
  minWidth: 0,
  background: '#1e1f22',
  color: '#d7d9dc',
  border: '1px solid #3f4147',
  borderRadius: 3,
  padding: '4px 8px',
  fontSize: 11,
  fontFamily: 'inherit',
  outline: 'none',
};
const pageEntryFilterChipStyle: React.CSSProperties = {
  fontSize: 10,
  padding: '2px 8px',
  borderRadius: 10,
  border: '1px solid #3f4147',
  cursor: 'pointer',
  fontFamily: 'inherit',
  letterSpacing: 0.3,
  flexShrink: 0,
};

function truncateMiddle(s: string, max: number): string {
  if (s.length <= max) return s;
  const half = Math.floor((max - 1) / 2);
  return `${s.slice(0, half)}…${s.slice(s.length - half)}`;
}

function entryKindBadgeStyle(kind: EntryDecl['kind']): React.CSSProperties {
  const color =
    kind === 'dockerfile_cmd' || kind === 'dockerfile_entrypoint'
      ? CATEGORY_COLORS.network
      : CATEGORY_COLORS.cache;
  return {
    fontSize: 9,
    color,
    border: `1px solid ${color}`,
    borderRadius: 2,
    padding: '1px 5px',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    flexShrink: 0,
    whiteSpace: 'nowrap',
  };
}

function entryConfBadgeStyle(c?: 'exact' | 'likely' | 'unmatched'): React.CSSProperties {
  const color =
    c === 'exact'
      ? SEVERITY_COLORS.high
      : c === 'likely'
        ? SEVERITY_COLORS.medium
        : SEVERITY_COLORS.low;
  return {
    fontSize: 9,
    color,
    border: `1px solid ${color}`,
    borderRadius: 2,
    padding: '0 4px',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    flexShrink: 0,
  };
}

function entrySymbolStyle(canJump: boolean): React.CSSProperties {
  return {
    color: canJump ? '#d7d9dc' : '#7e8189',
    fontSize: 11,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
    textDecoration: canJump ? 'underline dotted' : 'none',
  };
}

const entryRawStyle: React.CSSProperties = {
  background: '#1e1f22',
  padding: '2px 6px',
  borderRadius: 3,
  color: '#d7d9dc',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  flexShrink: 1,
  minWidth: 0,
  maxWidth: '40%',
};

const entrySpacerStyle: React.CSSProperties = {
  marginLeft: 'auto',
  display: 'flex',
  alignItems: 'center',
  gap: 6,
};

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
// Scrollable list container — bounds card height when the entry count is
// large (e.g. `make scan-roots` output with 50+ roots).
const scrollListStyle: React.CSSProperties = {
  maxHeight: 280, overflowY: 'auto',
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

// ─── Initial Roots styles ───────────────────────────────────────────────

const fullWidthSectionStyle: React.CSSProperties = {
  maxWidth: 1200, margin: '0 auto 24px', padding: '0 24px',
};
const rootRowStyle: React.CSSProperties = {
  display: 'block',
  background: '#26282c',
  border: '1px solid #3f4147',
  borderRadius: 4,
  padding: '10px 12px',
  marginBottom: 8,
};
const rootHeaderLinkStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'baseline',
  gap: 8,
  textDecoration: 'none',
  color: 'inherit',
};
const rootKindStyle: React.CSSProperties = {
  fontSize: 9, fontWeight: 700, color: '#7e8189',
  textTransform: 'uppercase', letterSpacing: 0.5,
  minWidth: 60,
};
const rootNameStyle: React.CSSProperties = {
  background: '#1e1f22', padding: '2px 6px', borderRadius: 3,
  color: '#d7d9dc', fontSize: 13, fontWeight: 600,
};
const rootLocStyle: React.CSSProperties = {
  marginLeft: 'auto', color: '#7e8189', fontSize: 10,
  fontFamily: 'ui-monospace, monospace',
};
const rootLabelStyle: React.CSSProperties = {
  fontSize: 9, fontWeight: 700, color: '#7e8189',
  textTransform: 'uppercase', letterSpacing: 0.4,
  minWidth: 70,
};
const rootReachRowStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 8, marginTop: 8,
};
const rootReachBarOuterStyle: React.CSSProperties = {
  flex: 1, height: 6, background: '#1e1f22', borderRadius: 2,
  position: 'relative', overflow: 'hidden',
};
const rootReachBarFillStyle: React.CSSProperties = {
  display: 'block', height: '100%', background: '#5b8def',
};
const rootReachValueStyle: React.CSSProperties = {
  fontSize: 11, color: '#d7d9dc', minWidth: 110, textAlign: 'right',
  fontFamily: 'ui-monospace, monospace',
};
const rootChipsRowStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 6, marginTop: 6, flexWrap: 'wrap',
};
const rootFindingsTotalStyle: React.CSSProperties = {
  fontSize: 11, color: '#d7d9dc', fontWeight: 600, marginRight: 4,
};
const rootDimStyle: React.CSSProperties = {
  fontSize: 11, color: '#7e8189', fontStyle: 'italic',
};
const sevPillStyle: React.CSSProperties = {
  display: 'inline-block', padding: '1px 7px', borderRadius: 3,
  color: '#0a0a14', fontSize: 9, fontWeight: 700,
  textTransform: 'uppercase', letterSpacing: 0.3,
};
const catChipStyle: React.CSSProperties = {
  display: 'inline-flex', alignItems: 'center', padding: '1px 7px',
  borderRadius: 3, color: '#0a0a14', fontSize: 9, fontWeight: 700,
  textTransform: 'uppercase', letterSpacing: 0.3,
};
const calleeChipStyle: React.CSSProperties = {
  display: 'inline-block', padding: '2px 7px', borderRadius: 3,
  background: '#1e1f22', border: '1px solid #3f4147', color: '#d7d9dc',
  fontSize: 11, fontFamily: 'ui-monospace, monospace',
};
const callerChipStyle: React.CSSProperties = {
  display: 'inline-block', padding: '2px 7px', borderRadius: 3,
  background: '#1e1f22', border: '1px dashed #3f4147', color: '#9ca0a8',
  fontSize: 11, fontFamily: 'ui-monospace, monospace',
};

// ─── Immediate Fixes + Refactor Candidates styles ─────────────────────

const fixRowStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'auto auto auto 1fr auto',
  alignItems: 'center',
  gap: 0,
  background: '#26282c',
  border: '1px solid #3f4147',
  borderRadius: 4,
  padding: '8px 12px',
  marginBottom: 6,
};
const fixMessageStyle: React.CSSProperties = {
  gridColumn: '1 / -1',
  marginTop: 6,
  color: '#d7d9dc',
  fontSize: 12,
  lineHeight: 1.4,
};
const effortPillStyle: React.CSSProperties = {
  display: 'inline-block',
  padding: '1px 7px',
  borderRadius: 3,
  color: '#0a0a14',
  fontSize: 9,
  fontWeight: 700,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
};
const refactorRowStyle: React.CSSProperties = {
  display: 'block',
  background: '#26282c',
  border: '1px solid #3f4147',
  borderRadius: 4,
  padding: '10px 12px',
  marginBottom: 8,
};
const refactorHeaderStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  flexWrap: 'wrap',
};
const refactorWhyStyle: React.CSSProperties = {
  marginTop: 6,
  fontSize: 12,
  color: '#d7d9dc',
  lineHeight: 1.4,
};
const refactorMetricsRowStyle: React.CSSProperties = {
  display: 'flex',
  flexWrap: 'wrap',
  gap: 14,
  marginTop: 8,
  alignItems: 'center',
};
const metricInlineStyle: React.CSSProperties = {
  display: 'inline-flex',
  flexDirection: 'column',
};
const metricKeyStyle: React.CSSProperties = {
  fontSize: 9,
  color: '#7e8189',
  textTransform: 'uppercase',
  letterSpacing: 0.4,
};
const metricValueStyle: React.CSSProperties = {
  fontSize: 13,
  color: '#d7d9dc',
  fontWeight: 600,
};
