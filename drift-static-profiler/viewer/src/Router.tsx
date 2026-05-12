import { Navigate, Route, Routes } from 'react-router-dom';
import { App } from './App';
import { FIXTURES } from './fixtures';
import { ScanReportPage } from './pages/ScanReportPage';
import { FindingDetailPage } from './pages/FindingDetailPage';
import { NodeDetailPage } from './pages/NodeDetailPage';
import { FixtureIndexPage } from './pages/FixtureIndexPage';

/**
 * Top-level route map. URL design:
 *
 *   /                                          → fixture index (one card per JSON)
 *   /scan/:fixtureKey                          → legacy in-tab dashboard (App.tsx)
 *   /scan/:fixtureKey/report                   → NEW dedicated full-page Scan Report
 *   /scan/:fixtureKey/finding/:findingIdx      → individual finding detail page
 *   /scan/:fixtureKey/node/:nodeId             → individual node detail page
 *
 * Every route is bookmarkable + shareable + back/forward-aware. The
 * fixture identity lives entirely in the URL — no more app-level
 * fixture-selection state.
 */
export function Router() {
  // First viable fixture for the legacy redirect — keeps `/` from being
  // a dead page when an old bookmark hits the new viewer.
  const defaultFixtureKey = FIXTURES[0]?.key ?? 'python-fastapi';
  return (
    <Routes>
      <Route path="/" element={<FixtureIndexPage />} />
      <Route path="/scan/:fixtureKey" element={<App />} />
      <Route path="/scan/:fixtureKey/report" element={<ScanReportPage />} />
      <Route path="/scan/:fixtureKey/finding/:findingIdx" element={<FindingDetailPage />} />
      <Route path="/scan/:fixtureKey/node/:nodeId" element={<NodeDetailPage />} />
      {/* Anything else → fixture index. */}
      <Route path="*" element={<Navigate to={`/scan/${defaultFixtureKey}/report`} replace />} />
    </Routes>
  );
}
