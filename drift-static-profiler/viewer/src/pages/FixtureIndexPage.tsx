import { Link } from 'react-router-dom';
import { FIXTURES } from '../fixtures';
import { useUserScans } from '../userScans';
import type { FixtureSpec } from '../types';

/**
 * Landing page at `/`. Renders one clickable card per available
 * fixture JSON. Each card links straight to that fixture's full Scan
 * Report page — the new dedicated `/scan/:fixtureKey/report` route.
 *
 * Two card sections: built-in `FIXTURES` (the language demos shipped
 * with the repo) and user scans (each `make scan /some/path` writes
 * one entry to `viewer/public/fixtures/scans/index.json`, picked up
 * here via `useUserScans`).
 */
export function FixtureIndexPage() {
  const { scans, loading } = useUserScans();
  return (
    <div style={pageStyle}>
      <header style={headerStyle}>
        <div style={brandStyle}>drift · static profiler</div>
        <div style={subStyle}>
          Each scan JSON is a separate page. Pick one to open its full report.
        </div>
      </header>

      <section style={sectionStyle}>
        <div style={sectionTitleStyle}>
          your scans
          <span style={sectionCountStyle}>
            {loading ? '…' : `${scans.length}`}
          </span>
        </div>
        {scans.length === 0 ? (
          <div style={emptyStyle}>
            No scans yet. Run <code style={codeStyle}>make scan /path/to/your-project</code>
            {' '}from the <code style={codeStyle}>drift-static-profiler/</code> directory.
            Each scan lands as its own card here, named after the folder.
          </div>
        ) : (
          <div style={gridStyle}>
            {scans.map((f) => (
              <ScanCard key={f.key} f={f} kindLabel="SCAN" />
            ))}
          </div>
        )}
      </section>

      <section style={sectionStyle}>
        <div style={sectionTitleStyle}>
          built-in fixtures
          <span style={sectionCountStyle}>{FIXTURES.length}</span>
        </div>
        <div style={gridStyle}>
          {FIXTURES.map((f) => (
            <ScanCard key={f.key} f={f} kindLabel="FIXTURE" />
          ))}
        </div>
      </section>

      <footer style={footerStyle}>
        Looking for the in-tab dashboard? Open{' '}
        <Link to="/scan/python-fastapi" style={linkStyle}>/scan/&lt;key&gt;</Link>{' '}
        directly. Every route works refresh-safely and back-button-safely.
      </footer>
    </div>
  );
}

function ScanCard({ f, kindLabel }: { f: FixtureSpec; kindLabel: string }) {
  return (
    <Link
      to={`/scan/${f.key}/report`}
      style={cardStyle}
      title={`Open the full scan report for ${f.label}`}
    >
      <div style={cardKindStyle}>{kindLabel}</div>
      <div style={cardLabelStyle}>{f.label}</div>
      <div style={cardDescStyle}>{f.description}</div>
      <div style={cardFooterStyle}>
        <span style={cardPathStyle}>{f.json}</span>
        <span style={cardArrowStyle}>→</span>
      </div>
    </Link>
  );
}

const pageStyle: React.CSSProperties = {
  minHeight: '100vh',
  background: '#1e1f22',
  color: '#d7d9dc',
  padding: '32px 24px',
};
const headerStyle: React.CSSProperties = {
  maxWidth: 1200, margin: '0 auto 22px',
};
const sectionStyle: React.CSSProperties = {
  maxWidth: 1200, margin: '0 auto 28px',
};
const sectionTitleStyle: React.CSSProperties = {
  fontSize: 11, fontWeight: 700, color: '#9ca0a8',
  textTransform: 'uppercase', letterSpacing: 0.8,
  margin: '0 0 10px',
  display: 'flex', alignItems: 'center', gap: 8,
};
const sectionCountStyle: React.CSSProperties = {
  background: '#2a2c30', color: '#7e8189',
  padding: '1px 7px', borderRadius: 8, fontSize: 10, fontWeight: 600,
};
const emptyStyle: React.CSSProperties = {
  background: '#26282c', border: '1px dashed #3f4147', borderRadius: 6,
  padding: '14px 16px', color: '#9ca0a8', fontSize: 13, lineHeight: 1.6,
};
const codeStyle: React.CSSProperties = {
  fontFamily: 'ui-monospace, monospace', fontSize: 12,
  background: '#1e1f22', color: '#d7d9dc',
  padding: '1px 5px', borderRadius: 3, border: '1px solid #3f4147',
};
const brandStyle: React.CSSProperties = {
  fontSize: 20, fontWeight: 700, color: '#d7d9dc', letterSpacing: 0.4,
};
const subStyle: React.CSSProperties = {
  fontSize: 13, color: '#9ca0a8', marginTop: 4,
};
const gridStyle: React.CSSProperties = {
  display: 'grid',
  maxWidth: 1200, margin: '0 auto',
  gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))',
  gap: 14,
};
const cardStyle: React.CSSProperties = {
  display: 'block',
  textDecoration: 'none',
  color: 'inherit',
  background: '#26282c',
  border: '1px solid #3f4147',
  borderRadius: 6,
  padding: 16,
  transition: 'border-color 120ms',
};
const cardKindStyle: React.CSSProperties = {
  fontSize: 9, fontWeight: 700, color: '#7e8189',
  textTransform: 'uppercase', letterSpacing: 0.5,
};
const cardLabelStyle: React.CSSProperties = {
  fontSize: 16, fontWeight: 600, color: '#d7d9dc', marginTop: 4,
};
const cardDescStyle: React.CSSProperties = {
  fontSize: 12, color: '#9ca0a8', marginTop: 8, lineHeight: 1.5,
};
const cardFooterStyle: React.CSSProperties = {
  display: 'flex', alignItems: 'center', justifyContent: 'space-between',
  marginTop: 14, paddingTop: 10, borderTop: '1px solid #2f3136',
};
const cardPathStyle: React.CSSProperties = {
  fontFamily: 'ui-monospace, monospace', fontSize: 10, color: '#5f626a',
};
const cardArrowStyle: React.CSSProperties = {
  fontSize: 14, color: '#5b8def',
};
const footerStyle: React.CSSProperties = {
  maxWidth: 1200, margin: '24px auto 0',
  fontSize: 11, color: '#7e8189',
};
const linkStyle: React.CSSProperties = {
  color: '#5b8def', textDecoration: 'underline',
};
