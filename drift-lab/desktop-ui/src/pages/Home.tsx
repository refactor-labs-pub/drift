import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";

import ActiveModelBadge from "../components/ActiveModelBadge";
import LocalServerLinks from "../components/LocalServerLinks";
import Orbs from "../components/Orbs";
import PreviousScansDropdown, {
  type PriorScanForPath,
} from "../components/PreviousScansDropdown";
import RunButton from "../components/RunButton";
import SearchBox from "../components/SearchBox";
import UpdateBanner from "../components/UpdateBanner";
import { SettingsIcon } from "../components/icons";
import StaticScanRunningView from "../components/scan-summary/StaticScanRunningView";
import {
  deleteStaticScan,
  listStaticScans,
  loadStaticScan,
  restartScanFromCache,
  selectProjectPath,
  startStaticScan,
  type ScanPickerRoot,
} from "../lib/tauri";
import { useRunStore } from "../store/runStore";

/**
 * The Home page IS the static-scan pipeline.
 *
 *   idle    → folder picker + Run button
 *   running → MagicOrb + streamed progress + inline entry picker + Stop button
 *   complete → brief state; auto-navigates to /scan/:scanId on mount
 *   error   → message + reset
 *
 * The phase state lives on the run store (NOT local `useState`), so a
 * round-trip to `/settings` and back doesn't drop a scan in progress —
 * Home re-mounts and re-reads the phase off the store. Event listeners
 * for `scan://progress` / `scan://complete` / `scan://error` are also
 * installed once at the App level so events arriving while Home is
 * unmounted (user is on Settings) still update the timeline.
 */
export default function HomePage() {
  const navigate = useNavigate();
  const { projectPath, setProjectPath } = useRunStore();
  const staticScan = useRunStore((s) => s.staticScan);
  const beginStaticScan = useRunStore((s) => s.beginStaticScan);
  const applyStaticScanError = useRunStore((s) => s.applyStaticScanError);
  const resetStaticScan = useRunStore((s) => s.resetStaticScan);

  const handlePick = useCallback(async () => {
    const picked = await selectProjectPath();
    if (picked) setProjectPath(picked);
  }, [setProjectPath]);

  const handleStart = useCallback(async () => {
    if (!projectPath.trim()) return;
    if (staticScan.kind === "running") return;
    try {
      const id = await startStaticScan(projectPath);
      beginStaticScan(id);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      // Pass an empty scanId since no scan ever got assigned — the
      // store's defensive check tolerates that and still writes the
      // error.
      applyStaticScanError("", message);
    }
  }, [projectPath, staticScan.kind, beginStaticScan, applyStaticScanError]);

  // Once the backend confirms completion, jump to the report. We watch
  // store state rather than relying on a callback so the navigation also
  // fires when the user was on /settings during completion: as soon as
  // they navigate back to /, this effect runs.
  useEffect(() => {
    if (staticScan.kind === "complete") {
      const target = `/scan/${staticScan.scanId}`;
      resetStaticScan();
      navigate(target);
    }
  }, [staticScan, navigate, resetStaticScan]);

  const handleReset = useCallback(() => resetStaticScan(), [resetStaticScan]);

  // Cache-awareness: when the user enters / picks a project path, look up
  // every prior scan whose `sourceRoot` matches. The `PreviousScansDropdown`
  // surfaces them as a single dismissible column-mate next to the main CTA;
  // each entry offers two resume points — "Pick entry" (cached candidates)
  // or "Open report" (jump to findings).
  //
  // Why exact-match on `sourceRoot`: a fuzzy match would surface stale
  // results from a sibling directory and the user would lose trust. Newest
  // first so the most-likely-relevant scan is at the top of the dropdown.
  //
  // The lookup is cheap on the Rust side (a `read_dir` + per-file
  // metadata read). We debounce by 200ms so typing into the SearchBox
  // doesn't spam the backend with one call per keystroke. Loading the
  // per-scan `pickerRoots` is also debounced + parallelised inside one
  // effect cycle so the dropdown shows up populated, not in two steps.
  const [priorScans, setPriorScans] = useState<PriorScanForPath[]>([]);
  useEffect(() => {
    if (!projectPath.trim()) {
      setPriorScans([]);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(async () => {
      try {
        const all = await listStaticScans();
        if (cancelled) return;
        const target = projectPath.trim();
        const matches = all
          .filter((s) => s.sourceRoot === target)
          .sort((a, b) => Date.parse(b.savedAt) - Date.parse(a.savedAt));
        if (matches.length === 0) {
          setPriorScans([]);
          return;
        }
        // Load each scan's envelope in parallel so the dropdown has the
        // cached `pickerRoots` ready on first render — disabling the
        // "Pick entry" button on the fly would be a worse UX than just
        // resolving up-front.
        const enriched = await Promise.all(
          matches.map(async (meta) => {
            try {
              const stored = await loadStaticScan(meta.scanId);
              return {
                meta,
                pickerRoots: stored.pickerRoots ?? [],
              } satisfies PriorScanForPath;
            } catch {
              return { meta, pickerRoots: [] } satisfies PriorScanForPath;
            }
          }),
        );
        if (!cancelled) setPriorScans(enriched);
      } catch {
        if (!cancelled) setPriorScans([]);
      }
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [projectPath]);

  // "Pick entry" from the dropdown re-runs the focused scan against the
  // cached candidates. Same handoff as the report page's "Pick another
  // entry" — seed the runStore so the in-flight scan picks up on
  // re-render, then stay on Home (the running view is already here).
  const handlePickEntryFromPrior = useCallback(
    async (scanId: string, roots: ScanPickerRoot[]) => {
      if (roots.length === 0) return;
      // For a one-click resume we pick the highest-reach root (index 0
      // in the saved order — it's already sorted by reach). The user
      // who wants to choose explicitly should use the report page,
      // which renders the full picker UI.
      try {
        const newId = await restartScanFromCache(scanId, roots[0].index);
        beginStaticScan(newId);
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        applyStaticScanError("", message);
      }
    },
    [beginStaticScan, applyStaticScanError],
  );

  const handleOpenPriorReport = useCallback(
    (scanId: string) => {
      navigate(`/scan/${scanId}`);
    },
    [navigate],
  );

  // Delete one prior scan and drop it from the dropdown without
  // re-fetching the whole list — the backend command is idempotent so a
  // brief race where the user double-clicks delete is harmless.
  const handleDeletePriorScan = useCallback(async (scanId: string) => {
    await deleteStaticScan(scanId);
    setPriorScans((prev) => prev.filter((s) => s.meta.scanId !== scanId));
  }, []);

  // Top-align when running/error so a tall left column (orb + many
  // progress rows) doesn't push the right-pane's picker above the
  // viewport via flex vertical centering. Idle keeps the centered hero
  // layout.
  const stageClass =
    staticScan.kind === "running" || staticScan.kind === "error"
      ? "stage stage--running"
      : "stage";

  return (
    <div className={stageClass}>
      <Orbs />

      <div className="home-update-slot">
        <UpdateBanner compact />
      </div>

      <div className="home-active-model-slot">
        <ActiveModelBadge />
      </div>

      <LocalServerLinks />

      <button
        type="button"
        className="settings-fab"
        aria-label="Settings"
        onClick={() => navigate("/settings")}
      >
        <SettingsIcon />
      </button>

      {staticScan.kind === "idle" && (
        <>
          <div className="logo">Drift</div>
          <div className="logo-sub">by refactor-labs</div>

          <SearchBox
            value={projectPath}
            onChange={setProjectPath}
            onPick={handlePick}
            onSubmit={handleStart}
            disabled={false}
          />

          {/*
            Action row: when prior scans exist for this folder, render
            two columns — primary CTA on the left, "Previous scans"
            dropdown on the right. When there are NO prior scans,
            collapse to a single centered column so the primary CTA
            sits dead-center instead of being pushed off-axis by a
            reserved-but-empty sibling column. Stacks on narrow
            viewports via the grid media query (see globals.css).
          */}
          <div
            className={
              priorScans.length > 0
                ? "home-action-row home-action-row--has-prior"
                : "home-action-row"
            }
          >
            <div className="home-action-col home-action-col--primary">
              <RunButton
                onClick={handleStart}
                disabled={!projectPath.trim()}
                subText="Full fresh discovery — no cache reuse"
                title="Runs the full static analyzer from source: walk → parse → discover entries → pick → focused profile. Discards every cached entry / suggestion from any prior scan of this folder."
              />
            </div>
            {priorScans.length > 0 && (
              <div className="home-action-col home-action-col--secondary">
                <PreviousScansDropdown
                  scans={priorScans}
                  onPickEntry={handlePickEntryFromPrior}
                  onOpenReport={handleOpenPriorReport}
                  onDelete={handleDeletePriorScan}
                />
              </div>
            )}
          </div>

          <div className="hint">
            Press <kbd>Enter</kbd> to Make Static Magic.
            {priorScans.length > 0 &&
              " Or pick where to resume from a previous scan on the right."}
          </div>

          <div className="home-secondary-row">
            <RunButton
              label="Make Active Magic"
              onClick={() => navigate("/live-scan")}
              subText="Live profiler — stream events or open a saved log"
              title="Open the active-scan page: live-tail profiler events from a Supabase Realtime channel, or aggregate a saved events.log into a snakeviz-style icicle chart."
            />
          </div>
        </>
      )}

      {staticScan.kind === "running" && (
        <StaticScanRunningView
          scanId={staticScan.scanId}
          rows={staticScan.rows}
          overall={staticScan.overall}
          roots={staticScan.roots}
          pickedRoot={staticScan.pickedRoot}
        />
      )}

      {staticScan.kind === "error" && (
        <ErrorPanel message={staticScan.message} onReset={handleReset} />
      )}
    </div>
  );
}

function ErrorPanel({
  message,
  onReset,
}: {
  message: string;
  onReset: () => void;
}) {
  // "scan stopped" (from the user-pressed-Stop path) renders with a
  // neutral colour and a quieter title — it isn't actually a failure, it's
  // a user choice. Everything else stays loud-red.
  const wasStopped = message === "scan stopped";
  const borderColor = wasStopped ? "var(--border)" : "#c82626";
  const titleColor = wasStopped ? "var(--text)" : "#c82626";
  const title = wasStopped ? "Scan stopped" : "Scan failed";
  return (
    <div className="done-state" style={{ borderColor }}>
      <div>
        <div className="done-title" style={{ color: titleColor }}>
          {title}
        </div>
        <div className="done-sub">{message}</div>
      </div>
      <div className="done-actions">
        <button type="button" className="ghost-btn" onClick={onReset}>
          Try again
        </button>
      </div>
    </div>
  );
}

