import { useEffect, useState } from 'react';
import { useParams } from 'react-router-dom';
import { FIXTURES } from '../fixtures';
import { useUserScans } from '../userScans';
import type { FixtureSpec, Report } from '../types';

/**
 * Shared report-loading hook. Every page route uses this so the
 * fetch/cache/error story is identical across them.
 *
 * Reads `:fixtureKey` from the route, fetches the matching JSON with
 * `cache: 'no-store'` (so `make refresh` is immediately visible), and
 * returns the parsed `Report` plus the resolved `FixtureSpec`.
 *
 * Fixture resolution checks the built-in `FIXTURES` array first, then
 * falls back to user scans from `/fixtures/scans/index.json` (populated
 * by `make scan`). While the user-scan list is still loading, the hook
 * stays in `loading: true` rather than erroring out — otherwise a hard
 * refresh on a user-scan URL would briefly show "unknown fixture".
 */
export function useReport(): {
  report: Report | null;
  fixture: FixtureSpec | null;
  fixtureKey: string | null;
  error: string | null;
  loading: boolean;
} {
  const { fixtureKey } = useParams<{ fixtureKey: string }>();
  const { scans: userScans, loading: scansLoading } = useUserScans();
  const builtIn = fixtureKey ? FIXTURES.find((f) => f.key === fixtureKey) : undefined;
  const fromScans = fixtureKey ? userScans.find((f) => f.key === fixtureKey) : undefined;
  const fixture = builtIn ?? fromScans ?? null;
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    setError(null);
    setReport(null);
    // Wait for the user-scans index before deciding the key is unknown —
    // otherwise a direct hit on `/scan/ktor/report` would error during
    // the index fetch.
    if (!fixture) {
      if (scansLoading) return;
      setError(fixtureKey ? `unknown fixture: ${fixtureKey}` : 'no fixture key in URL');
      return;
    }
    setLoading(true);
    fetch(fixture.json, { cache: 'no-store' })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((data: Report) => setReport(data))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [fixture?.json, fixtureKey, scansLoading]);

  return { report, fixture, fixtureKey: fixtureKey ?? null, error, loading };
}

/**
 * Walk all entry trees in preorder and build a flat list of findings —
 * each entry is `{node, finding, idx}` where `idx` is the stable
 * position the URL uses (`/scan/:key/finding/:idx`).
 */
export function flattenFindings(
  report: Report | null,
): { node: import('../types').CallTreeNode; finding: import('../types').Finding; idx: number }[] {
  if (!report) return [];
  const out: ReturnType<typeof flattenFindings> = [];
  let idx = 0;
  const walk = (n: import('../types').CallTreeNode) => {
    for (const f of n.findings ?? []) {
      out.push({ node: n, finding: f, idx: idx++ });
    }
    for (const c of n.children) walk(c);
  };
  for (const e of report.entries) walk(e);
  return out;
}

/** Find a node by its `id` (`file::class::name`) anywhere in the tree. */
export function findNodeById(
  report: Report | null,
  id: string,
): import('../types').CallTreeNode | null {
  if (!report) return null;
  const walk = (n: import('../types').CallTreeNode): import('../types').CallTreeNode | null => {
    if (n.id === id) return n;
    for (const c of n.children) {
      const hit = walk(c);
      if (hit) return hit;
    }
    return null;
  };
  for (const e of report.entries) {
    const hit = walk(e);
    if (hit) return hit;
  }
  return null;
}
