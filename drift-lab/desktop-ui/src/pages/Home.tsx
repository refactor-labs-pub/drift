import { useCallback, useState } from "react";
import { useNavigate } from "react-router-dom";

import Orbs from "../components/Orbs";
import RunButton from "../components/RunButton";
import SearchBox from "../components/SearchBox";
import UpdateBanner from "../components/UpdateBanner";
import { CheckIcon, SettingsIcon, ArrowRightIcon } from "../components/icons";
import StaticScanRunningView from "../components/scan-summary/StaticScanRunningView";
import {
  selectProjectPath,
  startStaticScan,
} from "../lib/tauri";
import { useRunStore } from "../store/runStore";

/**
 * The Home page IS the static-scan pipeline.
 *
 *   idle  → folder picker + Run button
 *   running → MagicOrb + streamed progress + inline entry picker
 *   done  → brief stats + auto-navigate to /scan/:scanId
 *   error → message + reset
 *
 * Static scan kicks off the moment the user clicks Run — no goal prompt
 * (the analyzer is deterministic), no Docker setup, no per-tool approval.
 * The LLM is only consulted in the *report* phase (Generate suggestions),
 * which happens on the next route.
 */

type Phase =
  | { kind: "idle" }
  | { kind: "running"; scanId: string; startedAt: number }
  | {
      kind: "done";
      scanId: string;
      savedPath: string;
      pickedRoot: string | null;
      durationMs: number;
    }
  | { kind: "error"; message: string };

export default function HomePage() {
  const navigate = useNavigate();
  const { projectPath, setProjectPath } = useRunStore();
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  const handlePick = useCallback(async () => {
    const picked = await selectProjectPath();
    if (picked) setProjectPath(picked);
  }, [setProjectPath]);

  const handleStart = useCallback(async () => {
    if (!projectPath.trim()) return;
    if (phase.kind === "running") return;
    try {
      const id = await startStaticScan(projectPath);
      setPhase({ kind: "running", scanId: id, startedAt: Date.now() });
    } catch (e) {
      setPhase({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, [projectPath, phase.kind]);

  // Called by StaticScanRunningView when `scan://complete` fires. The
  // saved scan is already on disk — we transition to a brief done card
  // and let the user step into the full report.
  const handleComplete = useCallback(
    (scanId: string, savedPath: string, pickedRoot: string | null) => {
      setPhase((cur) => {
        if (cur.kind !== "running") return cur;
        return {
          kind: "done",
          scanId,
          savedPath,
          pickedRoot,
          durationMs: Date.now() - cur.startedAt,
        };
      });
    },
    [],
  );

  const handleError = useCallback((message: string) => {
    setPhase({ kind: "error", message });
  }, []);

  const handleReset = useCallback(() => setPhase({ kind: "idle" }), []);

  return (
    <div className="stage">
      <Orbs />

      <div className="home-update-slot">
        <UpdateBanner compact />
      </div>

      <button
        type="button"
        className="settings-fab"
        aria-label="Settings"
        onClick={() => navigate("/settings")}
      >
        <SettingsIcon />
      </button>

      {phase.kind === "idle" && (
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

          <RunButton onClick={handleStart} disabled={!projectPath.trim()} />

          <div className="hint">
            Press <kbd>Enter</kbd> to run a static scan
          </div>
        </>
      )}

      {phase.kind === "running" && (
        <StaticScanRunningView
          scanId={phase.scanId}
          onComplete={handleComplete}
          onError={handleError}
        />
      )}

      {phase.kind === "done" && (
        <DonePanel
          scanId={phase.scanId}
          pickedRoot={phase.pickedRoot}
          durationMs={phase.durationMs}
          savedPath={phase.savedPath}
          onView={() => navigate(`/scan/${phase.scanId}`)}
          onAnother={handleReset}
        />
      )}

      {phase.kind === "error" && (
        <ErrorPanel message={phase.message} onReset={handleReset} />
      )}
    </div>
  );
}

function DonePanel({
  scanId,
  pickedRoot,
  durationMs,
  savedPath,
  onView,
  onAnother,
}: {
  scanId: string;
  pickedRoot: string | null;
  durationMs: number;
  savedPath: string;
  onView: () => void;
  onAnother: () => void;
}) {
  return (
    <div className="done-state">
      <div className="done-icon">
        <CheckIcon />
      </div>
      <div>
        <div className="done-title">Scan complete ✨</div>
        <div className="done-sub">
          {pickedRoot ? <>Focused on <code>{pickedRoot}</code> · </> : null}
          {(durationMs / 1000).toFixed(1)}s · saved to <code>{shortPath(savedPath)}</code>
        </div>
        <div className="done-sub" style={{ fontSize: 10.5, marginTop: 6 }}>
          scan id <code>{scanId.slice(0, 8)}…</code>
        </div>
      </div>
      <div className="done-actions">
        <button type="button" className="view-btn" onClick={onView}>
          View report
          <ArrowRightIcon />
        </button>
        <button type="button" className="ghost-btn" onClick={onAnother}>
          Run another
        </button>
      </div>
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
  return (
    <div className="done-state" style={{ borderColor: "#c82626" }}>
      <div>
        <div className="done-title" style={{ color: "#c82626" }}>Scan failed</div>
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

/** Shorten an absolute path to `~/.drift/scans/<id>.json` form for the
 *  done card. We expect the path to live under the user's home, so a
 *  leading-home replacement is enough. */
function shortPath(p: string): string {
  // Best-effort — the Tauri host doesn't expose $HOME from here, but
  // every saved scan lives under .drift/scans/ so chop everything before.
  const idx = p.lastIndexOf(".drift/");
  if (idx >= 0) return "~/" + p.slice(idx);
  return p;
}
