import { useMemo } from 'react';
import { Link, useParams } from 'react-router-dom';
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
} from '../types';
import type { Category, CallTreeNode } from '../types';
import { findNodeById, flattenFindings, useReport } from './useReport';

/**
 * Deep-link target for a single CallTreeNode at
 * `/scan/:fixtureKey/node/:nodeId`. The node id is
 * `file::class::name` — the same id the JSON emits — URL-encoded.
 */
export function NodeDetailPage() {
  const { nodeId } = useParams<{ nodeId: string }>();
  const { report, fixture, fixtureKey, error, loading } = useReport();
  const node = useMemo(() => (nodeId ? findNodeById(report, nodeId) : null), [report, nodeId]);

  // Build the same flat finding list ScanReportPage uses, then filter to
  // this node so each finding link still points at the canonical
  // /finding/:idx URL.
  const flatFindings = useMemo(() => flattenFindings(report), [report]);
  const myFindings = useMemo(
    () => (node ? flatFindings.filter((f) => f.node.id === node.id) : []),
    [flatFindings, node],
  );

  if (!fixtureKey) {
    return <Shell title="node"><Err msg="no fixture key in URL" /></Shell>;
  }
  if (loading) {
    return <Shell title="node" fixtureKey={fixtureKey}><Loading /></Shell>;
  }
  if (error || !report || !fixture) {
    return <Shell title="node" fixtureKey={fixtureKey}><Err msg={error ?? 'no report'} /></Shell>;
  }
  if (!node) {
    return (
      <Shell title="node" fixtureKey={fixtureKey} fixtureLabel={fixture.label}>
        <Err msg={`node "${nodeId}" not found in this scan`} />
        <p style={{ marginTop: 12 }}>
          <Link to={`/scan/${fixtureKey}/report`} style={primaryLinkStyle}>
            ← back to scan report
          </Link>
        </p>
      </Shell>
    );
  }

  return (
    <Shell
      title={node.name}
      fixtureKey={fixtureKey}
      fixtureLabel={fixture.label}
    >
      <div style={titleRowStyle}>
        <h1 style={titleStyle}>
          {node.parent_class && <span style={{ color: '#7e8189' }}>{node.parent_class}.</span>}
          {node.name}
        </h1>
        <span style={kindBadgeStyle}>{node.kind}</span>
        {node.is_async && <span style={{ ...kindBadgeStyle, marginLeft: 6 }}>async</span>}
        {node.is_recursive && <span style={{ ...miniBadgeStyle, marginLeft: 6, background: SEVERITY_COLORS.medium }}>recursive</span>}
      </div>
      <div style={fileLineStyle}>{node.file}:{node.line}</div>

      <div style={metricsRowStyle}>
        <Metric k="complexity" v={String(node.complexity)} />
        <Metric k="loc" v={String(node.loc)} />
        <Metric k="nesting" v={String(node.nesting_depth)} />
        <Metric k="params" v={String(node.parameter_count)} />
        <Metric k="callers" v={String(node.callers_count)} />
        <Metric k="callees" v={String(node.callees_count)} />
        <Metric k="subtree" v={String(node.subtree_size)} />
        <Metric k="pagerank" v={node.pagerank.toFixed(4)} />
        <Metric k="reach" v={`${node.percent_total.toFixed(1)}%`} />
      </div>

      {myFindings.length > 0 && (
        <Section title={`findings on this node · ${myFindings.length}`}>
          <ul style={listStyle}>
            {myFindings.map((f) => (
              <Link
                key={f.idx}
                to={`/scan/${fixtureKey}/finding/${f.idx}`}
                style={rowLinkStyle}
                title="Open finding detail page"
              >
                <li style={liStyle}>
                  <span style={{ ...badgeSmStyle, background: SEVERITY_COLORS[f.finding.severity] }}>
                    {f.finding.severity}
                  </span>
                  <span style={{ ...kindBadgeStyle, marginLeft: 8 }}>
                    {FINDING_KIND_LABEL[f.finding.kind] ?? f.finding.kind}
                  </span>
                  <span style={{ marginLeft: 10, color: '#d7d9dc' }}>{f.finding.message}</span>
                  <span style={locStyle}>:{f.finding.line}</span>
                </li>
              </Link>
            ))}
          </ul>
        </Section>
      )}

      {node.external_calls.length > 0 && (
        <Section title={`external calls · ${node.external_calls.length}`}>
          <ul style={listStyle}>
            {node.external_calls.map((e, j) => (
              <li key={j} style={liStyle}>
                <span
                  style={{
                    ...miniBadgeStyle,
                    background: CATEGORY_COLORS[e.category as Category],
                    minWidth: 60,
                  }}
                >
                  {e.category}
                </span>
                <code style={{ ...codeStyle, marginLeft: 8 }}>
                  {e.receiver ? <span style={{ color: '#7e8189' }}>{e.receiver}.</span> : null}
                  {e.name}
                </code>
                {e.in_loop && (
                  <span style={{ ...miniBadgeStyle, marginLeft: 6, background: SEVERITY_COLORS.high }}>
                    in-loop
                  </span>
                )}
                {e.in_await && (
                  <span style={{ ...miniBadgeStyle, marginLeft: 6, background: SEVERITY_COLORS.low }}>
                    in-await
                  </span>
                )}
                <span style={locStyle}>:{e.line}</span>
              </li>
            ))}
          </ul>
        </Section>
      )}

      {node.callers.length > 0 && (
        <Section title={`callers · ${node.callers.length}`}>
          <ul style={listStyle}>
            {node.callers.slice(0, 30).map((c) => (
              <Link
                key={c.id}
                to={`/scan/${fixtureKey}/node/${encodeURIComponent(c.id)}`}
                style={rowLinkStyle}
                title="Open caller's node detail page"
              >
                <li style={liStyle}>
                  <code style={codeStyle}>
                    {c.parent_class && <span style={{ color: '#7e8189' }}>{c.parent_class}.</span>}
                    {c.name}
                  </code>
                  <span style={locStyle}>{c.file}:{c.line}</span>
                </li>
              </Link>
            ))}
          </ul>
        </Section>
      )}

      <Section title="navigate">
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <Link to={`/scan/${fixtureKey}/report`} style={primaryLinkStyle}>
            ← Scan report
          </Link>
          <Link to={`/scan/${fixtureKey}`} style={secondaryLinkStyle}>
            ← Dashboard
          </Link>
          <ChildLinksRow node={node} fixtureKey={fixtureKey} />
        </div>
      </Section>
    </Shell>
  );
}

function ChildLinksRow({ node, fixtureKey }: { node: CallTreeNode; fixtureKey: string }) {
  if (node.children.length === 0) return null;
  return (
    <>
      {node.children.slice(0, 5).map((c) => (
        <Link
          key={c.id}
          to={`/scan/${fixtureKey}/node/${encodeURIComponent(c.id)}`}
          style={secondaryLinkStyle}
          title={`Drill into ${c.name}`}
        >
          → {c.name}
        </Link>
      ))}
      {node.children.length > 5 && (
        <span style={{ alignSelf: 'center', color: '#7e8189', fontSize: 11 }}>
          + {node.children.length - 5} more
        </span>
      )}
    </>
  );
}

// ─── Shared subcomponents ───────────────────────────────────────────────

function Shell({
  title, fixtureKey, fixtureLabel, children,
}: {
  title: string;
  fixtureKey?: string;
  fixtureLabel?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={pageStyle}>
      <header style={headerBarStyle}>
        <nav style={breadcrumbStyle}>
          <Link to="/" style={crumbLinkStyle}>scans</Link>
          {fixtureKey && (
            <>
              <span style={crumbSepStyle}>/</span>
              <Link to={`/scan/${fixtureKey}`} style={crumbLinkStyle}>
                {fixtureLabel ?? fixtureKey}
              </Link>
              <span style={crumbSepStyle}>/</span>
              <Link to={`/scan/${fixtureKey}/report`} style={crumbLinkStyle}>report</Link>
            </>
          )}
          <span style={crumbSepStyle}>/</span>
          <span style={crumbCurrentStyle}>{title}</span>
        </nav>
      </header>
      <main style={mainStyle}>{children}</main>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ marginTop: 22 }}>
      <h2 style={sectionTitleStyle}>{title}</h2>
      {children}
    </section>
  );
}

function Metric({ k, v }: { k: string; v: string }) {
  return (
    <div style={metricStyle}>
      <span style={metricKeyStyle}>{k}</span>
      <span style={metricValueStyle}>{v}</span>
    </div>
  );
}

function Loading() {
  return <div style={{ color: '#7e8189' }}>loading…</div>;
}
function Err({ msg }: { msg: string }) {
  return <div style={{ color: '#ff7e7e', fontFamily: 'monospace' }}>error: {msg}</div>;
}

// ─── styles ──────────────────────────────────────────────────────────────

const pageStyle: React.CSSProperties = {
  minHeight: '100vh', background: '#1e1f22', color: '#d7d9dc',
};
const headerBarStyle: React.CSSProperties = {
  padding: '10px 24px', background: '#26282c', borderBottom: '1px solid #3f4147',
};
const breadcrumbStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 6, fontSize: 12,
};
const crumbLinkStyle: React.CSSProperties = {
  color: '#9ca0a8', textDecoration: 'none', cursor: 'pointer',
};
const crumbCurrentStyle: React.CSSProperties = { color: '#d7d9dc', fontWeight: 600 };
const crumbSepStyle: React.CSSProperties = { color: '#5f626a' };
const mainStyle: React.CSSProperties = {
  maxWidth: 980, margin: '0 auto', padding: '22px 24px 40px',
};
const titleRowStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap',
};
const titleStyle: React.CSSProperties = {
  fontSize: 22, fontWeight: 600, color: '#d7d9dc', margin: 0,
};
const fileLineStyle: React.CSSProperties = {
  fontSize: 12, color: '#7e8189', marginTop: 6, fontFamily: 'ui-monospace, monospace',
};
const metricsRowStyle: React.CSSProperties = {
  display: 'flex', flexWrap: 'wrap', gap: 18, marginTop: 18,
};
const metricStyle: React.CSSProperties = {
  display: 'inline-flex', flexDirection: 'column',
};
const metricKeyStyle: React.CSSProperties = {
  fontSize: 10, color: '#7e8189', textTransform: 'uppercase', letterSpacing: 0.5,
};
const metricValueStyle: React.CSSProperties = {
  fontSize: 14, color: '#d7d9dc', fontWeight: 600,
};
const sectionTitleStyle: React.CSSProperties = {
  fontSize: 11, color: '#9ca0a8', textTransform: 'uppercase', letterSpacing: 0.4,
  marginBottom: 8, fontWeight: 700,
};
const listStyle: React.CSSProperties = { margin: 0, padding: 0, listStyle: 'none' };
const liStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 6, padding: '6px 6px',
  borderBottom: '1px solid #2f3136', fontSize: 12, fontFamily: 'ui-monospace, monospace',
};
const rowLinkStyle: React.CSSProperties = {
  textDecoration: 'none', color: 'inherit', display: 'block', cursor: 'pointer',
};
const codeStyle: React.CSSProperties = {
  background: '#26282c', padding: '2px 6px', borderRadius: 3, color: '#d7d9dc',
};
const locStyle: React.CSSProperties = {
  marginLeft: 'auto', color: '#7e8189', fontSize: 10,
};
const badgeSmStyle: React.CSSProperties = {
  display: 'inline-block', padding: '1px 7px', borderRadius: 3, color: '#0a0a14',
  fontSize: 10, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.3,
};
const miniBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '1px 6px', borderRadius: 3, color: '#0a0a14',
  fontSize: 9, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.3,
};
const kindBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '3px 9px', borderRadius: 3,
  background: '#3f4147', color: '#d7d9dc', fontSize: 11, fontWeight: 600,
  textTransform: 'uppercase', letterSpacing: 0.3,
};
const primaryLinkStyle: React.CSSProperties = {
  display: 'inline-block', textDecoration: 'none',
  background: 'transparent', border: '1px solid #5b8def',
  color: '#5b8def', fontSize: 11, padding: '5px 12px', borderRadius: 3,
  textTransform: 'uppercase', letterSpacing: 0.4, fontWeight: 600,
};
const secondaryLinkStyle: React.CSSProperties = {
  display: 'inline-block', textDecoration: 'none',
  background: 'transparent', border: '1px solid #3f4147',
  color: '#9ca0a8', fontSize: 11, padding: '5px 12px', borderRadius: 3,
  textTransform: 'uppercase', letterSpacing: 0.4, fontWeight: 600,
};
