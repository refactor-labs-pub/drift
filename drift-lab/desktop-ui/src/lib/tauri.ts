/**
 * Thin wrapper around the Tauri 2 API. When running in a plain browser (e.g.
 * `npm run dev` outside of `tauri dev`), every command falls back to a local
 * mock so the UI is fully exercisable without the Rust side built.
 */

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

export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ---------- Tauri-backed implementation ----------
async function realInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

async function realListen<T>(
  event: string,
  cb: (payload: T) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<T>(event, (e) => cb(e.payload));
  return unlisten;
}

// ---------- Mock implementation (browser dev mode) ----------
const MOCK_STEPS: { detail: string; doneDetail: string; duration: number }[] = [
  { detail: "Scanning project for Dockerfile…",       doneDetail: "Found checkout-service:latest (247 MB)", duration: 1200 },
  { detail: "Inspecting image layers…",                doneDetail: "Python 3.11 · FastAPI · uvicorn",         duration: 1400 },
  { detail: "Injecting py-spy into container…",        doneDetail: "py-spy v0.3.14 installed",                duration: 1700 },
  { detail: "Driving load · 50 RPS for 60s…",          doneDetail: "3,047 samples captured",                  duration: 2400 },
  { detail: "Building flame graph & ranking issues…",  doneDetail: "7 issues detected",                       duration: 1400 },
];

type Listener<T> = (p: T) => void;
const mockListeners: Record<string, Set<Listener<unknown>>> = {};

function mockEmit<T>(event: string, payload: T) {
  mockListeners[event]?.forEach((cb) => cb(payload as unknown));
}

function mockListen<T>(event: string, cb: (payload: T) => void): () => void {
  const set = (mockListeners[event] ??= new Set());
  const wrapped: Listener<unknown> = (p) => cb(p as T);
  set.add(wrapped);
  return () => set.delete(wrapped);
}

async function mockStartRun(_path: string): Promise<string> {
  const runId = crypto.randomUUID();
  // Fire steps with the same cadence as the example.
  let cumulative = 350;
  MOCK_STEPS.forEach((s, index) => {
    setTimeout(
      () => mockEmit<StepUpdate>("run://step", { runId, index, status: "active", detail: s.detail }),
      cumulative,
    );
    cumulative += s.duration;
    setTimeout(
      () => mockEmit<StepUpdate>("run://step", { runId, index, status: "done", detail: s.doneDetail, durationMs: s.duration }),
      cumulative,
    );
  });
  setTimeout(
    () => mockEmit<RunComplete>("run://complete", { runId, issuesFound: 7, criticalCount: 3 }),
    cumulative + 600,
  );
  return runId;
}

async function mockSelectPath(): Promise<string | null> {
  // No native dialog in browser; just echo a fake path.
  return "/Users/jdoe/projects/checkout-service";
}

// ---------- Public surface ----------
export async function selectProjectPath(): Promise<string | null> {
  if (!isTauri()) return mockSelectPath();
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: true, multiple: false, title: "Choose project" });
  if (result === null) return null;
  return Array.isArray(result) ? (result[0] ?? null) : result;
}

export async function startRun(projectPath: string): Promise<string> {
  if (!isTauri()) return mockStartRun(projectPath);
  return realInvoke<string>("start_run", { projectPath });
}

export async function onStepUpdate(cb: (u: StepUpdate) => void): Promise<() => void> {
  if (!isTauri()) return mockListen("run://step", cb);
  return realListen<StepUpdate>("run://step", cb);
}

export async function onRunComplete(cb: (c: RunComplete) => void): Promise<() => void> {
  if (!isTauri()) return mockListen("run://complete", cb);
  return realListen<RunComplete>("run://complete", cb);
}

export async function onRunError(cb: (e: RunError) => void): Promise<() => void> {
  if (!isTauri()) return mockListen("run://error", cb);
  return realListen<RunError>("run://error", cb);
}

// ---------- LLM agent backend ----------

export type ModelBackendConfig =
  | {
      mode: "api";
      base_url: string;
      api_key: string;
      model: string;
    }
  | {
      mode: "local";
      /** `repo_id:quant`, e.g. `unsloth/gemma-3-1b-it-GGUF:Q4_K_M`. */
      spec: string;
      port: number;
    };

export async function configureBackend(config: ModelBackendConfig): Promise<void> {
  if (!isTauri()) {
    mockSavedConfig = config;
    mockBackendConfigured = true;
    mockSetStatus(
      config.mode === "api"
        ? { kind: "ready", mode: "api", model: config.model }
        : { kind: "ready", mode: "local", model: config.spec },
    );
    return;
  }
  return realInvoke<void>("configure_backend", { config });
}

/** Persist the config without resolving (download / spawn happens on first chat). */
export async function saveBackendConfig(config: ModelBackendConfig): Promise<void> {
  if (!isTauri()) {
    mockSavedConfig = config;
    mockBackendConfigured = true;
    mockSetStatus(
      config.mode === "api"
        ? { kind: "idle", mode: "api", model: config.model }
        : { kind: "idle", mode: "local", model: config.spec },
    );
    return;
  }
  return realInvoke<void>("save_backend_config", { config });
}

export async function loadBackendConfig(): Promise<ModelBackendConfig | null> {
  if (!isTauri()) return mockSavedConfig;
  return realInvoke<ModelBackendConfig | null>("load_backend_config");
}

export async function clearBackend(): Promise<void> {
  if (!isTauri()) {
    mockSavedConfig = null;
    mockBackendConfigured = false;
    mockSetStatus({ kind: "unconfigured" });
    return;
  }
  return realInvoke<void>("clear_backend");
}

/**
 * Backend lifecycle status. Mirrors the `BackendStatus` enum in `events.rs`.
 * `kind` is the discriminator; remaining fields depend on the variant.
 */
export type BackendStatus =
  | { kind: "unconfigured" }
  | { kind: "idle"; mode: string; model: string }
  | { kind: "downloading"; file: string }
  | { kind: "starting" }
  | { kind: "ready"; mode: string; model: string }
  | { kind: "error"; message: string };

export async function getBackendStatus(): Promise<BackendStatus> {
  if (!isTauri()) return mockStatus;
  return realInvoke<BackendStatus>("get_backend_status");
}

export async function onBackendStatus(
  cb: (status: BackendStatus) => void,
): Promise<() => void> {
  if (!isTauri()) return mockListen("backend:status", cb);
  return realListen<BackendStatus>("backend:status", cb);
}

/**
 * Stream a chat message. Tokens arrive via `chat:token` events; completion via
 * `chat:done`; errors via `chat:error`. Returns once the request is queued.
 */
export async function chat(message: string, preamble?: string): Promise<void> {
  if (!isTauri()) return mockChat(message);
  return realInvoke<void>("chat", { message, preamble });
}

/** Non-streaming variant — returns the full response as a string. */
export async function chatOneshot(message: string, preamble?: string): Promise<string> {
  if (!isTauri()) return mockChatOneshot(message);
  return realInvoke<string>("chat_oneshot", { message, preamble });
}

export async function onChatToken(cb: (token: string) => void): Promise<() => void> {
  if (!isTauri()) return mockListen("chat:token", cb);
  return realListen<string>("chat:token", cb);
}

export async function onChatDone(cb: () => void): Promise<() => void> {
  if (!isTauri()) return mockListen("chat:done", () => cb());
  return realListen<unknown>("chat:done", () => cb());
}

export async function onChatError(cb: (msg: string) => void): Promise<() => void> {
  if (!isTauri()) return mockListen("chat:error", cb);
  return realListen<string>("chat:error", cb);
}

// ---------- Mock chat (browser dev) ----------
let mockBackendConfigured = false;
let mockSavedConfig: ModelBackendConfig | null = null;
let mockStatus: BackendStatus = { kind: "unconfigured" };

function mockSetStatus(s: BackendStatus) {
  mockStatus = s;
  mockEmit("backend:status", s);
}

async function mockChat(message: string): Promise<void> {
  if (!mockBackendConfigured) {
    setTimeout(() => mockEmit("chat:error", "backend not configured (mock)"), 50);
    setTimeout(() => mockEmit("chat:done", null), 60);
    return;
  }
  const reply = `(mock) you said: ${message}`;
  let cumulative = 50;
  for (const word of reply.split(" ")) {
    setTimeout(() => mockEmit("chat:token", word + " "), cumulative);
    cumulative += 80;
  }
  setTimeout(() => mockEmit("chat:done", null), cumulative + 50);
}

async function mockChatOneshot(message: string): Promise<string> {
  if (!mockBackendConfigured) throw new Error("backend not configured (mock)");
  return `(mock) you said: ${message}`;
}

// ---------- Multi-provider config (Phase 1.5) ----------

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
}

export interface LocalModelPreset {
  spec: string;
  name: string;
  sizeGb: number;
  description: string;
  tags: string[];
}

export interface HfModelHit {
  repoId: string;
  author: string | null;
  downloads: number;
  likes: number;
  lastModified: string | null;
  tags: string[];
}

export interface HfQuantFile {
  filename: string;
  /** e.g. `Q4_K_M`, `IQ3_M`, `F16`. `null` if not parseable. */
  quant: string | null;
  /** Bytes, when HuggingFace reports it. */
  size: number | null;
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
}

export async function listPresets(): Promise<ProviderPreset[]> {
  if (!isTauri()) return MOCK_PRESETS;
  return realInvoke<ProviderPreset[]>("list_presets");
}

export async function listLocalPresets(): Promise<LocalModelPreset[]> {
  if (!isTauri()) return MOCK_LOCAL_PRESETS;
  return realInvoke<LocalModelPreset[]>("list_local_presets");
}

export async function getAppConfig(): Promise<AppConfig> {
  if (!isTauri()) return mockAppConfig;
  return realInvoke<AppConfig>("get_app_config");
}

export async function testProvider(config: ModelBackendConfig): Promise<void> {
  if (!isTauri()) return;
  return realInvoke<void>("test_provider", { config });
}

export async function saveProvider(
  name: string,
  config: ModelBackendConfig,
  activate: boolean,
): Promise<SavedProvider> {
  if (!isTauri()) {
    const provider: SavedProvider = {
      id: crypto.randomUUID(),
      name,
      config,
      createdAt: Math.floor(Date.now() / 1000),
    };
    mockAppConfig.providers.push(provider);
    if (activate) {
      mockAppConfig.activeProviderId = provider.id;
      mockAppConfig.onboardingComplete = true;
    }
    return provider;
  }
  return realInvoke<SavedProvider>("save_provider", { name, config, activate });
}

export async function activateProvider(id: string): Promise<void> {
  if (!isTauri()) {
    mockAppConfig.activeProviderId = id;
    return;
  }
  return realInvoke<void>("activate_provider", { id });
}

export async function deleteProvider(id: string): Promise<void> {
  if (!isTauri()) {
    mockAppConfig.providers = mockAppConfig.providers.filter((p) => p.id !== id);
    if (mockAppConfig.activeProviderId === id) mockAppConfig.activeProviderId = null;
    return;
  }
  return realInvoke<void>("delete_provider", { id });
}

export async function resetAllConfig(): Promise<void> {
  if (!isTauri()) {
    mockAppConfig = {
      onboardingComplete: false,
      activeProviderId: null,
      providers: [],
    };
    return;
  }
  return realInvoke<void>("reset_all_config");
}

// Mocks for browser dev mode
const MOCK_PRESETS: ProviderPreset[] = [
  {
    id: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    models: ["gpt-4o", "gpt-4o-mini"],
    apiKeyUrl: "https://platform.openai.com/api-keys",
    requiresApiKey: true,
    description: "OpenAI's hosted API. Bring your own key.",
  },
  {
    id: "ollama",
    name: "Ollama (local)",
    baseUrl: "http://localhost:11434/v1",
    models: [],
    apiKeyUrl: "https://ollama.com",
    requiresApiKey: false,
    description: "Runs models on this machine.",
  },
  {
    id: "custom",
    name: "Custom (OpenAI-compatible)",
    baseUrl: "http://localhost:8080/v1",
    models: [],
    apiKeyUrl: "",
    requiresApiKey: false,
    description: "Any OpenAI-compatible HTTP endpoint.",
  },
];
const MOCK_LOCAL_PRESETS: LocalModelPreset[] = [
  {
    spec: "unsloth/gemma-4-26B-A4B-it-GGUF:Q4_K_M",
    name: "Gemma 4 26B A4B",
    sizeGb: 15.8,
    description: "Mixture-of-experts. Strong general + vision.",
    tags: ["Recommended", "Vision"],
  },
];

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

// ---------- Local-server pre-flight ----------

/**
 * Returns the `llama-server --version` string when the binary is on PATH,
 * or throws with an install hint when it isn't. UI calls this before
 * attempting to activate a Local model so we fail fast instead of leaving
 * the user staring at a stuck button.
 */
export async function checkLlamaServer(): Promise<string> {
  if (!isTauri()) return "mock";
  return realInvoke<string>("check_llama_server");
}

// ---------- Live model discovery ----------

export async function searchHfModels(query: string): Promise<HfModelHit[]> {
  if (!isTauri()) return [];
  return realInvoke<HfModelHit[]>("search_hf_models", { query });
}

export async function listHfQuants(repoId: string): Promise<HfQuantFile[]> {
  if (!isTauri()) return [];
  return realInvoke<HfQuantFile[]>("list_hf_quants", { repoId });
}

/** Probe an OpenAI-compatible endpoint (cloud or local) for its model list.
 *  Throws if unreachable or non-OpenAI-shaped response. */
export async function listModelsFromEndpoint(
  baseUrl: string,
  apiKey?: string,
): Promise<string[]> {
  if (!isTauri()) return [];
  return realInvoke<string[]>("list_models_from_endpoint", {
    baseUrl,
    apiKey: apiKey || null,
  });
}
let mockAppConfig: AppConfig = {
  onboardingComplete: false,
  activeProviderId: null,
  providers: [],
};

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
 * In browser dev mode (`make ui`) this always returns null — the updater plugin
 * isn't reachable without the Tauri shell.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!isTauri()) return null;
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
  if (!isTauri()) throw new Error("updater unavailable outside Tauri");
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
  if (!isTauri()) return "dev";
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
  if (!isTauri()) return [];
  return realInvoke<ConversationSummary[]>("list_conversations");
}

export async function loadConversation(id: string): Promise<Conversation> {
  if (!isTauri())
    return { id, title: "Mock", messages: [], updatedAt: Math.floor(Date.now() / 1000) };
  return realInvoke<Conversation>("load_conversation", { id });
}

export async function newConversation(): Promise<void> {
  if (!isTauri()) return;
  return realInvoke<void>("new_conversation");
}

export async function deleteConversation(id: string): Promise<void> {
  if (!isTauri()) return;
  return realInvoke<void>("delete_conversation", { id });
}

export async function getCurrentConversation(): Promise<Conversation | null> {
  if (!isTauri()) return null;
  return realInvoke<Conversation | null>("get_current_conversation");
}

export async function cancelChat(): Promise<void> {
  if (!isTauri()) return;
  return realInvoke<void>("cancel_chat");
}

export async function onChatCancelled(cb: () => void): Promise<() => void> {
  if (!isTauri()) return mockListen("chat:cancelled", () => cb());
  return realListen<unknown>("chat:cancelled", () => cb());
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
