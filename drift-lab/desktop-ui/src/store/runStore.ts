import { create } from "zustand";

import type { Entry as AgentLogEntry } from "../components/ReasoningLog";
import type {
  AgentMode,
  BlockedQuestion,
  LogLine,
  TelemetrySample,
  VisibilityMap,
} from "../lib/tauri";

/** Cap on retained samples. ~600 = 5 minutes at the backend's 2 Hz cadence —
 *  any longer and the sparkline becomes unreadable anyway. */
const TELEMETRY_CAP = 600;
/** Cap on retained log lines. A debug-level scan can emit a few hundred per
 *  minute; 2000 covers ~10 minutes before the oldest start rolling out. */
const LOG_CAP = 2000;

export type StepStatus = "pending" | "active" | "done" | "error";

export interface StepState {
  title: string;
  detail: string;
  status: StepStatus;
  durationMs?: number;
}

export interface RunResult {
  runId: string;
  issuesFound: number;
  criticalCount: number;
}

/** Inputs the user picked for the most-recent run. Persisted on the store so
 *  the Rerun button on the Report and Done views can replay the same scan
 *  without making the user re-enter anything. */
export interface RunParams {
  projectPath: string;
  mode: AgentMode;
  goalPrompt?: string;
}

interface RunStore {
  projectPath: string;
  setProjectPath: (p: string) => void;

  runId: string | null;
  isRunning: boolean;
  error: string | null;
  result: RunResult | null;

  steps: StepState[];

  /** Streaming agent reasoning + tool log. Lives on the store so the Report
   *  page can read it after the live `Home` view unmounts. */
  logEntries: AgentLogEntry[];
  /** Rolling-window telemetry samples for the live TelemetryPane sparklines.
   *  Capped at {@link TELEMETRY_CAP}; oldest drop when full. */
  telemetrySamples: TelemetrySample[];
  /** Rolling-window backend tracing lines, mirroring what's on stderr.
   *  Capped at {@link LOG_CAP}. */
  backendLog: LogLine[];
  /** Currently-in-flight `ask_user` question, or null. While set the
   *  BlockedModal is open and the run is parked. */
  blockedQuestion: BlockedQuestion | null;
  /** Structured "visibility map" delivered just before `RunComplete`. Null
   *  until the backend emits `run://report`. */
  visibilityMap: VisibilityMap | null;
  /** Inputs to replay the same scan. Set when a run starts. */
  runParams: RunParams | null;
  /** Wall-clock UTC ms when the most-recent scan started — Report uses it to
   *  show "ran X seconds ago". */
  startedAt: number | null;
  /** Wall-clock UTC ms when the most-recent scan ended (success or fail). */
  endedAt: number | null;

  /* Internal mutators used by the IPC bridge. Pages should rely on these
   * instead of touching state directly. */
  beginRun: (runId: string, params: RunParams) => void;
  applyStep: (update: { index: number; status: StepStatus; detail?: string; durationMs?: number }) => void;
  finishRun: (result: RunResult) => void;
  failRun: (message: string) => void;
  reset: () => void;
  setLogEntries: (entries: AgentLogEntry[]) => void;
  pushTelemetry: (sample: TelemetrySample) => void;
  pushLogLine: (line: LogLine) => void;
  setBlockedQuestion: (q: BlockedQuestion | null) => void;
  setVisibilityMap: (map: VisibilityMap) => void;
}

/** 6-stage UI timeline. The agent's internal 10-step recipe (in the system
 *  prompt) maps onto these — each visible stage may bundle 1-2 internal
 *  steps. Keep in sync with `agent::workflow::tool_to_step_index` in Rust. */
const DEFAULT_STEPS: StepState[] = [
  { title: "Understanding code",   detail: "Waiting…", status: "pending" },
  { title: "Locating how to run",  detail: "Waiting…", status: "pending" },
  { title: "Setting up runtime",   detail: "Waiting…", status: "pending" },
  { title: "Running + profiling",  detail: "Waiting…", status: "pending" },
  { title: "Building thesis",      detail: "Waiting…", status: "pending" },
  { title: "Reporting",            detail: "Waiting…", status: "pending" },
];

export const useRunStore = create<RunStore>((set) => ({
  projectPath: "/Users/jdoe/projects/checkout-service",
  setProjectPath: (p) => set({ projectPath: p }),

  runId: null,
  isRunning: false,
  error: null,
  result: null,

  steps: DEFAULT_STEPS.map((s) => ({ ...s })),

  logEntries: [],
  telemetrySamples: [],
  backendLog: [],
  blockedQuestion: null,
  visibilityMap: null,
  runParams: null,
  startedAt: null,
  endedAt: null,

  beginRun: (runId, params) =>
    set({
      runId,
      isRunning: true,
      error: null,
      result: null,
      steps: DEFAULT_STEPS.map((s) => ({ ...s })),
      logEntries: [],
      telemetrySamples: [],
      backendLog: [],
      blockedQuestion: null,
      visibilityMap: null,
      runParams: params,
      startedAt: Date.now(),
      endedAt: null,
    }),

  applyStep: ({ index, status, detail, durationMs }) =>
    set((state) => {
      const steps = state.steps.slice();
      const current = steps[index];
      if (!current) return state;
      steps[index] = {
        ...current,
        status,
        detail: detail ?? current.detail,
        durationMs: durationMs ?? current.durationMs,
      };
      return { steps };
    }),

  finishRun: (result) => set({ isRunning: false, result, endedAt: Date.now() }),
  failRun: (message) => set({ isRunning: false, error: message, endedAt: Date.now() }),
  reset: () =>
    set({
      runId: null,
      isRunning: false,
      error: null,
      result: null,
      steps: DEFAULT_STEPS.map((s) => ({ ...s })),
      logEntries: [],
      telemetrySamples: [],
      backendLog: [],
      blockedQuestion: null,
      visibilityMap: null,
      runParams: null,
      startedAt: null,
      endedAt: null,
    }),
  setLogEntries: (entries) => set({ logEntries: entries }),
  pushTelemetry: (sample) =>
    set((state) => {
      const next = state.telemetrySamples.concat(sample);
      if (next.length > TELEMETRY_CAP) {
        next.splice(0, next.length - TELEMETRY_CAP);
      }
      return { telemetrySamples: next };
    }),
  pushLogLine: (line) =>
    set((state) => {
      const next = state.backendLog.concat(line);
      if (next.length > LOG_CAP) {
        next.splice(0, next.length - LOG_CAP);
      }
      return { backendLog: next };
    }),
  setBlockedQuestion: (q) => set({ blockedQuestion: q }),
  setVisibilityMap: (map) => set({ visibilityMap: map }),
}));
