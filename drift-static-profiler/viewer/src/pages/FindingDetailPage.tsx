import { useMemo } from 'react';
import { Link, useParams } from 'react-router-dom';
import {
  CATEGORY_COLORS,
  FINDING_KIND_LABEL,
  SEVERITY_COLORS,
} from '../types';
import type { Category } from '../types';
import { flattenFindings, useReport } from './useReport';

/**
 * Deep-link target for an individual finding at
 * `/scan/:fixtureKey/finding/:findingIdx`.
 *
 * The index is the position in the preorder walk of every finding
 * across every entry tree (see `flattenFindings` in `useReport.ts`).
 * That's stable per scan-JSON — different scans renumber, but within
 * one JSON the URL keeps pointing at the same finding.
 */
export function FindingDetailPage() {
  const { findingIdx } = useParams<{ findingIdx: string }>();
  const { report, fixture, fixtureKey, error, loading } = useReport();

  const all = useMemo(() => flattenFindings(report), [report]);
  const idx = Number(findingIdx);
  const current = Number.isFinite(idx) ? all[idx] : undefined;
  const prev = current && idx > 0 ? all[idx - 1] : undefined;
  const next = current && idx + 1 < all.length ? all[idx + 1] : undefined;

  if (!fixtureKey) {
    return <Shell title="finding"><Err msg="no fixture key in URL" /></Shell>;
  }
  if (loading) {
    return <Shell title="finding" fixtureKey={fixtureKey}><Loading /></Shell>;
  }
  if (error || !report || !fixture) {
    return <Shell title="finding" fixtureKey={fixtureKey}><Err msg={error ?? 'no report'} /></Shell>;
  }
  if (!current) {
    return (
      <Shell title="finding" fixtureKey={fixtureKey} fixtureLabel={fixture.label}>
        <Err msg={`finding #${findingIdx} not found in this scan (have ${all.length})`} />
        <p style={{ marginTop: 12 }}>
          <Link to={`/scan/${fixtureKey}/report`} style={primaryLinkStyle}>
            ← back to scan report
          </Link>
        </p>
      </Shell>
    );
  }

  const { node, finding } = current;
  return (
    <Shell title={`finding #${idx + 1}`} fixtureKey={fixtureKey} fixtureLabel={fixture.label}>
      <div style={hdrStyle}>
        <span style={{ ...badgeStyle, background: SEVERITY_COLORS[finding.severity] }}>
          {finding.severity}
        </span>
        <span style={{ ...kindBadgeStyle, marginLeft: 8 }}>
          {FINDING_KIND_LABEL[finding.kind] ?? finding.kind}
        </span>
        <span style={{ marginLeft: 'auto', color: '#7e8189', fontSize: 11 }}>
          confidence {finding.confidence.toFixed(2)} · #{idx + 1} of {all.length}
        </span>
      </div>

      <h1 style={titleStyle}>{finding.message}</h1>

      <div style={metaRowStyle}>
        <Meta k="symbol" v={(node.parent_class ? `${node.parent_class}.` : '') + node.name} />
        <Meta k="file" v={`${node.file}:${finding.line}`} />
        <Meta
          k="node"
          v={node.name}
          link={`/scan/${fixtureKey}/node/${encodeURIComponent(node.id)}`}
          linkTitle="Open this node's detail page"
        />
        <Meta k="kind" v={FINDING_KIND_LABEL[finding.kind] ?? finding.kind} />
      </div>

      {(finding.evidence?.length ?? 0) > 0 && (
        <Section title="evidence">
          <ul style={listStyle}>
            {finding.evidence!.map((e, j) => (
              <li key={j} style={evidenceItemStyle}>
                <code style={codeStyle}>{e.call}</code>
                <span style={{ color: '#7e8189', marginLeft: 6 }}>@ :{e.line}</span>
                {e.category && (
                  <span
                    style={{
                      ...miniBadgeStyle,
                      marginLeft: 8,
                      background: CATEGORY_COLORS[e.category as Category],
                    }}
                  >
                    {e.category}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </Section>
      )}

      {finding.remediation && (
        <Section title="remediation">
          <div style={remediationStyle}>{finding.remediation}</div>
        </Section>
      )}

      <Section title="navigate">
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <Link
            to={`/scan/${fixtureKey}/node/${encodeURIComponent(node.id)}`}
            style={primaryLinkStyle}
          >
            Open node detail →
          </Link>
          <Link to={`/scan/${fixtureKey}/report`} style={secondaryLinkStyle}>
            ← Scan report
          </Link>
          <Link to={`/scan/${fixtureKey}`} style={secondaryLinkStyle}>
            ← Dashboard
          </Link>
          {prev && (
            <Link
              to={`/scan/${fixtureKey}/finding/${idx - 1}`}
              style={secondaryLinkStyle}
              title={`Previous finding: ${FINDING_KIND_LABEL[prev.finding.kind] ?? prev.finding.kind}`}
            >
              ← Prev finding (#{idx})
            </Link>
          )}
          {next && (
            <Link
              to={`/scan/${fixtureKey}/finding/${idx + 1}`}
              style={secondaryLinkStyle}
              title={`Next finding: ${FINDING_KIND_LABEL[next.finding.kind] ?? next.finding.kind}`}
            >
              Next finding (#{idx + 2}) →
            </Link>
          )}
        </div>
      </Section>
    </Shell>
  );
}

// ─── Shared shell ────────────────────────────────────────────────────────

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

function Meta({
  k, v, link, linkTitle,
}: { k: string; v: string; link?: string; linkTitle?: string }) {
  const body = (
    <>
      <span style={metaKeyStyle}>{k}</span>
      <span style={metaValueStyle}>{v}</span>
    </>
  );
  if (link) {
    return (
      <Link to={link} title={linkTitle} style={{ ...metaStyle, textDecoration: 'none' }}>
        {body}
      </Link>
    );
  }
  return <div style={metaStyle}>{body}</div>;
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
const crumbCurrentStyle: React.CSSProperties = {
  color: '#d7d9dc', fontWeight: 600,
};
const crumbSepStyle: React.CSSProperties = { color: '#5f626a' };
const mainStyle: React.CSSProperties = {
  maxWidth: 980, margin: '0 auto', padding: '22px 24px 40px',
};
const hdrStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center',
};
const titleStyle: React.CSSProperties = {
  fontSize: 20, fontWeight: 600, color: '#d7d9dc', marginTop: 12, marginBottom: 0,
  lineHeight: 1.4,
};
const metaRowStyle: React.CSSProperties = {
  display: 'flex', flexWrap: 'wrap', gap: 18, marginTop: 18,
};
const metaStyle: React.CSSProperties = {
  display: 'inline-flex', flexDirection: 'column', color: 'inherit',
};
const metaKeyStyle: React.CSSProperties = {
  fontSize: 10, color: '#7e8189', textTransform: 'uppercase', letterSpacing: 0.5,
};
const metaValueStyle: React.CSSProperties = {
  fontSize: 13, color: '#d7d9dc', fontWeight: 500,
};
const sectionTitleStyle: React.CSSProperties = {
  fontSize: 11, color: '#9ca0a8', textTransform: 'uppercase', letterSpacing: 0.4,
  marginBottom: 8, fontWeight: 700,
};
const listStyle: React.CSSProperties = { margin: 0, padding: 0, listStyle: 'none' };
const evidenceItemStyle: React.CSSProperties = {
  padding: '6px 0', borderBottom: '1px solid #2f3136', fontFamily: 'ui-monospace, monospace',
  fontSize: 12, display: 'flex', alignItems: 'center', flexWrap: 'wrap',
};
const codeStyle: React.CSSProperties = {
  background: '#26282c', padding: '2px 6px', borderRadius: 3, color: '#d7d9dc',
};
const remediationStyle: React.CSSProperties = {
  background: '#26282c', border: '1px solid #3f4147', borderRadius: 4, padding: 12,
  color: '#d7d9dc', fontSize: 13, lineHeight: 1.5,
};
const badgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '3px 9px', borderRadius: 3, color: '#0a0a14',
  fontSize: 11, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.3,
};
const kindBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '3px 9px', borderRadius: 3,
  background: '#3f4147', color: '#d7d9dc', fontSize: 11, fontWeight: 600,
  textTransform: 'uppercase', letterSpacing: 0.3,
};
const miniBadgeStyle: React.CSSProperties = {
  display: 'inline-block', padding: '1px 6px', borderRadius: 3, color: '#0a0a14',
  fontSize: 9, fontWeight: 700, textTransform: 'uppercase', letterSpacing: 0.3,
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
