import { useEffect, useState } from 'react';
import type { FixtureSpec } from './types';

/**
 * User scans live in `viewer/public/fixtures/scans/<name>.json` and are
 * indexed by `scans/index.json` (the Makefile's `make scan` recipe
 * regenerates that index on every run). Browsers can't list directories,
 * so this is the only way the viewer learns what scans the user has run.
 *
 * The index is fetched once per app load and cached in a module-level
 * promise so the three consumers (`useReport`, `FixtureIndexPage`,
 * `App.tsx`) share one network round-trip.
 *
 * Cache: `no-store` — a fresh `make scan` should be visible on the next
 * hook subscriber (mounting a page, switching routes) without a hard
 * reload. The index is a tiny JSON, so refetching is cheap.
 */

const INDEX_URL = '/fixtures/scans/index.json';

let cached: Promise<FixtureSpec[]> | null = null;

/** Fetch (or return the cached promise of) the user-scan list. */
export function loadUserScans(): Promise<FixtureSpec[]> {
  if (cached) return cached;
  cached = fetch(INDEX_URL, { cache: 'no-store' })
    .then((r) => (r.ok ? (r.json() as Promise<FixtureSpec[]>) : []))
    // 404 = no scans yet (the file hasn't been generated). Treat as empty.
    .catch(() => [] as FixtureSpec[]);
  return cached;
}

/** Force the next `loadUserScans()` call to refetch. */
export function invalidateUserScans(): void {
  cached = null;
}

/**
 * React hook returning `{ scans, loading }`. The list starts empty
 * synchronously, then populates after the fetch resolves.
 */
export function useUserScans(): { scans: FixtureSpec[]; loading: boolean } {
  const [scans, setScans] = useState<FixtureSpec[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let alive = true;
    loadUserScans().then((s) => {
      if (!alive) return;
      setScans(s);
      setLoading(false);
    });
    return () => {
      alive = false;
    };
  }, []);
  return { scans, loading };
}
