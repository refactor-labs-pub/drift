import { useEffect, useRef, useState } from "react";

import {
  UpdateInfo,
  UpdateProgress,
  checkForUpdate,
  downloadAndInstallUpdate,
  withTimeout,
} from "../lib/tauri";

type Phase = "idle" | "checking" | "available" | "downloading" | "installing" | "error";

interface Props {
  /** Auto-check for updates on mount. Defaults to true. */
  autoCheck?: boolean;
  /** Compact banner mode for headers (no card chrome). */
  compact?: boolean;
}

/**
 * Self-contained update prompt. Auto-checks on mount, shows a button when an
 * update is available, streams download progress, then relaunches.
 *
 * Render at most once per page — the underlying updater plugin is fine to call
 * concurrently but the UX would be confusing.
 */
export default function UpdateBanner({ autoCheck = true, compact = false }: Props) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const startedAt = useRef(false);

  useEffect(() => {
    if (!autoCheck || startedAt.current) return;
    startedAt.current = true;
    // Silent on auto-check: a 404 / unconfigured endpoint shouldn't pop a
    // banner at every launch. Real errors surface when the user explicitly
    // clicks Check / Retry below.
    void runCheck({ silent: true });
  }, [autoCheck]);

  async function runCheck(opts: { silent?: boolean } = {}) {
    setPhase("checking");
    setErrorMsg(null);
    try {
      // Same 8s deadline as the Settings → Updates tab — never let the
      // banner sit on "checking" if the endpoint is unreachable.
      const next = await withTimeout(checkForUpdate(), 8000, "Update check");
      if (!next) {
        setPhase("idle");
        setInfo(null);
        return;
      }
      setInfo(next);
      setPhase("available");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (opts.silent) {
        // Stay quiet: log for diagnostics, but don't surface.
        console.warn("[UpdateBanner] auto-check failed:", msg);
        setPhase("idle");
        return;
      }
      setErrorMsg(msg);
      setPhase("error");
    }
  }

  async function runInstall() {
    setPhase("downloading");
    setProgress(null);
    setErrorMsg(null);
    try {
      await downloadAndInstallUpdate((p) => {
        setProgress(p);
        if (p.kind === "finished") setPhase("installing");
      });
      // relaunch() above doesn't return on success — anything past this is rare.
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  if (phase === "idle" || phase === "checking") return null;

  const wrapClass = compact ? "update-banner update-banner--compact" : "update-banner";

  if (phase === "error") {
    return (
      <div className={wrapClass} role="alert">
        <div className="update-banner-text">
          <strong>Update check failed.</strong>{" "}
          <span className="muted">{errorMsg}</span>
        </div>
        <button type="button" className="ghost-btn" onClick={() => runCheck()}>
          Retry
        </button>
      </div>
    );
  }

  if (phase === "available" && info) {
    return (
      <div className={wrapClass}>
        <div className="update-banner-text">
          <strong>Update available — v{info.version}</strong>
          <span className="muted"> · you're on v{info.currentVersion}</span>
        </div>
        <button type="button" className="primary-btn" onClick={runInstall}>
          Update &amp; relaunch
        </button>
      </div>
    );
  }

  if (phase === "downloading" || phase === "installing") {
    const pct =
      progress && progress.kind === "progress" && progress.contentLength
        ? Math.min(100, Math.round((progress.downloaded / progress.contentLength) * 100))
        : null;
    const label =
      phase === "installing"
        ? "Installing… app will relaunch"
        : pct !== null
          ? `Downloading ${pct}%`
          : "Downloading…";
    return (
      <div className={wrapClass} aria-live="polite">
        <div className="update-banner-text">
          <strong>{label}</strong>
          {pct !== null && (
            <div className="update-banner-bar">
              <div className="update-banner-fill" style={{ width: `${pct}%` }} />
            </div>
          )}
        </div>
      </div>
    );
  }

  return null;
}
