/**
 * Thin wrapper around the Tauri 2 IPC. Every call goes straight to the Rust
 * backend via `invoke` / `listen` — there is no browser-mode fallback. The UI
 * is always shipped inside Tauri; running pure vite (`make ui`) will fail any
 * call that touches the backend, by design.
 *
 * Why no mocks: the scan flow is end-to-end agent-driven. Fake step timers
 * shadow the real `agent::workflow` event stream and let regressions hide.
 * If you need to iterate on layout without the backend, work in components
 * that don't touch this module.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(cmd, args);
}

async function listen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
  return tauriListen<T>(event, (e) => cb(e.payload));
}

// ---------- Run timeline events ----------

type StepStatus = "pending" | "active" | "done" | "error";

export interface StepUpdate {
  runId: string;
  index: number;
  status: StepStatus;
  detail?: string;
  durationMs?: number;
}

export interface RunComplete {
  runId: string;
  issuesFound: number;
  criticalCount: number;
}

export interface RunError {
  runId: string;
  message: string;
}

export async function selectProjectPath(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: true, multiple: false, title: "Choose project" });
  if (result === null) return null;
  return Array.isArray(result) ? (result[0] ?? null) : result;
}

// ---------- Agent-driven scan ----------

/** Permission mode for the iterative agent. `auto` runs every tool without
 *  prompting (the user has explicitly opted in by starting a scan); `default`
 *  asks for approval before destructive steps; `read_only` refuses them. */
export type AgentMode = "auto" | "default" | "read_only";

export interface PromptPreset {
  label: string;
  prompt: string;
}

/** Server-side list of canned scan goals. UI augments with an Other slot
 *  that submits free text as `goalPrompt`. */
export async function listPromptPresets(): Promise<PromptPreset[]> {
  return invoke<PromptPreset[]>("list_prompt_presets");
}

/**
 * Start an agent-driven scan. Returns the `run_id` immediately; the agent
 * loop runs in a Tokio task and emits `run://step`, `run://complete`, and
 * `run://error` events tagged with that id. This split — POST-and-return,
 * events on a separate channel — mirrors goosed's session-event bus.
 *
 * `goalPrompt` is `undefined` to use the default recipe prompt (see
 * `agent::workflow::default_goal_prompt`); pass a string from
 * `listPromptPresets` or free text for an "Other" goal.
 */
export async function startAgentRun(
  projectPath: string,
  options: { mode?: AgentMode; goalPrompt?: string } = {},
): Promise<string> {
  return invoke<string>("start_agent_run", {
    projectPath,
    mode: options.mode ?? "auto",
    goalPrompt: options.goalPrompt ?? null,
  });
}

export async function onStepUpdate(cb: (u: StepUpdate) => void): Promise<() => void> {
  return listen<StepUpdate>("run://step", cb);
}

export async function onRunComplete(cb: (c: RunComplete) => void): Promise<() => void> {
  return listen<RunComplete>("run://complete", cb);
}

export async function onRunError(cb: (e: RunError) => void): Promise<() => void> {
  return listen<RunError>("run://error", cb);
}

// ---------- Telemetry + visibility map ----------

/** One container snapshot from the Rust-side `docker stats` poller. Counters
 *  are absolute (cumulative bytes); the UI derives bytes/sec by diffing
 *  successive samples in the same series. */
export interface TelemetrySample {
  runId: string;
  tsMs: number;
  containerId: string;
  cpuPct: number;
  memMb: number;
  memPct: number;
  netRxBytes: number;
  netTxBytes: number;
  blockReadBytes: number;
  blockWriteBytes: number;
}

/** One ranked hotspot from `analyze_samples`. Mirrors the Rust `Issue` struct
 *  in `tools/analyze_samples.rs` — keep in sync. */
export type IssueCategory =
  | "database"
  | "network"
  | "cpu"
  | "lock"
  | "gc"
  | "serde"
  | "filesystem"
  | "unknown";

export type IssueSeverity = "critical" | "high" | "medium" | "low";

export interface Issue {
  rank: number;
  function: string;
  category: IssueCategory;
  severity: IssueSeverity;
  self_pct: number;
  total_pct: number;
  samples: number;
  example_stack: string;
}

/** End-of-run summary the backend builds from `analyze_samples` + one LLM
 *  synthesis turn for `architecture_advice`. */
export interface VisibilityMap {
  critical: Issue[];
  warnings: Issue[];
  estimatedCpuReductionPct: number;
  architectureAdvice: string[];
}

export interface RunReport {
  runId: string;
  map: VisibilityMap;
}

export async function onTelemetrySample(
  cb: (sample: TelemetrySample) => void,
): Promise<() => void> {
  return listen<TelemetrySample>("agent:telemetry", cb);
}

export async function onRunReport(cb: (report: RunReport) => void): Promise<() => void> {
  return listen<RunReport>("run://report", cb);
}

/** One formatted line out of the Rust `tracing` pipeline. Mirrors what's
 *  printed to stderr — the in-app log pane renders this so you don't need a
 *  terminal to see what the backend is up to. */
export interface LogLine {
  tsMs: number;
  level: string;
  target: string;
  message: string;
}

export async function onLogLine(cb: (line: LogLine) => void): Promise<() => void> {
  return listen<LogLine>("agent:log", cb);
}

// ---------- Blocked-on-user question ----------

/** The agent called `ask_user` and is parked waiting for a human answer.
 *  The UI shows a BlockedModal with `question`; the user's reply flows back
 *  through {@link answerBlockedQuestion}. */
export interface BlockedQuestion {
  id: string;
  question: string;
}

export async function onBlockedQuestion(
  cb: (q: BlockedQuestion) => void,
): Promise<() => void> {
  return listen<BlockedQuestion>("agent:blocked", cb);
}

/** Deliver the operator's answer to the in-flight blocked question. The
 *  agent loop resumes with `answer` as the tool's content. Throws if no
 *  question is pending. */
export async function answerBlockedQuestion(answer: string): Promise<void> {
  return invoke<void>("answer_blocked_question", { answer });
}

/**
 * Streaming `AgentEvent` mirror — the Rust workflow forwards every event the
 * iterative loop produced (text deltas, tool dispatch / completion, usage,
 * errors) so the UI can render a "live reasoning + tool log" panel.
 *
 * Variants match `crate::agent::agent_loop::AgentEvent` (serde tag = "kind",
 * snake_case). Keep this union in sync with the Rust enum — it ships over
 * the wire as JSON, so a missing variant becomes an `unknown` at runtime.
 */
export type AgentEvent =
  | { kind: "text_delta"; text: string }
  | { kind: "assistant_message"; message: unknown }
  | {
      kind: "tool_dispatched";
      id: string;
      name: string;
      arguments: unknown;
    }
  | { kind: "tool_completed"; id: string; content: string; is_error: boolean }
  | {
      kind: "tool_needs_approval";
      id: string;
      name: string;
      arguments: unknown;
    }
  | { kind: "usage"; [extra: string]: unknown }
  | { kind: "turn_budget_exceeded"; max_turns: number }
  | { kind: "error"; message: string }
  | { kind: "done" };

export async function onAgentEvent(
  cb: (event: AgentEvent) => void,
): Promise<() => void> {
  return listen<AgentEvent>("agent:event", cb);
}

// ---------- LLM agent backend ----------

/** A saved backend config — one of two wire protocols. Mirrors
 *  `crate::model_config::ModelBackend` (serde tag = "mode", lowercase).
 *
 *  - `api`: OpenAI-compatible HTTP. Covers cloud OpenAI, Groq, OpenRouter,
 *    Azure, and every local runtime (Ollama, LM Studio, Docker Model
 *    Runner, vLLM, llama-server). Local providers are discovered via
 *    {@link probeLocalRuntimes} and saved with `api_key: "not-needed"`.
 *  - `anthropic`: Claude's native `/v1/messages` shape. Distinct because
 *    auth uses `x-api-key` instead of `Authorization: Bearer`, `max_tokens`
 *    is required, and the SSE wire format differs in load-bearing ways.
 *    Provider impl: `agent::anthropic::ClaudeProvider`. */
export type ModelBackendConfig =
  | {
      mode: "api";
      base_url: string;
      api_key: string;
      model: string;
    }
  | {
      mode: "anthropic";
      base_url: string;
      api_key: string;
      model: string;
    };

export async function configureBackend(config: ModelBackendConfig): Promise<void> {
  return invoke<void>("configure_backend", { config });
}

/** Persist the config without resolving (download / spawn happens on first chat). */
export async function saveBackendConfig(config: ModelBackendConfig): Promise<void> {
  return invoke<void>("save_backend_config", { config });
}

export async function loadBackendConfig(): Promise<ModelBackendConfig | null> {
  return invoke<ModelBackendConfig | null>("load_backend_config");
}

export async function clearBackend(): Promise<void> {
  return invoke<void>("clear_backend");
}

/**
 * Backend lifecycle status. Mirrors the `BackendStatus` enum in `events.rs`.
 * `kind` is the discriminator; remaining fields depend on the variant.
 */
export type BackendStatus =
  | { kind: "unconfigured" }
  | { kind: "idle"; mode: string; model: string }
  | { kind: "starting" }
  | { kind: "ready"; mode: string; model: string }
  | { kind: "error"; message: string };

export async function getBackendStatus(): Promise<BackendStatus> {
  return invoke<BackendStatus>("get_backend_status");
}

export async function onBackendStatus(
  cb: (status: BackendStatus) => void,
): Promise<() => void> {
  return listen<BackendStatus>("backend:status", cb);
}

/**
 * Stream a chat message. Tokens arrive via `chat:token` events; completion via
 * `chat:done`; errors via `chat:error`. Returns once the request is queued.
 */
export async function chat(message: string, preamble?: string): Promise<void> {
  return invoke<void>("chat", { message, preamble });
}

/** Non-streaming variant — returns the full response as a string. */
export async function chatOneshot(message: string, preamble?: string): Promise<string> {
  return invoke<string>("chat_oneshot", { message, preamble });
}

export async function onChatToken(cb: (token: string) => void): Promise<() => void> {
  return listen<string>("chat:token", cb);
}

export async function onChatDone(cb: () => void): Promise<() => void> {
  return listen<unknown>("chat:done", () => cb());
}

export async function onChatError(cb: (msg: string) => void): Promise<() => void> {
  return listen<string>("chat:error", cb);
}

// ---------- Multi-provider config (Phase 1.5) ----------

/** Wire protocol a preset targets. Forwarded into the saved
 *  `ModelBackendConfig.mode`. `api` for everything OpenAI-compatible,
 *  `anthropic` for Claude (its `/v1/messages` shape is incompatible enough
 *  to need its own path). */
export type PresetMode = "api" | "anthropic";

export interface ProviderPreset {
  id: string;
  name: string;
  baseUrl: string;
  /** May be empty for local OpenAI-compatible endpoints — call
   *  {@link listModelsFromEndpoint} to populate. */
  models: string[];
  apiKeyUrl: string;
  /** `false` for local providers (Ollama, Docker Model Runner, LM Studio).
   *  When false, the Add Provider form hides the key input and submits
   *  `not-needed` automatically. */
  requiresApiKey: boolean;
  /** One-line copy explaining how to install/start this provider. */
  description: string;
  /** Which wire protocol this preset speaks. The Onboarding /
   *  AddProviderForm flows read this to choose the `mode` they send to
   *  `save_provider` / `test_provider`. Defaults to `api`. */
  mode: PresetMode;
}

export interface SavedProvider {
  id: string;
  name: string;
  config: ModelBackendConfig;
  createdAt: number;
}

export interface AppConfig {
  onboardingComplete: boolean;
  activeProviderId: string | null;
  providers: SavedProvider[];
  /** User-toggleable scan-walker filters. The Rust side serde-defaults this
   *  when missing from older config files; the field always exists at
   *  runtime, but is marked optional for forward-compat across future Rust
   *  rollbacks. */
  scanFilters: ScanFilters;
}

export interface ScanFilters {
  /** Skip directories named `static` / `assets` during static analysis.
   *  Default true — these almost always hold vendored minified JS that
   *  dominates the entry-point picker with synthetic top callers. Users
   *  analyzing a project where these dirs hold real source can disable. */
  excludeStaticAssets: boolean;
  /** Drop test/spec/mock files at the walker stage. Default true — a
   *  heavy bundled test file (e.g. a vite-built `*.test.js`) can otherwise
   *  flip the linguist breakdown to the wrong language and starve the
   *  entry-point picker. Mirrors `make scan-prompt`'s default behavior. */
  excludeTests: boolean;
}

/** One local runtime detected on the user's machine via `probeLocalRuntimes`.
 *  Plug-and-play: whichever runtime (Ollama, LM Studio, Docker Model Runner)
 *  the user already has installed shows up here. */
export interface DiscoveredRuntime {
  presetId: string;
  name: string;
  baseUrl: string;
  models: string[];
  /** Set when models were detected via a sidechannel (e.g. `docker model list`)
   *  but the HTTP endpoint isn't reachable. UI should disable activation and
   *  show this string. */
  note?: string;
}

export async function listPresets(): Promise<ProviderPreset[]> {
  return invoke<ProviderPreset[]>("list_presets");
}

/** Probe every curated local OpenAI-compatible runtime in parallel with a
 *  hard 800ms per-runtime timeout. Returns only the ones that responded.
 *  Also writes the result to the SQLite cache. */
export async function probeLocalRuntimes(): Promise<DiscoveredRuntime[]> {
  return invoke<DiscoveredRuntime[]>("probe_local_runtimes");
}

/** Read last-known runtimes from the SQLite cache without doing a network
 *  probe. Call this on mount for an instant first paint; then call
 *  {@link probeLocalRuntimes} in the background to refresh. */
export async function cachedLocalRuntimes(): Promise<DiscoveredRuntime[]> {
  return invoke<DiscoveredRuntime[]>("cached_local_runtimes");
}

export async function getAppConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_app_config");
}

/** Persist a new ScanFilters block. Returns the saved value so the UI can
 *  reconcile against the canonical Rust-side state in case any field was
 *  normalized server-side. */
export async function updateScanFilters(
  filters: ScanFilters,
): Promise<ScanFilters> {
  return invoke<ScanFilters>("update_scan_filters", { filters });
}

export async function testProvider(config: ModelBackendConfig): Promise<void> {
  return invoke<void>("test_provider", { config });
}

export async function saveProvider(
  name: string,
  config: ModelBackendConfig,
  activate: boolean,
): Promise<SavedProvider> {
  return invoke<SavedProvider>("save_provider", { name, config, activate });
}

export async function activateProvider(id: string): Promise<void> {
  return invoke<void>("activate_provider", { id });
}

export async function deleteProvider(id: string): Promise<void> {
  return invoke<void>("delete_provider", { id });
}

export async function resetAllConfig(): Promise<void> {
  return invoke<void>("reset_all_config");
}

// ---------- Generic helpers ----------

/**
 * Race a promise against a deadline. On timeout, throws `Error("timed out
 * after Xms")` — match against `/timed out|timeout/i` to route in the UI.
 *
 * Used to guard `checkForUpdate()` so the UI never sits on "Checking…" if
 * Tauri's reqwest is hung against an unreachable / placeholder endpoint.
 */
export function withTimeout<T>(p: Promise<T>, ms: number, label = "operation"): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms),
    ),
  ]);
}

// ---------- Live model discovery ----------

/** Probe an OpenAI-compatible endpoint (cloud or local) for its model list.
 *  Throws if unreachable or non-OpenAI-shaped response. */
export async function listModelsFromEndpoint(
  baseUrl: string,
  apiKey?: string,
): Promise<string[]> {
  return invoke<string[]>("list_models_from_endpoint", {
    baseUrl,
    apiKey: apiKey || null,
  });
}

// ---------- Auto-update ----------

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes?: string;
  date?: string;
}

export type UpdateProgress =
  | { kind: "started"; contentLength?: number }
  | { kind: "progress"; downloaded: number; contentLength?: number }
  | { kind: "finished" };

/**
 * Check the configured updater endpoint. Returns the available update or null.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  const { check } = await import("@tauri-apps/plugin-updater");
  const update = await check();
  if (!update || !update.available) return null;
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    notes: update.body ?? undefined,
    date: update.date ?? undefined,
  };
}

/**
 * Download and install the latest update, streaming progress events, then
 * relaunch the app. Throws if no update is available — call `checkForUpdate`
 * first.
 */
export async function downloadAndInstallUpdate(
  onProgress?: (p: UpdateProgress) => void,
): Promise<void> {
  const { check } = await import("@tauri-apps/plugin-updater");
  const update = await check();
  if (!update || !update.available) throw new Error("no update available");

  let total: number | undefined;
  let received = 0;

  await update.downloadAndInstall((event) => {
    if (!onProgress) return;
    if (event.event === "Started") {
      total = event.data.contentLength ?? undefined;
      received = 0;
      onProgress({ kind: "started", contentLength: total });
    } else if (event.event === "Progress") {
      received += event.data.chunkLength;
      onProgress({ kind: "progress", downloaded: received, contentLength: total });
    } else if (event.event === "Finished") {
      onProgress({ kind: "finished" });
    }
  });

  const { relaunch } = await import("@tauri-apps/plugin-process");
  await relaunch();
}

/** App version pulled from the Tauri config — no network round-trip. */
export async function getAppVersion(): Promise<string> {
  const { getVersion } = await import("@tauri-apps/api/app");
  return getVersion();
}

// ---------- Conversations + cancel (Phase 3 + 4) ----------

/**
 * `rig::message::Message` is a complex tagged union (text, tool calls,
 * reasoning blocks, etc.). The UI rarely cares — just read `role` and the
 * extracted text via `messageText()`.
 */
export type ChatMessage = unknown;

export interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  updatedAt: number;
}

export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: number;
  messageCount: number;
}

export async function listConversations(): Promise<ConversationSummary[]> {
  return invoke<ConversationSummary[]>("list_conversations");
}

export async function loadConversation(id: string): Promise<Conversation> {
  return invoke<Conversation>("load_conversation", { id });
}

export async function newConversation(): Promise<void> {
  return invoke<void>("new_conversation");
}

export async function deleteConversation(id: string): Promise<void> {
  return invoke<void>("delete_conversation", { id });
}

export async function getCurrentConversation(): Promise<Conversation | null> {
  return invoke<Conversation | null>("get_current_conversation");
}

export async function cancelChat(): Promise<void> {
  return invoke<void>("cancel_chat");
}

// ---------- Static scan (drift-static-profiler) ----------
//
// Two-step flow: `startStaticScan` runs root discovery, emits a stream of
// `scan://progress` events, then fires `scan://entries-ready` with the
// top-10 entry roots. The user picks one; the UI calls
// `selectEntryAndScan(scanId, rootIndex)` and the same task wakes up to
// build the focused report. On success a `scan://complete` event arrives
// with the path of the saved JSON.

export type ScanProgress =
  | { kind: "walk_start"; scanId: string }
  | { kind: "walk_progress"; scanId: string; filesSeen: number }
  | { kind: "walk_end"; scanId: string; totalFiles: number; bytes: number }
  | { kind: "parse_start"; scanId: string; totalSourceFiles: number }
  | { kind: "parse_progress"; scanId: string; done: number; total: number; current: string | null }
  | { kind: "phase"; scanId: string; name: string }
  | { kind: "step_start"; scanId: string; label: string; total: number }
  | { kind: "step_progress"; scanId: string; label: string; done: number; total: number; current: string | null };

export interface ScanPickerCaller {
  name: string;
  file: string;
  line: number;
}

export interface ScanPickerRoot {
  index: number;
  name: string;
  file: string;
  line: number;
  reach: number;
  callers: ScanPickerCaller[];
}

export interface ScanEntriesReady {
  scanId: string;
  roots: ScanPickerRoot[];
}

export interface ScanComplete {
  scanId: string;
  savedPath: string;
  pickedRoot: string | null;
}

export interface ScanErrorPayload {
  scanId: string;
  message: string;
}

/** Row metadata, fired *before* the LLM stream opens. The UI uses this to
 *  render an empty suggestion row with a streaming spinner, so the user sees
 *  the badges + file:line immediately — same UX as a ChatGPT bubble that
 *  appears before any text. */
export interface ScanSuggestionStartPayload {
  scanId: string;
  index: number;
  source: "immediate_fix" | "refactor_candidate" | "finding_top";
  kind: string;
  severity: string;
  file: string;
  line: number;
  name: string;
}

/** One text fragment from the provider stream. Append to the row's body —
 *  the backend guarantees fragments are sequential and non-overlapping. */
export interface ScanSuggestionDeltaPayload {
  scanId: string;
  index: number;
  delta: string;
}

/** Final settled body for one finding. The frontend uses this both to mark
 *  the row no-longer-streaming and to reconcile its delta accumulator (Tauri
 *  events are best-effort — reconciliation guarantees the row ends with the
 *  exact text the backend captured). */
export interface ScanSuggestionPayload {
  scanId: string;
  index: number;
  source: "immediate_fix" | "refactor_candidate" | "finding_top";
  kind: string;
  severity: string;
  file: string;
  line: number;
  name: string;
  suggestion: string;
}

export interface ScanSuggestionDone {
  scanId: string;
  total: number;
  failed: number;
}

/** Saved-scan envelope returned by `loadStaticScan`. The `report` mirrors
 *  the JSON the static-profiler CLI writes — same shape, so the same
 *  summary components render it identically. */
export interface StoredScan {
  scanId: string;
  savedAt: string;
  /// Typed loosely here; the summary components in
  /// `components/scan-summary` declare the concrete shape they need. */
  report: unknown;
}

export interface ScanMeta {
  scanId: string;
  savedAt: string;
  sourceRoot: string | null;
  profiledLanguage: string | null;
  files: number;
  symbols: number;
  findingsTotal: number;
}

export async function startStaticScan(projectPath: string): Promise<string> {
  return invoke<string>("start_static_scan", { projectPath });
}

export async function selectEntryAndScan(
  scanId: string,
  rootIndex: number | null,
): Promise<void> {
  return invoke<void>("select_entry_and_scan", { scanId, rootIndex });
}

export async function listStaticScans(): Promise<ScanMeta[]> {
  return invoke<ScanMeta[]>("list_static_scans");
}

export async function loadStaticScan(scanId: string): Promise<StoredScan> {
  return invoke<StoredScan>("load_static_scan", { scanId });
}

/** One row in the canonical (dedupe + ranked + truncated) finding list the
 *  suggester would iterate over. The array position is the stable index —
 *  pass it straight into {@link startScanFindingSuggestion} to ask the LLM
 *  to study that exact row. */
export interface ListedFinding {
  source: "immediate_fix" | "refactor_candidate" | "finding_top";
  kind: string;
  severity: string;
  name: string;
  file: string;
  line: number;
  message: string;
}

export async function listScanFindings(scanId: string): Promise<ListedFinding[]> {
  return invoke<ListedFinding[]>("list_scan_findings", { scanId });
}

/** Kick off a single-finding LLM stream. The same `scan://suggestion-{start,
 *  delta,done}` events fire — keyed on `index` so existing handlers reconcile
 *  the right row. Idempotent for `(scanId, index)`: a second call while the
 *  first is still streaming is a silent no-op. */
export async function startScanFindingSuggestion(
  scanId: string,
  index: number,
): Promise<void> {
  return invoke<void>("start_scan_finding_suggestion", { scanId, index });
}

export async function stopScanFindingSuggestion(
  scanId: string,
  index: number,
): Promise<boolean> {
  return invoke<boolean>("stop_scan_finding_suggestion", { scanId, index });
}

export async function onScanProgress(
  cb: (p: ScanProgress) => void,
): Promise<() => void> {
  return listen<ScanProgress>("scan://progress", cb);
}

export async function onScanEntriesReady(
  cb: (p: ScanEntriesReady) => void,
): Promise<() => void> {
  return listen<ScanEntriesReady>("scan://entries-ready", cb);
}

export async function onScanComplete(
  cb: (p: ScanComplete) => void,
): Promise<() => void> {
  return listen<ScanComplete>("scan://complete", cb);
}

export async function onScanError(
  cb: (p: ScanErrorPayload) => void,
): Promise<() => void> {
  return listen<ScanErrorPayload>("scan://error", cb);
}

export async function onScanSuggestionStart(
  cb: (p: ScanSuggestionStartPayload) => void,
): Promise<() => void> {
  return listen<ScanSuggestionStartPayload>("scan://suggestion-start", cb);
}

export async function onScanSuggestionDelta(
  cb: (p: ScanSuggestionDeltaPayload) => void,
): Promise<() => void> {
  return listen<ScanSuggestionDeltaPayload>("scan://suggestion-delta", cb);
}

export async function onScanSuggestion(
  cb: (p: ScanSuggestionPayload) => void,
): Promise<() => void> {
  return listen<ScanSuggestionPayload>("scan://suggestion", cb);
}

export async function onScanSuggestionDone(
  cb: (p: ScanSuggestionDone) => void,
): Promise<() => void> {
  return listen<ScanSuggestionDone>("scan://suggestion-done", cb);
}

export async function onChatCancelled(cb: () => void): Promise<() => void> {
  return listen<unknown>("chat:cancelled", () => cb());
}

/**
 * Best-effort text extraction from a `rig::message::Message`. The shape
 * differs across rig versions, so we navigate defensively. Returns the
 * concatenated `text` fields of any `Text` content blocks.
 */
export function messageText(m: ChatMessage): string {
  const obj = m as { role?: string; content?: unknown };
  const content = obj.content;
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((c) => {
        if (typeof c === "string") return c;
        const block = c as { text?: string; type?: string };
        return block.text ?? "";
      })
      .join("");
  }
  return "";
}

export function messageRole(m: ChatMessage): string {
  return (m as { role?: string }).role ?? "unknown";
}
