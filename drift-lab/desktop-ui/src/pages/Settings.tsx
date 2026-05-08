import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";

import { ModelTag } from "../components/Onboarding";
import Orbs from "../components/Orbs";
import {
  AppConfig,
  BackendStatus,
  HfModelHit,
  HfQuantFile,
  LocalModelPreset,
  ModelBackendConfig,
  ProviderPreset,
  SavedProvider,
  UpdateInfo,
  UpdateProgress,
  activateProvider,
  checkForUpdate,
  checkLlamaServer,
  deleteProvider,
  downloadAndInstallUpdate,
  getAppConfig,
  getAppVersion,
  getBackendStatus,
  listHfQuants,
  listLocalPresets,
  listModelsFromEndpoint,
  listPresets,
  onBackendStatus,
  resetAllConfig,
  saveProvider,
  searchHfModels,
  testProvider,
  withTimeout,
} from "../lib/tauri";

type Tab = "models" | "local" | "providers" | "updates";

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
            Local Inference
          </TabButton>
          <TabButton active={tab === "providers"} onClick={() => switchTab("providers")}>
            Providers
          </TabButton>
          <TabButton active={tab === "updates"} onClick={() => switchTab("updates")}>
            Updates
          </TabButton>
        </nav>

        {config && tab === "models" && (
          <ModelsTab config={config} status={status} refresh={refresh} switchTab={switchTab} />
        )}
        {config && tab === "local" && <LocalTab config={config} refresh={refresh} />}
        {config && tab === "providers" && (
          <ProvidersTab config={config} refresh={refresh} />
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
          <div className="settings-card-title">{labelFor(active.config)}</div>
          <div className="settings-card-sub">
            {active.name} · {active.config.mode === "api" ? "Cloud API" : "Local Inference"} ·{" "}
            <StatusBadge status={status} />
          </div>
          <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
            <button type="button" className="primary-btn" onClick={() => switchTab("providers")}>
              Switch models
            </button>
            <button
              type="button"
              className="ghost-btn"
              onClick={() => switchTab(active.config.mode === "api" ? "providers" : "local")}
            >
              Configure providers
            </button>
          </div>
        </section>
      ) : (
        <div className="settings-section">
          <h2>No model active</h2>
          <p className="muted">
            Add a provider in <strong>Providers</strong> or pick a local model in{" "}
            <strong>Local Inference</strong>.
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

function labelFor(c: ModelBackendConfig): string {
  return c.mode === "api" ? c.model : c.spec;
}

function StatusBadge({ status }: { status: BackendStatus }) {
  const label =
    status.kind === "unconfigured"
      ? "Not configured"
      : status.kind === "idle"
        ? "Idle"
        : status.kind === "downloading"
          ? `Downloading ${status.file}`
          : status.kind === "starting"
            ? "Starting…"
            : status.kind === "ready"
              ? "Ready"
              : `Error: ${status.message}`;
  return <span className={`status-badge status-${status.kind}`}>{label}</span>;
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
          {labelFor(provider.config)}
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
    const config: ModelBackendConfig = {
      mode: "api",
      base_url: baseUrl,
      api_key: effectiveKey,
      model,
    };
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

/** Calm, copy-paste-friendly install nudge. Local inference is optional —
 *  Cloud API mode is a working alternative — so this is informational, not
 *  an error. */
function LlamaServerMissingBanner() {
  const [copied, setCopied] = useState(false);
  const cmd =
    typeof navigator !== "undefined" && /Mac/.test(navigator.platform)
      ? "brew install llama.cpp"
      : "# See https://github.com/ggml-org/llama.cpp/releases";

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(cmd);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard blocked — fall through; the command is visible inline.
    }
  };

  return (
    <div className="info-banner" style={{ marginBottom: 14 }}>
      <div style={{ fontWeight: 500, marginBottom: 4 }}>
        Local inference needs <code>llama-server</code> on this machine
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 10, fontSize: 13 }}>
        Local Inference is optional. Install <code>llama-server</code> to download
        and run open-source models, or skip this tab and use a Cloud API
        provider — Ollama, Docker Model Runner, OpenAI, etc. all work without
        any extra install.
      </p>
      <div className="install-cmd-row">
        <code className="install-cmd">{cmd}</code>
        <button type="button" className="ghost-btn" onClick={copy}>
          {copied ? "Copied!" : "Copy"}
        </button>
      </div>
      <p className="muted" style={{ marginTop: 10, marginBottom: 0, fontSize: 12 }}>
        Other platforms: see{" "}
        <a
          href="https://github.com/ggml-org/llama.cpp/releases"
          target="_blank"
          rel="noreferrer"
        >
          llama.cpp releases
        </a>
        .
      </p>
    </div>
  );
}

// ---------- Local Inference tab ----------
function LocalTab({
  config,
  refresh,
}: {
  config: AppConfig;
  refresh: () => Promise<void>;
}) {
  const [presets, setPresets] = useState<LocalModelPreset[]>([]);
  const [activating, setActivating] = useState<string | null>(null);
  const [activateError, setActivateError] = useState<string | null>(null);
  const [llamaCheck, setLlamaCheck] = useState<{
    ok: boolean;
    msg: string;
  } | null>(null);

  useEffect(() => {
    listLocalPresets().then(setPresets);
    // Run the pre-flight on mount so users see the install hint *before*
    // they click anything that depends on llama-server.
    checkLlamaServer()
      .then((v) => setLlamaCheck({ ok: true, msg: v }))
      .catch((e) =>
        setLlamaCheck({
          ok: false,
          msg: e instanceof Error ? e.message : String(e),
        }),
      );
  }, []);

  // Local providers already saved (so we know which presets are "downloaded")
  const savedSpecs = new Set(
    config.providers
      .filter((p) => p.config.mode === "local")
      .map((p) => (p.config as Extract<ModelBackendConfig, { mode: "local" }>).spec),
  );

  async function activate(preset: LocalModelPreset) {
    setActivateError(null);
    if (llamaCheck && !llamaCheck.ok) {
      setActivateError(llamaCheck.msg);
      return;
    }
    setActivating(preset.spec);
    try {
      const existing = config.providers.find(
        (p) => p.config.mode === "local" && p.config.spec === preset.spec,
      );
      if (existing) {
        await activateProvider(existing.id);
      } else {
        const cfg: ModelBackendConfig = { mode: "local", spec: preset.spec, port: 8080 };
        await saveProvider(preset.name, cfg, true);
      }
      // The Rust side resolves the backend in a background task and emits
      // `backend:status` events. We refresh immediately to flip the UI to
      // "Active"; the parent SettingsPage already subscribes to status.
      await refresh();
    } catch (e) {
      setActivateError(e instanceof Error ? e.message : String(e));
    } finally {
      setActivating(null);
    }
  }

  return (
    <section className="settings-card">
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        Local Inference Models
      </div>
      <p className="muted" style={{ marginTop: 4, marginBottom: 14 }}>
        Curated GGUF models. <code>llama-server</code> downloads them on first activation
        — the first run of a multi-GB model can take several minutes.
      </p>

      {/* Pre-flight: surface install instructions calmly. Local inference is
          optional — Cloud API providers (Ollama / Docker / OpenAI) are a
          fully working alternative that needs no extra install. */}
      {llamaCheck && !llamaCheck.ok && <LlamaServerMissingBanner />}
      {llamaCheck && llamaCheck.ok && (
        <div className="muted" style={{ fontSize: 12, marginBottom: 14 }}>
          Detected: {llamaCheck.msg}
        </div>
      )}

      {/* Inline error from the most recent activation attempt. */}
      {activateError && (
        <div className="onboarding-error" style={{ marginBottom: 14 }}>
          {activateError}
        </div>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {presets.map((p) => {
          const isSaved = savedSpecs.has(p.spec);
          const activeProvider = config.providers.find(
            (sp) => sp.id === config.activeProviderId,
          );
          const isActive =
            activeProvider?.config.mode === "local" &&
            activeProvider.config.spec === p.spec;
          return (
            <div key={p.spec} className="provider-row">
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 500 }}>
                  {p.name}{" "}
                  <span className="muted" style={{ fontSize: 12, fontWeight: 400 }}>
                    · {p.sizeGb} GB
                  </span>
                  {p.tags.map((t) => (
                    <ModelTag key={t} tag={t} />
                  ))}
                  {isActive && <span className="status-badge status-ready">Active</span>}
                </div>
                <div className="muted" style={{ fontSize: 12 }}>
                  {p.description}
                </div>
                <div className="muted" style={{ fontSize: 11, wordBreak: "break-all" }}>
                  {p.spec}
                </div>
              </div>
              <div style={{ display: "flex", gap: 8 }}>
                {!isActive && (
                  <button
                    type="button"
                    className="primary-btn"
                    onClick={() => activate(p)}
                    disabled={activating === p.spec}
                  >
                    {activating === p.spec
                      ? "Activating…"
                      : isSaved
                        ? "Activate"
                        : "Download & activate"}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      <HfSearch
        onActivate={async (spec, label) => {
          const cfg: ModelBackendConfig = { mode: "local", spec, port: 8080 };
          await saveProvider(label, cfg, true);
          await refresh();
        }}
      />
    </section>
  );
}

// ---------- HuggingFace search section ----------
function HfSearch({
  onActivate,
}: {
  onActivate: (spec: string, label: string) => Promise<void>;
}) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<HfModelHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [quants, setQuants] = useState<Record<string, HfQuantFile[]>>({});

  async function runSearch() {
    if (!query.trim()) return;
    setSearching(true);
    setError(null);
    try {
      setHits(await searchHfModels(query));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSearching(false);
    }
  }

  async function expand(repoId: string) {
    setExpanded(expanded === repoId ? null : repoId);
    if (!quants[repoId]) {
      try {
        const q = await listHfQuants(repoId);
        setQuants((prev) => ({ ...prev, [repoId]: q }));
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    }
  }

  return (
    <div style={{ marginTop: 24, paddingTop: 18, borderTop: "1px solid var(--border)" }}>
      <div className="settings-card-title" style={{ fontSize: 16 }}>
        Search HuggingFace
      </div>
      <p className="muted" style={{ marginTop: 4, marginBottom: 12 }}>
        Find any GGUF model on HuggingFace and pick a quant.
      </p>
      <div style={{ display: "flex", gap: 8 }}>
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") runSearch();
          }}
          placeholder="gemma, llama, qwen, deepseek…"
          className="onboarding-input"
        />
        <button
          type="button"
          className="primary-btn"
          onClick={runSearch}
          disabled={searching || !query.trim()}
        >
          {searching ? "…" : "Search"}
        </button>
      </div>
      {error && <div className="onboarding-error">{error}</div>}
      <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 14 }}>
        {hits.map((h) => (
          <div key={h.repoId} className="provider-row" style={{ flexDirection: "column", alignItems: "stretch" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 500, wordBreak: "break-all" }}>{h.repoId}</div>
                <div className="muted" style={{ fontSize: 12 }}>
                  ↓ {h.downloads.toLocaleString()} · ♥ {h.likes.toLocaleString()}
                </div>
              </div>
              <button type="button" className="ghost-btn" onClick={() => expand(h.repoId)}>
                {expanded === h.repoId ? "Hide quants" : "Pick quant"}
              </button>
            </div>
            {expanded === h.repoId && (
              <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 10 }}>
                {!quants[h.repoId] ? (
                  <div className="muted">Loading…</div>
                ) : quants[h.repoId].length === 0 ? (
                  <div className="muted">No GGUF files in this repo.</div>
                ) : (
                  quants[h.repoId].map((q) => (
                    <div key={q.filename} className="provider-row" style={{ background: "var(--bg-soft)" }}>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontWeight: 500 }}>{q.quant ?? "?"}</div>
                        <div className="muted" style={{ fontSize: 11, wordBreak: "break-all" }}>
                          {q.filename}
                          {q.size && ` · ${(q.size / 1_000_000_000).toFixed(2)} GB`}
                        </div>
                      </div>
                      <button
                        type="button"
                        className="primary-btn"
                        disabled={!q.quant}
                        onClick={() =>
                          q.quant && onActivate(`${h.repoId}:${q.quant}`, h.repoId)
                        }
                      >
                        Download & activate
                      </button>
                    </div>
                  ))
                )}
              </div>
            )}
          </div>
        ))}
      </div>
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
  | "not-configured" // updater disabled or release pipeline not live yet
  | "error";

/** Recognise errors that mean "the updater isn't set up yet" — distinct from
 *  real failures we'd want to bug the user about. Matches:
 *  - the plugin saying it's disabled (`active: false`)
 *  - reqwest network errors for the release JSON
 *  - GitHub returning 404 for missing releases
 *  - Tauri "no manifest" / "invalid signature" / placeholder-pubkey states */
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
      // 8s ceiling — well under the OS-level connect timeout. If the update
      // endpoint is dead/slow/disabled, the UI fails to "not configured"
      // instead of leaving the user staring at a Checking… spinner.
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

/** Calm, informative replacement for the raw HTTP error. Renders when the
 *  updater plugin is disabled OR the release pipeline isn't live yet. */
function NotConfiguredPanel({ detail }: { detail?: string }) {
  const [showDetail, setShowDetail] = useState(false);
  return (
    <div className="info-banner" style={{ marginBottom: 14 }}>
      <div style={{ fontWeight: 500, marginBottom: 4 }}>
        Auto-update isn't configured yet
      </div>
      <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 13 }}>
        The release pipeline hasn't published a <code>latest.json</code> manifest
        yet, or the updater is disabled in <code>tauri.conf.json</code>. This is
        normal during development. Updates will arrive automatically once the
        first signed release is published.
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
        <p className="muted" style={{ fontSize: 12, marginTop: 8, marginBottom: 0 }}>
          To enable: publish a GitHub release with a <code>latest.json</code>{" "}
          asset, replace <code>pubkey</code> with your Tauri signer key, and
          set <code>plugins.updater.active</code> back to <code>true</code> in{" "}
          <code>src-tauri/tauri.conf.json</code>.
        </p>
      </details>
    </div>
  );
}
