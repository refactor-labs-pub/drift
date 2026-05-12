import { Link } from 'react-router-dom';
import { FIXTURES } from '../fixtures';

/**
 * Landing page at `/`. Renders one clickable card per available
 * fixture JSON. Each card links straight to that fixture's full Scan
 * Report page — the new dedicated `/scan/:fixtureKey/report` route.
 *
 * Acts as the "one page per JSON" affordance the user asked for: every
 * scan is a first-class navigable destination, not a dropdown choice.
 */
export function FixtureIndexPage() {
  return (
    <div style={pageStyle}>
      <header style={headerStyle}>
        <div style={brandStyle}>drift · static profiler</div>
        <div style={subStyle}>
          Each scan JSON is a separate page. Pick one to open its full report.
        </div>
      </header>
      <main style={gridStyle}>
        {FIXTURES.map((f) => (
          <Link
            key={f.key}
            to={`/scan/${f.key}/report`}
            style={cardStyle}
            title={`Open the full scan report for ${f.label}`}
          >
            <div style={cardKindStyle}>{f.key === 'custom' ? 'CUSTOM' : 'FIXTURE'}</div>
            <div style={cardLabelStyle}>{f.label}</div>
            <div style={cardDescStyle}>{f.description}</div>
            <div style={cardFooterStyle}>
              <span style={cardPathStyle}>{f.json}</span>
              <span style={cardArrowStyle}>→</span>
            </div>
          </Link>
        ))}
      </main>
      <footer style={footerStyle}>
        Looking for the in-tab dashboard? Open{' '}
        <Link to="/scan/python-fastapi" style={linkStyle}>/scan/&lt;key&gt;</Link>{' '}
        directly. Every route works refresh-safely and back-button-safely.
      </footer>
    </div>
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
