import { useEffect, useRef, useState } from "react";

import {
  onScanComplete,
  onScanEntriesReady,
  onScanError,
  onScanProgress,
  selectEntryAndScan,
  type ScanPickerRoot,
} from "../../lib/tauri";
import MagicOrb from "../MagicOrb";
import EntryPicker from "./EntryPicker";
import ScanProgressList, { reduceProgress, type PhaseRow } from "./ScanProgress";

/**
 * The Home page's "running" state for a static scan.
 *
 * Owns the four lifecycle subscriptions (`progress`, `entries-ready`,
 * `complete`, `error`) for the active `scanId`. Maintains:
 *   - the reduced progress timeline (PhaseRow[]) for the left column,
 *   - the entry roots returned by `entries-ready` (right column when
 *     present),
 *   - a local "picker has been answered" flag so the picker disappears
 *     after selection while the focused-tree build is still streaming.
 *
 * Lifecycle callbacks (`onComplete`, `onError`) bubble up to Home so it
 * can navigate or render the appropriate done-state — this view doesn't
 * own routing.
 */
interface Props {
  scanId: string;
  onComplete: (scanId: string) => void;
  onError: (message: string) => void;
}

export default function StaticScanRunningView({
  scanId,
  onComplete,
  onError,
}: Props) {
  const [rows, setRows] = useState<PhaseRow[]>([]);
  const [roots, setRoots] = useState<ScanPickerRoot[] | null>(null);
  const [pickedRoot, setPickedRoot] = useState<ScanPickerRoot | null>(null);

  // Long-lived listeners mustn't be torn down by re-renders, but they DO
  // need to filter against the active `scanId`. A ref shadows the prop so
  // the once-installed handlers always see the latest value.
  const scanIdRef = useRef(scanId);
  useEffect(() => {
    scanIdRef.current = scanId;
  }, [scanId]);

  // Reset local state whenever the parent kicks off a fresh scan with a
  // different id. Keeps the view honest between back-to-back runs.
  useEffect(() => {
    setRows([]);
    setRoots(null);
    setPickedRoot(null);
  }, [scanId]);

  useEffect(() => {
    const cleanup: Array<() => void> = [];
    const isMine = (id: string) => scanIdRef.current === id;
    (async () => {
      cleanup.push(
        await onScanProgress((ev) => {
          if (!isMine(ev.scanId)) return;
          setRows((prev) => reduceProgress(prev, ev));
        }),
      );
      cleanup.push(
        await onScanEntriesReady((p) => {
          if (!isMine(p.scanId)) return;
          setRoots(p.roots);
        }),
      );
      cleanup.push(
        await onScanComplete((p) => {
          if (!isMine(p.scanId)) return;
          onComplete(p.scanId);
        }),
      );
      cleanup.push(
        await onScanError((p) => {
          if (!isMine(p.scanId)) return;
          onError(p.message);
        }),
      );
    })();
    return () => {
      cleanup.forEach((fn) => fn());
    };
  }, [onComplete, onError]);

  const handlePick = async (root: ScanPickerRoot) => {
    setPickedRoot(root);
    try {
      await selectEntryAndScan(scanId, root.index);
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    }
  };

  const isPicking = roots !== null && pickedRoot === null;
  const phaseLabel = isPicking
    ? `Pick one of ${roots.length} entry roots to analyze`
    : pickedRoot
      ? `Building focused call-tree for ${pickedRoot.name}`
      : "Discovering entry points";

  return (
    <div className="running-split">
      <div className="running-split-col running-split-col-left">
        <MagicOrb />
        <div className="scan-running-phase">{phaseLabel}</div>
        <ScanProgressList rows={rows} />
      </div>
      <div className="running-split-col running-split-col-right">
        <div className="right-pane">
          <div className="right-pane-body" style={{ padding: "0 14px 14px" }}>
            {isPicking && roots ? (
              <EntryPicker roots={roots} onPick={handlePick} />
            ) : (
              <RunningSidebarInfo pickedRoot={pickedRoot} />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function RunningSidebarInfo({ pickedRoot }: { pickedRoot: ScanPickerRoot | null }) {
  return (
    <div style={{ padding: "16px 4px", color: "var(--text-dim)", fontSize: 13 }}>
      {pickedRoot ? (
        <>
          <div style={{ fontWeight: 600, color: "var(--text)", marginBottom: 8 }}>
            Focused entry
          </div>
          <code className="scan-code">{pickedRoot.name}</code>
          <div style={{ marginTop: 6, fontSize: 11.5 }}>
            {pickedRoot.file}:{pickedRoot.line} · reach {pickedRoot.reach}
          </div>
          <div style={{ marginTop: 14, fontSize: 12 }}>
            Building call tree, then assembling the findings summary. This is
            where the long compute on big projects lives.
          </div>
        </>
      ) : (
        <>
          <div style={{ fontWeight: 600, color: "var(--text)", marginBottom: 8 }}>
            Static scan
          </div>
          <p>
            Walking your source tree, building a call graph, and ranking the
            top entry points. No LLM is consulted in this phase — the
            analyzer is deterministic.
          </p>
          <p style={{ marginTop: 10 }}>
            When discovery completes, the top entry roots will appear here for
            you to pick from.
          </p>
        </>
      )}
    </div>
  );
}
