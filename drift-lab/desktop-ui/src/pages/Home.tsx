import { useCallback, useState } from "react";
import { useNavigate } from "react-router-dom";

import ActiveModelBadge from "../components/ActiveModelBadge";
import Orbs from "../components/Orbs";
import RunButton from "../components/RunButton";
import SearchBox from "../components/SearchBox";
import UpdateBanner from "../components/UpdateBanner";
import { SettingsIcon } from "../components/icons";
import StaticScanRunningView from "../components/scan-summary/StaticScanRunningView";
import {
  selectProjectPath,
  startStaticScan,
} from "../lib/tauri";
import { useRunStore } from "../store/runStore";

/**
 * The Home page IS the static-scan pipeline.
 *
 *   idle    → folder picker + Run button
 *   running → MagicOrb + streamed progress + inline entry picker
 *   error   → message + reset
 *
 * Once the scan completes the page navigates straight to `/scan/:scanId` —
 * the user never sees an in-between "view report" screen. The LLM is only
 * consulted on the report page, and only when the user clicks "Study this"
 * on an individual finding (no automatic generation).
 */

type Phase =
  | { kind: "idle" }
  | { kind: "running"; scanId: string }
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
      setPhase({ kind: "running", scanId: id });
    } catch (e) {
      setPhase({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, [projectPath, phase.kind]);

  // Scan complete → jump directly to the scan-report page. No intermediate
  // "view report" affordance — the user asked for one less click.
  const handleComplete = useCallback(
    (scanId: string) => {
      navigate(`/scan/${scanId}`);
    },
    [navigate],
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

      <div className="home-active-model-slot">
        <ActiveModelBadge />
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

      {phase.kind === "error" && (
        <ErrorPanel message={phase.message} onReset={handleReset} />
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
