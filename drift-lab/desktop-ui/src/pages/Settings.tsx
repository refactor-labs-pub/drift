import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";

import DockerSetupHint from "../components/DockerSetupHint";
import Orbs from "../components/Orbs";
import {
  activateProvider,
  activateRealtimeProfile,
  AppConfig,
  BackendStatus,
  cachedLocalRuntimes,
  cancelRealtimeTest,
  checkForUpdate,
  deleteProvider,
  deleteRealtimeProfile,
  DiscoveredRuntime,
  downloadAndInstallUpdate,
  getAppConfig,
  getAppVersion,
  getBackendStatus,
  listModelsFromEndpoint,
  listPresets,
  listRealtimeProfiles,
  ModelBackendConfig,
  onBackendStatus,
  onTestRealtimeProgress,
  probeLocalRuntimes,
  ProviderPreset,
  realtimeApiKeyName,
  RealtimeProfile,
  RealtimeSettings,
  resetAllConfig,
  SavedProvider,
  saveProvider,
  saveRealtimeProfile,
  ScanFilters,
  secretStatus,
  testProvider,
  testRealtimeConnection,
  UpdateInfo,
  UpdateProgress,
  updateScanFilters,
  withTimeout,
} from "../lib/tauri";

type Tab =
  | "models"
  | "local"
  | "providers"
  | "scanning"
  | "realtime"
  | "updates";

export default function SettingsPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const initialTab = (searchParams.get("tab") as Tab) || "models";
  const [tab, setTab] = useState<Tab>(initialTab);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [status, setStatus] = useState<BackendStatus>({ kind: "unconfigured" });

  async function refresh() {
    setConfig(await getAppConfig());
    setStatus(await getBackendStatus());
  }

  useEffect(() => {
    let unsub: (() => void) | undefined;
    (async () => {
      await refresh();
      unsub = await onBackendStatus(setStatus);
    })();
    return () => unsub?.();
  }, []);

  const switchTab = (t: Tab) => {
    setTab(t);
    setSearchParams({ tab: t });
  };

  return (
    <div className="settings-page">
      <Orbs />
      <div className="settings-shell">
        <header className="settings-header">
          <h1>Settings</h1>
          <button type="button" className="ghost-btn" onClick={() => navigate("/")}>
            ← Back
          </button>
        </header>

        <nav className="settings-tabs">
          <TabButton active={tab === "models"} onClick={() => switchTab("models")}>
            Models
          </TabButton>
          <TabButton active={tab === "local"} onClick={() => switchTab("local")}>
            Local Runtimes
          </TabButton>
          <TabButton active={tab === "providers"} onClick={() => switchTab("providers")}>
            Providers
          </TabButton>
          <TabButton active={tab === "scanning"} onClick={() => switchTab("scanning")}>
            Scanning
          </TabButton>
          <TabButton active={tab === "realtime"} onClick={() => switchTab("realtime")}>
            Realtime
          </TabButton>
          <TabButton active={tab === "updates"} onClick={() => switchTab("updates")}>
            Updates
          </TabButton>
        </nav>

        {config && tab === "models" && (
          <ModelsTab config={config} status={status} refresh={refresh} switchTab={switchTab} />
        )}
        {config && tab === "local" && (
          <LocalRuntimesTab config={config} refresh={refresh} />
        )}
        {config && tab === "providers" && (
          <ProvidersTab config={config} refresh={refresh} />
        )}
        {config && tab === "scanning" && (
          <ScanningTab config={config} refresh={refresh} />
        )}
        {config && tab === "realtime" && (
          <RealtimeTab refresh={refresh} />
        )}
        {tab === "updates" && <UpdatesTab />}
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      className={`settings-tab ${active ? "is-active" : ""}`}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

// ---------- Models tab ----------
function ModelsTab({
  config,
  status,
  refresh,
  switchTab,
}: {
  config: AppConfig;
  status: BackendStatus;
  refresh: () => Promise<void>;
  switchTab: (t: Tab) => void;
}) {
  const active = config.providers.find((p) => p.id === config.activeProviderId) ?? null;

  return (
    <>
      {active ? (
        <section className="settings-card">
          <div className="settings-card-title">{active.config.model}</div>
          <div className="settings-card-sub">
            {active.name} · <StatusBadge status={status} />
          </div>
          <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
            <button type="button" className="primary-btn" onClick={() => switchTab("providers")}>
              Switch models
            </button>
            <button
              type="button"
              className="ghost-btn"
              onClick={() => switchTab("local")}
            >
              Detected local runtimes
            </button>
          </div>
        </section>
      ) : (
        <div className="settings-section">
          <h2>No model active</h2>
          <p className="muted">
            Pick a detected local runtime in <strong>Local Runtimes</strong> or add a
            cloud API key in <strong>Providers</strong>.
          </p>
        </div>
      )}

      <section className="settings-card">
        <div className="settings-card-title" style={{ fontSize: 16 }}>
          Reset Provider and Model
        </div>
        <p className="muted" style={{ marginTop: 4, marginBottom: 16 }}>
          Clear all saved providers and reset onboarding. You'll be asked to set up
          again on next launch.
        </p>
        <button
          type="button"
          className="danger-btn"
          onClick={async () => {
            if (confirm("Reset all providers and onboarding?")) {
              await resetAllConfig();
              await refresh();
            }
          }}
        >
          Reset Provider and Model
        </button>
      </section>
    </>
  );
}

function StatusBadge({ status }: { status: BackendStatus }) {
  const label =
    status.kind === "unconfigured"
      ? "Not configured"
      : status.kind === "idle"
        ? "Idle"
        : status.kind === "starting"
          ? "Starting…"
          : status.kind === "ready"
            ? "Ready"
            : `Error: ${status.message}`;
  return <span className={`status-badge status-${status.kind}`}>{label}</span>;
}

/** Return the saved provider whose backend points at `(baseUrl, modelId)`,
 *  or `null` if none. Only `api`-mode providers can match (local runtimes
 *  always speak the OpenAI-compatible protocol). The base URL comparison is
 *  trailing-slash tolerant — `http://localhost:11434` and
 *  `http://localhost:11434/` are the same endpoint to a user, and serde
 *  normalises only one of those forms depending on which path the value
 *  was set through. */
function findMatchingProvider(
  config: AppConfig,
  baseUrl: string,
  modelId: string,
): SavedProvider | null {
  const want = baseUrl.replace(/\/+$/, "");
  for (const p of config.providers) {
    if (p.config.mode !== "api") continue;
    if (p.config.model !== modelId) continue;
    const have = p.config.base_url.replace(/\/+$/, "");
    if (have === want) return p;
  }
  return null;
}

// ---------- Local Runtimes tab ----------
//
// Plug-and-play: probes every curated local OpenAI-compatible runtime
// (Ollama, LM Studio, Docker Model Runner) in parallel and shows whichever
// one the user already has installed and running. One click on a model
// saves it as an Api provider pointed at the runtime's loopback URL.
//
// **Active mirror**: each row reads from `config.providers` to figure out
// whether a saved provider already points at `(rt.baseUrl, modelId)`. The
// row then renders one of three states:
//   - "In use" badge + disabled button — this exact combo is the active
//     provider, so clicking again would do nothing.
//   - "Activate" button — a saved provider for this combo exists but
//     another provider is currently active. One click flips the active
//     pointer without creating a duplicate.
//   - "Use this model" button — no saved provider yet; first click saves
//     and activates.
function LocalRuntimesTab({
  config,
  refresh,
}: {
  config: AppConfig;
  refresh: () => Promise<void>;
}) {
  const [runtimes, setRuntimes] = useState<DiscoveredRuntime[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activating, setActivating] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);

  async function rescan() {
    setError(null);
    setScanning(true);
    try {
      setRuntimes(await probeLocalRuntimes());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setScanning(false);
    }
  }

  useEffect(() => {
    // Instant first paint from the SQLite cache, then a fresh probe in the
    // background. If cache is empty (first launch), the spinner shows until
    // the probe returns.
    let cancelled = false;
    (async () => {
      try {
        const cached = await cachedLocalRuntimes();
        if (!cancelled && cached.length > 0) setRuntimes(cached);
      } catch {
        // Cache miss is fine — fall through to the probe.
      }
      if (!cancelled) await rescan();
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  async function activate(rt: DiscoveredRuntime, modelId: string) {
    setError(null);
    if (rt.note) {
      // Surface the runtime's setup hint instead of silently failing to
      // connect — e.g. Docker Model Runner detected via CLI but host-side
      // TCP support is off. The user needs to act on the hint first.
      setError(rt.note);
      return;
    }
    setActivating(`${rt.presetId}:${modelId}`);
    try {
      // If a provider already points at this (base_url, model), reuse it
      // instead of saving a duplicate. Saves clutter in the Providers tab
      // when the user toggles between the same models repeatedly.
      const existing = findMatchingProvider(config, rt.baseUrl, modelId);
      if (existing) {
        await activateProvider(existing.id);
      } else {
        const backend: ModelBackendConfig = {
          mode: "api",
          base_url: rt.baseUrl,
          api_key: "not-needed",
          model: modelId,
        };
        await saveProvider(`${rt.name} · ${modelId}`, backend, true);
      }
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setActivating(null);
    }
  }

  return (
    <section className="settings-card">
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          gap: 12,
        }}
      >
        <div>
          <div className="settings-card-title" style={{ fontSize: 16 }}>
            Detected local runtimes
          </div>
          <p className="muted" style={{ marginTop: 4, marginBottom: 0 }}>
            drift-lab probes <code>localhost:11434</code> (Ollama),{" "}
            <code>localhost:1234</code> (LM Studio), and{" "}
            <code>localhost:12434</code> (Docker Model Runner) in parallel. Install
            any one of them, run a model, and it shows up here.
          </p>
        </div>
        <button type="button" className="ghost-btn" onClick={rescan} disabled={scanning}>
          {scanning ? "Scanning…" : "↻ Re-scan"}
        </button>
      </div>

      {error && <div className="onboarding-error" style={{ marginTop: 14 }}>{error}</div>}

      {runtimes === null && (
        <p className="muted" style={{ marginTop: 14 }}>Probing…</p>
      )}

      {runtimes !== null && runtimes.length === 0 && (
        <div className="info-banner" style={{ marginTop: 14 }}>
          <div style={{ fontWeight: 500, marginBottom: 4 }}>
            No local runtime detected
          </div>
          <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 13 }}>
            Install any one of these, then re-scan:
          </p>
          <ul className="muted" style={{ fontSize: 13, marginTop: 0 }}>
            <li>
              <strong>Ollama</strong> ·{" "}
              <a href="https://ollama.com" target="_blank" rel="noreferrer">
                ollama.com
              </a>
            </li>
            <li>
              <strong>LM Studio</strong> ·{" "}
              <a href="https://lmstudio.ai" target="_blank" rel="noreferrer">
                lmstudio.ai
              </a>
            </li>
            <li>
              <strong>Docker Model Runner</strong> ·{" "}
              <a
                href="https://docs.docker.com/ai/model-runner/"
                target="_blank"
                rel="noreferrer"
              >
                Docker Desktop 4.40+
              </a>{" "}
              (enable in Settings → AI)
              <DockerSetupHint variant="not-detected" />
            </li>
          </ul>
        </div>
      )}

      {runtimes !== null &&
        runtimes.map((rt) => (
          <div key={rt.presetId} style={{ marginTop: 20 }}>
            <div className="settings-card-title" style={{ fontSize: 14 }}>
              {rt.name}{" "}
              <span className="muted" style={{ fontSize: 12, fontWeight: 400 }}>
                · {rt.baseUrl}
              </span>
            </div>
            {rt.note && (
              <div className="info-banner" style={{ marginTop: 8 }}>
                <p className="muted" style={{ marginTop: 0, marginBottom: 0, fontSize: 13 }}>
                  {rt.note}
                </p>
                {rt.presetId === "docker-model-runner" && (
                  <DockerSetupHint variant="needs-tcp" />
                )}
              </div>
            )}
            {rt.models.length === 0 ? (
              <p className="muted" style={{ fontSize: 13, marginTop: 6 }}>
                Up but no models loaded. Pull one (e.g.{" "}
                <code>ollama pull llama3.2:1b</code> or{" "}
                <code>docker model pull ai/smollm2</code>) and re-scan.
              </p>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 10 }}>
                {rt.models.map((m) => {
                  const key = `${rt.presetId}:${m}`;
                  const blocked = !!rt.note;
                  const match = findMatchingProvider(config, rt.baseUrl, m);
                  const isActive =
                    match !== null && match.id === config.activeProviderId;
                  const isBusy = activating === key;
                  return (
                    <div
                      key={m}
                      className={`provider-row${isActive ? " is-active" : ""}`}
                    >
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontWeight: 500,
                            wordBreak: "break-all",
                            display: "flex",
                            alignItems: "center",
                            gap: 8,
                            flexWrap: "wrap",
                          }}
                        >
                          {m}
                          {isActive && (
                            <span className="status-badge status-ready">In use</span>
                          )}
                        </div>
                      </div>
                      <button
                        type="button"
                        className="primary-btn"
                        onClick={() => activate(rt, m)}
                        disabled={isBusy || isActive}
                        title={
                          blocked
                            ? rt.note
                            : isActive
                              ? "This model is the active provider."
                              : undefined
                        }
                      >
                        {isBusy
                          ? "Activating…"
                          : blocked
                            ? "Setup needed"
                            : isActive
                              ? "In use"
                              : match
                                ? "Activate"
                                : "Use this model"}
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        ))}
    </section>
  );
}

// ---------- Providers tab ----------
function ProvidersTab({
  config,
  refresh,
}: {
  config: AppConfig;
  refresh: () => Promise<void>;
}) {
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    listPresets().then(setPresets);
  }, []);

  return (
    <section className="settings-card">
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        Saved providers
      </div>
      {config.providers.length === 0 && (
        <p className="muted" style={{ marginTop: 8 }}>
          No providers yet. Add one below.
        </p>
      )}
      {config.providers.map((p) => (
        <ProviderRow
          key={p.id}
          provider={p}
          isActive={p.id === config.activeProviderId}
          onActivate={async () => {
            await activateProvider(p.id);
            await refresh();
          }}
          onDelete={async () => {
            if (confirm(`Delete "${p.name}"?`)) {
              await deleteProvider(p.id);
              await refresh();
            }
          }}
        />
      ))}

      <div style={{ marginTop: 18 }}>
        {adding ? (
          <AddProviderForm
            presets={presets}
            onCancel={() => setAdding(false)}
            onSaved={async () => {
              setAdding(false);
              await refresh();
            }}
          />
        ) : (
          <button type="button" className="primary-btn" onClick={() => setAdding(true)}>
            + Add provider
          </button>
        )}
      </div>
    </section>
  );
}

function ProviderRow({
  provider,
  isActive,
  onActivate,
  onDelete,
}: {
  provider: SavedProvider;
  isActive: boolean;
  onActivate: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="provider-row">
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontWeight: 500 }}>
          {provider.name}{" "}
          {isActive && <span className="status-badge status-ready">Active</span>}
        </div>
        <div className="muted" style={{ fontSize: 12, wordBreak: "break-all" }}>
          {provider.config.model}
        </div>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        {!isActive && (
          <button type="button" className="ghost-btn" onClick={onActivate}>
            Activate
          </button>
        )}
        <button type="button" className="ghost-btn" onClick={onDelete}>
          Delete
        </button>
      </div>
    </div>
  );
}

function AddProviderForm({
  presets,
  onCancel,
  onSaved,
}: {
  presets: ProviderPreset[];
  onCancel: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const [presetId, setPresetId] = useState(presets[0]?.id ?? "");
  const [name, setName] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [fetchedModels, setFetchedModels] = useState<string[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);

  const preset = presets.find((p) => p.id === presetId);
  const requiresKey = preset?.requiresApiKey ?? true;

  useEffect(() => {
    if (!preset) return;
    setBaseUrl(preset.baseUrl);
    setModel(preset.models[0] ?? "");
    setName(preset.name);
    setApiKey(preset.requiresApiKey ? "" : "not-needed");
    setFetchedModels(null);
  }, [presetId]);

  async function fetchModels() {
    setError(null);
    setFetchingModels(true);
    try {
      const list = await listModelsFromEndpoint(
        baseUrl,
        requiresKey ? apiKey : undefined,
      );
      setFetchedModels(list);
      if (list.length > 0 && !model) setModel(list[0]);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setFetchingModels(false);
    }
  }

  async function save(activate: boolean) {
    if (!preset) return;
    setError(null);
    setSaving(true);
    const effectiveKey = requiresKey ? apiKey : "not-needed";
    // Dispatch on the preset's wire protocol. Anthropic's /v1/messages and
    // the OpenAI-compat /chat/completions diverge enough that we persist
    // them as two distinct `ModelBackend` variants — the provider factory
    // (`agent::make_provider`) picks the right impl from the saved tag.
    const config: ModelBackendConfig =
      preset.mode === "anthropic"
        ? { mode: "anthropic", base_url: baseUrl, api_key: effectiveKey, model }
        : { mode: "api", base_url: baseUrl, api_key: effectiveKey, model };
    try {
      await testProvider(config);
      await saveProvider(name || preset.name, config, activate);
      await onSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  const dropdownModels = fetchedModels ?? (preset?.models ?? []);
  const continueDisabled = saving || (requiresKey && !apiKey.trim()) || !model.trim();

  return (
    <div className="add-provider-form">
      <label className="onboarding-label">Provider</label>
      <select
        value={presetId}
        onChange={(e) => setPresetId(e.target.value)}
        className="onboarding-input"
      >
        {presets.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
          </option>
        ))}
      </select>

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        Display name
      </label>
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value)}
        className="onboarding-input"
        placeholder="My OpenAI"
      />

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        Base URL
      </label>
      <input
        type="text"
        value={baseUrl}
        onChange={(e) => setBaseUrl(e.target.value)}
        className="onboarding-input"
      />

      {requiresKey && (
        <>
          <label className="onboarding-label" style={{ marginTop: 10 }}>
            API key
          </label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            className="onboarding-input"
            placeholder="sk-..."
          />
        </>
      )}

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        Model
      </label>
      {dropdownModels.length > 0 ? (
        <select
          value={model}
          onChange={(e) => setModel(e.target.value)}
          className="onboarding-input"
        >
          {dropdownModels.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      ) : (
        <input
          type="text"
          value={model}
          onChange={(e) => setModel(e.target.value)}
          className="onboarding-input"
          placeholder="model name"
        />
      )}

      {preset?.mode === "anthropic" ? (
        <p className="muted" style={{ marginTop: 10, fontSize: 12 }}>
          Anthropic doesn't expose <code>/v1/models</code> — pick from the
          curated list or paste a model ID from the{" "}
          <a
            href="https://docs.anthropic.com/en/docs/models-overview"
            target="_blank"
            rel="noreferrer"
          >
            model docs
          </a>
          .
        </p>
      ) : (
        <button
          type="button"
          className="ghost-btn"
          onClick={fetchModels}
          disabled={fetchingModels}
          style={{ marginTop: 10 }}
        >
          {fetchingModels
            ? "Fetching…"
            : fetchedModels
              ? `Refresh models (${fetchedModels.length})`
              : "Fetch models from endpoint"}
        </button>
      )}

      {error && <div className="onboarding-error">{error}</div>}

      <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
        <button
          type="button"
          className="primary-btn"
          onClick={() => save(true)}
          disabled={continueDisabled}
        >
          {saving ? "Testing…" : "Save & activate"}
        </button>
        <button
          type="button"
          className="ghost-btn"
          onClick={() => save(false)}
          disabled={continueDisabled}
        >
          Save only
        </button>
        <button type="button" className="ghost-btn" onClick={onCancel} disabled={saving}>
          Cancel
        </button>
      </div>
    </div>
  );
}

// ---------- Scanning tab ----------
//
// Static-scan filter preferences. The Rust runner reads `scan_filters` from
// AppConfig at the moment a scan kicks off, so toggling here takes effect
// on the very next scan. No need to restart the app.
//
// Design: each filter row is a single `<FilterToggle>` so adding another
// filter (e.g. "exclude tests") is a one-line append. Persistence is
// optimistic — toggle flips visually, then we call updateScanFilters; on
// error we roll back and surface the message.
function ScanningTab({
  config,
  refresh,
}: {
  config: AppConfig;
  refresh: () => Promise<void>;
}) {
  const [filters, setFilters] = useState<ScanFilters>(config.scanFilters);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Keep local state aligned if the parent reloads config (e.g. after a
  // reset-config action elsewhere). Single source of truth stays Rust-side.
  useEffect(() => {
    setFilters(config.scanFilters);
  }, [config.scanFilters]);

  async function persist(next: ScanFilters) {
    const previous = filters;
    setFilters(next); // optimistic
    setError(null);
    setSaving(true);
    try {
      const saved = await updateScanFilters(next);
      setFilters(saved);
      await refresh();
    } catch (e) {
      setFilters(previous); // rollback
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="settings-card">
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        Static-scan filters
      </div>
      <p className="muted" style={{ marginTop: 4, marginBottom: 18 }}>
        Control which directories the analyzer walks when discovering entry
        points. Defaults match the 90% case — toggle off only if your
        project legitimately keeps hand-written source under these dirs.
      </p>

      <FilterToggle
        checked={filters.excludeStaticAssets}
        disabled={saving}
        label="Exclude static/assets directories"
        hint={
          <>
            Skip every <code>static/</code> and <code>assets/</code> dir at
            any depth. Stops vendored bundles (e.g.{" "}
            <code>swagger-ui-bundle.js</code>, minified vendor JS) from
            dominating the entry-point picker with synthetic functions like{" "}
            <code>Gk</code>, <code>Ek</code>.
          </>
        }
        onChange={(v) => persist({ ...filters, excludeStaticAssets: v })}
      />

      <FilterToggle
        checked={filters.excludeTests}
        disabled={saving}
        label="Exclude test/spec/mock files"
        hint={
          <>
            Drop files like <code>*.test.ts</code>, <code>*.spec.js</code>,{" "}
            <code>test_*.py</code>, and directories like{" "}
            <code>tests/</code>, <code>__tests__/</code> at the walker
            stage. Mirrors <code>make scan-prompt</code>. Turn off only when
            the test code itself is what you want to analyze — a heavy
            bundled test file (e.g. a vite-built <code>main.test.js</code>)
            can otherwise flip the dominant language pick and leave the
            picker with zero application roots.
          </>
        }
        onChange={(v) => persist({ ...filters, excludeTests: v })}
      />

      {error && (
        <div className="onboarding-error" style={{ marginTop: 14 }}>
          {error}
        </div>
      )}
      <p className="muted" style={{ marginTop: 18, fontSize: 12 }}>
        Changes take effect on the next scan you kick off — runs already in
        flight are unaffected.
      </p>
    </section>
  );
}

function FilterToggle({
  checked,
  disabled,
  label,
  hint,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  hint: React.ReactNode;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className="filter-toggle">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="filter-toggle-body">
        <span className="filter-toggle-label">{label}</span>
        <span className="filter-toggle-hint muted">{hint}</span>
      </span>
    </label>
  );
}

// ---------- Realtime tab (PR-2a — multi-profile) ----------
//
// Replaces the legacy single-form view. List of saved profiles with
// Add / Edit / Delete / Activate / Test per row. Mirrors `ProvidersTab`
// in structure so the two settings surfaces feel the same.
//
// `listRealtimeProfiles()` triggers the backend's one-time migration:
// pre-PR-2a users see their old single record appear as a profile
// named "default" the first time this tab opens.
//
// The API key field is NEVER pre-filled — we never read JWTs back to
// the renderer. A green "key saved" badge tells the user a key exists;
// leaving the field empty during save means "leave it alone". Mirrors
// the AddProviderForm behavior for API keys.

function RealtimeTab({ refresh }: { refresh: () => Promise<void> }) {
  const [settings, setSettings] = useState<RealtimeSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Editor state: `null` = closed. `{ id: null }` = add new.
  // `{ id: <existing> }` = edit existing. Only one editor at a time.
  const [editing, setEditing] = useState<{ id: string | null } | null>(null);

  const loadProfiles = useCallback(async () => {
    setError(null);
    try {
      setSettings(await listRealtimeProfiles());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  const initialFor = (id: string | null) =>
    id ? settings?.profiles.find((p) => p.id === id) ?? null : null;

  return (
    <section className="settings-card">
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        Supabase Realtime profiles
      </div>
      <p className="muted" style={{ marginTop: 4, marginBottom: 18 }}>
        One profile per project / service. Each profile has its own
        URL, channel, and stored JWT. One profile is active at a time —
        Active Scan starts streams against the active profile by
        default, with optional per-scan overrides.
      </p>

      {error && <div className="onboarding-error">{error}</div>}

      {settings && settings.profiles.length === 0 && !editing && (
        <p className="muted" style={{ marginTop: 8 }}>
          No profiles yet. Click <strong>+ Add profile</strong> to
          create one.
        </p>
      )}

      {settings?.profiles.map((p) => (
        <RealtimeProfileRow
          key={p.id}
          profile={p}
          isActive={p.id === settings.activeProfileId}
          onActivate={async () => {
            try {
              await activateRealtimeProfile(p.id);
              await loadProfiles();
              await refresh();
            } catch (e) {
              setError(e instanceof Error ? e.message : String(e));
            }
          }}
          onDelete={async () => {
            if (!confirm(`Delete profile "${p.name}"?`)) return;
            try {
              await deleteRealtimeProfile(p.id);
              await loadProfiles();
              await refresh();
            } catch (e) {
              setError(e instanceof Error ? e.message : String(e));
            }
          }}
          onEdit={() => setEditing({ id: p.id })}
          editing={editing?.id === p.id}
        />
      ))}

      {!editing ? (
        <div style={{ marginTop: 18 }}>
          <button
            type="button"
            className="primary-btn"
            onClick={() => setEditing({ id: null })}
          >
            + Add profile
          </button>
        </div>
      ) : (
        <RealtimeProfileEditor
          initial={initialFor(editing.id)}
          onSaved={async () => {
            setEditing(null);
            await loadProfiles();
            await refresh();
          }}
          onCancel={() => setEditing(null)}
        />
      )}
    </section>
  );
}

function RealtimeProfileRow({
  profile,
  isActive,
  onActivate,
  onDelete,
  onEdit,
  editing,
}: {
  profile: RealtimeProfile;
  isActive: boolean;
  onActivate: () => void;
  onDelete: () => void;
  onEdit: () => void;
  editing: boolean;
}) {
  return (
    <div
      className="provider-row"
      style={editing ? { opacity: 0.6 } : undefined}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontWeight: 500 }}>
          {profile.name}{" "}
          {isActive && (
            <span className="status-badge status-ready">Active</span>
          )}
        </div>
        <div
          className="muted"
          style={{ fontSize: 12, wordBreak: "break-all" }}
        >
          {profile.url} · channel <code>{profile.channel}</code>
        </div>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        {!isActive && (
          <button type="button" className="ghost-btn" onClick={onActivate}>
            Activate
          </button>
        )}
        <button type="button" className="ghost-btn" onClick={onEdit}>
          Edit
        </button>
        <button type="button" className="ghost-btn" onClick={onDelete}>
          Delete
        </button>
      </div>
    </div>
  );
}

// Inline editor — used for both "add new" and "edit existing".
// Owns the Test/Stop logic locally so the page-level state stays small.
function RealtimeProfileEditor({
  initial,
  onSaved,
  onCancel,
}: {
  initial: RealtimeProfile | null;
  onSaved: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const isNew = initial === null;
  const [name, setName] = useState(initial?.name ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [channel, setChannel] = useState(initial?.channel ?? "");
  const [eventName, setEventName] = useState(initial?.eventName ?? "");
  const [frameFilter, setFrameFilter] = useState(initial?.frameFilter ?? "");
  const [apiKey, setApiKey] = useState("");
  const [keyConfigured, setKeyConfigured] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Test state — same race-protected pattern as Phase 0.5 used at the
  // page level. Scoped to the editor so a Test for one profile can't
  // bleed into another row.
  const [testing, setTesting] = useState(false);
  const [testStageLabel, setTestStageLabel] = useState<string | null>(null);
  const [activeTestId, setActiveTestId] = useState<string | null>(null);
  const liveTestIdRef = useRef<string | null>(null);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);

  // Presence-check the namespaced SecretStore slot for THIS profile
  // (only meaningful when editing an existing one). New profiles never
  // have a saved key.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!initial) {
        setKeyConfigured(false);
        return;
      }
      try {
        const present = await secretStatus(realtimeApiKeyName(initial.id));
        if (!cancelled) setKeyConfigured(present);
      } catch {
        // Non-fatal — badge stays grey, user can still proceed.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [initial]);

  async function test() {
    const typedKey = apiKey.trim();
    if (!url.trim() || !name.trim()) {
      setTestResult({
        ok: false,
        message: "Enter a profile name and Supabase URL before testing.",
      });
      return;
    }
    if (!typedKey && !keyConfigured) {
      setTestResult({
        ok: false,
        message: "Enter an API key (or save one) before testing.",
      });
      return;
    }

    // **Save first, then test.** The user types a key, clicks Test, and
    // expects the key to be persisted. Before this, Test was a dry-run
    // and the key only stuck via a separate Save click — surprising
    // when the test passed but the next session lost the key. Now Test
    // is "save the profile (including the key) THEN run a connect+join
    // against the saved values". One round trip, one persisted state.
    // If save fails, we don't test — surface the save error and bail.
    setSaveError(null);
    try {
      const saved = await saveRealtimeProfile({
        id: initial?.id ?? null,
        name: name.trim(),
        url: url.trim(),
        channel: channel.trim(),
        eventName: eventName.trim(),
        frameFilter: frameFilter.trim(),
        apiKey: typedKey || null,
      });
      // Activate the just-saved profile so the test command's vault
      // resolves to ITS namespaced key, not whatever else happens to
      // be active. Without this, testing a fresh profile would read
      // the previously-active profile's JWT — silent wrong-account
      // failure.
      await activateRealtimeProfile(saved.id);
      // Clear the typed-key field once it's persisted, mirroring the
      // post-Save UX. The "✓ saved" badge confirms the slot is filled.
      setApiKey("");
      setKeyConfigured(true);
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
      return;
    }

    const id = crypto.randomUUID();
    liveTestIdRef.current = id;
    setActiveTestId(id);
    setTesting(true);
    setTestStageLabel("Testing…");
    setTestResult(null);
    const unlisten = await onTestRealtimeProgress((p) => {
      if (liveTestIdRef.current === id) setTestStageLabel(p.label);
    });
    try {
      // Inputs all null — the saved profile (which we just wrote) is
      // the authoritative source. The Rust shim resolves them from the
      // active profile or, in the case of a fresh profile we just
      // created, from the saved record.
      const result = await testRealtimeConnection(id, {
        supabaseUrl: null,
        apiKey: null,
        channel: null,
      });
      if (liveTestIdRef.current === id) setTestResult(result);
    } catch (e) {
      if (liveTestIdRef.current === id) {
        setTestResult({
          ok: false,
          message: e instanceof Error ? e.message : String(e),
        });
      }
    } finally {
      unlisten();
      if (liveTestIdRef.current === id) {
        liveTestIdRef.current = null;
        setTestStageLabel(null);
        setTesting(false);
        setActiveTestId(null);
      }
    }
  }

  async function stopTest() {
    if (!activeTestId) return;
    const idToCancel = activeTestId;
    liveTestIdRef.current = null;
    setTestStageLabel(null);
    setTesting(false);
    setActiveTestId(null);
    setTestResult({ ok: false, message: "Test cancelled." });
    try {
      await cancelRealtimeTest(idToCancel);
    } catch {
      // Best-effort; the 5 s Rust budget reaps it anyway.
    }
  }

  async function save() {
    setSaveError(null);
    setSaving(true);
    try {
      await saveRealtimeProfile({
        id: initial?.id ?? null,
        name: name.trim(),
        url: url.trim(),
        channel: channel.trim(),
        eventName: eventName.trim(),
        frameFilter: frameFilter.trim(),
        // Empty string is treated as null on the Rust side — "leave
        // the existing key alone". A user explicitly clears a key by
        // deleting + re-creating the profile (intentional — clearing
        // a JWT is rare and shouldn't be accidental).
        apiKey: apiKey.trim() || null,
      });
      await onSaved();
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  const canSave = !saving && !!name.trim() && !!url.trim();

  return (
    <div className="add-provider-form" style={{ marginTop: 18 }}>
      <div className="settings-card-title" style={{ fontSize: 14, marginBottom: 8 }}>
        {isNew ? "Add new profile" : `Edit profile`}
      </div>

      <label className="onboarding-label">Profile name</label>
      <input
        type="text"
        className="onboarding-input"
        value={name}
        placeholder="e.g. checkout-service prod"
        onChange={(e) => setName(e.target.value)}
        spellCheck={false}
      />

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        Supabase URL
      </label>
      <input
        type="text"
        className="onboarding-input"
        value={url}
        placeholder="https://abc123.supabase.co"
        onChange={(e) => setUrl(e.target.value)}
        spellCheck={false}
        autoComplete="off"
      />

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        API key{" "}
        {keyConfigured && (
          <span
            style={{
              marginLeft: 6,
              fontSize: 11,
              color: "#34d399",
              fontWeight: 500,
            }}
          >
            ✓ saved
          </span>
        )}
      </label>
      <input
        type="password"
        className="onboarding-input"
        value={apiKey}
        placeholder={
          keyConfigured
            ? "(saved — leave empty to keep current)"
            : "Paste your anon or service-role JWT"
        }
        onChange={(e) => setApiKey(e.target.value)}
        spellCheck={false}
        autoComplete="off"
      />

      <label className="onboarding-label" style={{ marginTop: 10 }}>
        Channel
      </label>
      <input
        type="text"
        className="onboarding-input"
        value={channel}
        placeholder="drift-profiler-events"
        onChange={(e) => setChannel(e.target.value)}
        spellCheck={false}
      />

      <button
        type="button"
        className="ghost-btn"
        style={{ marginTop: 8, alignSelf: "flex-start", fontSize: 12 }}
        onClick={() => setShowAdvanced((v) => !v)}
      >
        {showAdvanced ? "▾" : "▸"} Advanced
      </button>

      {showAdvanced && (
        <>
          <label className="onboarding-label" style={{ marginTop: 10 }}>
            Inner event name filter
          </label>
          <input
            type="text"
            className="onboarding-input"
            value={eventName}
            placeholder="profiler-event"
            onChange={(e) => setEventName(e.target.value)}
            spellCheck={false}
          />
          <span className="muted" style={{ fontSize: 12 }}>
            Subscriber drops broadcasts whose <code>payload.event</code>{" "}
            doesn't match. Empty = accept all.
          </span>

          <label className="onboarding-label" style={{ marginTop: 10 }}>
            Default frame filter
          </label>
          <input
            type="text"
            className="onboarding-input"
            value={frameFilter}
            placeholder="file:/app/ name:create"
            onChange={(e) => setFrameFilter(e.target.value)}
            spellCheck={false}
          />
          <span className="muted" style={{ fontSize: 12 }}>
            Pre-fills the per-scan search input. DSL:{" "}
            <code>name:foo</code>, <code>file:/app/</code>, free-text
            matches both, leading <code>!</code> negates.
          </span>
        </>
      )}

      {saveError && (
        <div className="onboarding-error" style={{ marginTop: 10 }}>
          {saveError}
        </div>
      )}

      <div style={{ display: "flex", gap: 10, marginTop: 14, flexWrap: "wrap" }}>
        <button
          type="button"
          className="primary-btn"
          onClick={save}
          disabled={!canSave}
        >
          {saving ? "Saving…" : "Save"}
        </button>
        {testing ? (
          <button type="button" className="ghost-btn" onClick={stopTest}>
            ⏹ Stop ({testStageLabel ?? "Testing…"})
          </button>
        ) : (
          <button
            type="button"
            className="ghost-btn"
            onClick={test}
            disabled={!url.trim() || (!apiKey.trim() && !keyConfigured)}
            title={
              !url.trim()
                ? "Enter the Supabase URL first"
                : !apiKey.trim() && !keyConfigured
                  ? "Paste or save an API key first"
                  : "Connect + join + close (one-shot, ~5s)"
            }
          >
            Test Connection
          </button>
        )}
        <button
          type="button"
          className="ghost-btn"
          onClick={onCancel}
          disabled={saving}
        >
          Cancel
        </button>
      </div>

      {testResult && (
        <div
          style={{
            marginTop: 10,
            padding: "8px 12px",
            borderRadius: 6,
            fontSize: 13,
            background: testResult.ok
              ? "rgba(52, 211, 153, 0.08)"
              : "rgba(248, 113, 113, 0.08)",
            color: testResult.ok ? "#34d399" : "#f87171",
            border: `1px solid ${
              testResult.ok
                ? "rgba(52, 211, 153, 0.3)"
                : "rgba(248, 113, 113, 0.3)"
            }`,
          }}
        >
          {testResult.ok ? "✓ " : "✗ "}
          {testResult.message}
        </div>
      )}
    </div>
  );
}

// ---------- Updates tab ----------
type UpdatePhase =
  | "idle"
  | "checking"
  | "uptodate"
  | "available"
  | "downloading"
  | "installing"
  | "not-configured"
  | "error";

function isUnconfiguredUpdaterError(msg: string): boolean {
  const m = msg.toLowerCase();
  return (
    m.includes("error sending request") ||
    m.includes("404") ||
    m.includes("not found") ||
    m.includes("disabled") ||
    m.includes("not configured") ||
    m.includes("invalid manifest") ||
    m.includes("could not fetch a valid release") ||
    m.includes("timed out") ||
    m.includes("timeout")
  );
}

function UpdatesTab() {
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [appVersion, setAppVersion] = useState<string>("");

  useEffect(() => {
    getAppVersion().then(setAppVersion);
    void runCheck();
  }, []);

  async function runCheck() {
    setPhase("checking");
    setErrorMsg(null);
    setInfo(null);
    try {
      const next = await withTimeout(checkForUpdate(), 8000, "Update check");
      if (!next) {
        setPhase("uptodate");
        return;
      }
      setInfo(next);
      setPhase("available");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setErrorMsg(msg);
      setPhase(isUnconfiguredUpdaterError(msg) ? "not-configured" : "error");
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
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  const downloadPct =
    progress && progress.kind === "progress" && progress.contentLength
      ? Math.min(100, Math.round((progress.downloaded / progress.contentLength) * 100))
      : null;

  return (
    <section className="settings-card">
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        App version
      </div>
      <p className="muted" style={{ marginTop: 4, marginBottom: 16 }}>
        Drift Lab <strong>v{appVersion || "…"}</strong>
        {phase === "uptodate" && " · you're on the latest version."}
        {phase === "checking" && " · checking for updates…"}
        {phase === "available" && info && ` · v${info.version} available.`}
      </p>

      {phase === "available" && info && (
        <>
          <div className="settings-card-sub" style={{ marginBottom: 12 }}>
            Update available — <strong>v{info.version}</strong>
            {info.date && <span className="muted"> · {info.date}</span>}
          </div>
          {info.notes && (
            <pre className="update-notes">{info.notes}</pre>
          )}
        </>
      )}

      {(phase === "downloading" || phase === "installing") && (
        <div style={{ marginBottom: 14 }} aria-live="polite">
          <div className="settings-card-sub">
            {phase === "installing"
              ? "Installing… app will relaunch in a moment."
              : downloadPct !== null
                ? `Downloading ${downloadPct}%`
                : "Downloading…"}
          </div>
          {downloadPct !== null && (
            <div className="update-banner-bar" style={{ marginTop: 8 }}>
              <div className="update-banner-fill" style={{ width: `${downloadPct}%` }} />
            </div>
          )}
        </div>
      )}

      {phase === "not-configured" && (
        <NotConfiguredPanel detail={errorMsg ?? undefined} />
      )}

      {phase === "error" && (
        <div className="onboarding-error" style={{ marginBottom: 14 }}>
          {errorMsg ?? "Update check failed."}
        </div>
      )}

      <div style={{ display: "flex", gap: 10 }}>
        {phase === "available" ? (
          <button type="button" className="primary-btn" onClick={runInstall}>
            Update &amp; relaunch
          </button>
        ) : phase === "not-configured" ? null : (
          <button
            type="button"
            className="ghost-btn"
            onClick={runCheck}
            disabled={phase === "checking" || phase === "downloading" || phase === "installing"}
          >
            {phase === "checking" ? "Checking…" : "Check for updates"}
          </button>
        )}
      </div>
    </section>
  );
}

function NotConfiguredPanel({ detail }: { detail?: string }) {
  const [showDetail, setShowDetail] = useState(false);
  return (
    <div className="info-banner" style={{ marginBottom: 14 }}>
      <div style={{ fontWeight: 500, marginBottom: 4 }}>
        Auto-update isn't configured yet
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 13 }}>
        The release pipeline hasn't published a <code>latest.json</code> manifest
        yet, or the <code>pubkey</code> in <code>tauri.conf.json</code> is still
        the placeholder. See{" "}
        <a
          href="https://github.com/refactor-labs-pub/drift/blob/main/drift-lab/UPDATER.md"
          target="_blank"
          rel="noreferrer"
        >
          UPDATER.md
        </a>{" "}
        for the one-time bootstrap (generate keys, set GitHub secrets, paste
        the public key) and the release recipe (push a <code>drift-lab-v*</code>
        tag). Updates will arrive automatically once the first signed release
        is published.
      </p>
      <details
        open={showDetail}
        onToggle={(e) => setShowDetail((e.target as HTMLDetailsElement).open)}
      >
        <summary className="muted" style={{ fontSize: 12, cursor: "pointer" }}>
          {showDetail ? "Hide technical details" : "Show technical details"}
        </summary>
        <pre
          style={{
            marginTop: 8,
            fontSize: 11,
            background: "rgba(0,0,0,0.04)",
            padding: 10,
            borderRadius: 8,
            overflowX: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
          }}
        >
          {detail ?? "(no details)"}
        </pre>
      </details>
    </div>
  );
}
