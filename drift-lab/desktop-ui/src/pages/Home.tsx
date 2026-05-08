import { useEffect } from "react";
import { useNavigate } from "react-router-dom";

import DoneState from "../components/DoneState";
import MagicOrb from "../components/MagicOrb";
import Orbs from "../components/Orbs";
import RunButton from "../components/RunButton";
import SearchBox from "../components/SearchBox";
import Steps from "../components/Steps";
import UpdateBanner from "../components/UpdateBanner";
import { SettingsIcon } from "../components/icons";
import {
  onRunComplete,
  onRunError,
  onStepUpdate,
  selectProjectPath,
  startRun,
} from "../lib/tauri";
import { useRunStore } from "../store/runStore";

export default function HomePage() {
  const navigate = useNavigate();
  const {
    projectPath,
    setProjectPath,
    runId,
    isRunning,
    error,
    result,
    steps,
    beginRun,
    applyStep,
    finishRun,
    failRun,
    reset,
  } = useRunStore();

  // Subscribe to backend events for the lifetime of the page.
  useEffect(() => {
    let stepUnsub: (() => void) | undefined;
    let doneUnsub: (() => void) | undefined;
    let errUnsub: (() => void) | undefined;

    (async () => {
      stepUnsub = await onStepUpdate((u) => {
        applyStep({ index: u.index, status: u.status, detail: u.detail, durationMs: u.durationMs });
      });
      doneUnsub = await onRunComplete((c) => finishRun(c));
      errUnsub = await onRunError((e) => failRun(e.message));
    })();

    return () => {
      stepUnsub?.();
      doneUnsub?.();
      errUnsub?.();
    };
  }, [applyStep, finishRun, failRun]);

  const handleStart = async () => {
    if (isRunning) return;
    try {
      const id = await startRun(projectPath);
      beginRun(id);
    } catch (e) {
      failRun(e instanceof Error ? e.message : String(e));
    }
  };

  const handlePick = async () => {
    const picked = await selectProjectPath();
    if (picked) setProjectPath(picked);
  };

  const view = !runId ? "idle" : result ? "done" : "running";

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

      {view === "idle" && (
        <>
          <div className="logo">Drift</div>
          <div className="logo-sub">by refactor-labs</div>

          <SearchBox
            value={projectPath}
            onChange={setProjectPath}
            onPick={handlePick}
            onSubmit={handleStart}
            disabled={isRunning}
          />

          <RunButton onClick={handleStart} disabled={isRunning || !projectPath.trim()} />

          <div className="hint">
            Press <kbd>Enter</kbd> to start
          </div>

          {error && (
            <div className="hint" style={{ color: "#c82626", marginTop: 12 }}>
              {error}
            </div>
          )}
        </>
      )}

      {view === "running" && (
        <div className="loading-wrap">
          <MagicOrb />
          <Steps steps={steps} />
        </div>
      )}

      {view === "done" && result && (
        <DoneState
          issuesFound={result.issuesFound}
          criticalCount={result.criticalCount}
          onView={() => navigate(`/report/${result.runId}`)}
          onReset={reset}
        />
      )}
    </div>
  );
}
