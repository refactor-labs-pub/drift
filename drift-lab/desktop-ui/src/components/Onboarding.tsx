import { useEffect, useState } from "react";

import {
  DiscoveredRuntime,
  ModelBackendConfig,
  ProviderPreset,
  cachedLocalRuntimes,
  listModelsFromEndpoint,
  listPresets,
  probeLocalRuntimes,
  saveProvider,
  testProvider,
} from "../lib/tauri";
import DockerSetupHint from "./DockerSetupHint";
import Orbs from "./Orbs";

type Step =
  | "pick-mode"
  | "pick-local-runtime"
  | "pick-api-preset"
  | "enter-api-key"
  | "testing";

export default function Onboarding({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<Step>("pick-mode");
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [runtimes, setRuntimes] = useState<DiscoveredRuntime[] | null>(null);
  const [picked, setPicked] = useState<ProviderPreset | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);

  useEffect(() => {
    listPresets().then(setPresets);
  }, []);

  async function refreshRuntimes() {
    setError(null);
    // Show cached results immediately if any — the user gets instant
    // feedback while the live probe runs.
    try {
      const cached = await cachedLocalRuntimes();
      if (cached.length > 0) setRuntimes(cached);
    } catch {
      // ignore — fall through to live probe
    }
    try {
      setRuntimes(await probeLocalRuntimes());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setRuntimes((prev) => prev ?? []);
    }
  }

  async function fetchEndpointModels() {
    if (!picked) return;
    setFetchingModels(true);
    setError(null);
    try {
      const models = await listModelsFromEndpoint(
        picked.baseUrl,
        picked.requiresApiKey ? apiKey : undefined,
      );
      setFetchedModels(models);
      if (models.length > 0 && !model) setModel(models[0]);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setFetchingModels(false);
    }
  }

  async function activateApi() {
    if (!picked) return;
    setError(null);
    setStep("testing");
    const effectiveKey = picked.requiresApiKey ? apiKey : "not-needed";
    // Anthropic talks `/v1/messages` with `x-api-key`, not `/chat/completions`
    // with `Authorization: Bearer`. The preset's `mode` decides which
    // ModelBackend variant we persist — they share field names so it's just a
    // tag swap, but everything downstream (test, save, provider impl) dispatches
    // on it.
    const config: ModelBackendConfig =
      picked.mode === "anthropic"
        ? {
            mode: "anthropic",
            base_url: picked.baseUrl,
            api_key: effectiveKey,
            model: model || picked.models[0] || "",
          }
        : {
            mode: "api",
            base_url: picked.baseUrl,
            api_key: effectiveKey,
            model: model || picked.models[0] || "",
          };
    try {
      await testProvider(config);
      await saveProvider(picked.name, config, true);
      onComplete();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStep("enter-api-key");
    }
  }

  async function activateDiscovered(rt: DiscoveredRuntime, modelId: string) {
    setError(null);
    if (rt.note) {
      // Runtime detected but not yet usable (e.g. Docker Model Runner via
      // CLI, host-side TCP off). Surface the hint instead of trying to save.
      setError(rt.note);
      return;
    }
    setStep("testing");
    const config: ModelBackendConfig = {
      mode: "api",
      base_url: rt.baseUrl,
      api_key: "not-needed",
      model: modelId,
    };
    try {
      await saveProvider(`${rt.name} · ${modelId}`, config, true);
      onComplete();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStep("pick-local-runtime");
    }
  }

  return (
    <div className="onboarding-page">
      <Orbs />
      <div className="onboarding-card">
        {step === "pick-mode" && (
          <>
            <h1>Welcome to Drift Lab</h1>
            <p className="muted" style={{ marginBottom: 28 }}>
              Pick how you'd like to run your AI model. You can change this later in
              Settings.
            </p>
            <button
              type="button"
              className="onboarding-tile"
              onClick={() => {
                setStep("pick-local-runtime");
                void refreshRuntimes();
              }}
            >
              <div className="onboarding-tile-title">Use a local runtime</div>
              <div className="onboarding-tile-sub">
                Auto-detect Ollama, LM Studio, or Docker Model Runner already running
                on this machine. Free and private — no API key needed.
              </div>
            </button>
            <button
              type="button"
              className="onboarding-tile"
              onClick={() => setStep("pick-api-preset")}
            >
              <div className="onboarding-tile-title">Use a cloud API</div>
              <div className="onboarding-tile-sub">
                OpenAI, Groq, OpenRouter, or any OpenAI-compatible URL. Requires an
                API key.
              </div>
            </button>
          </>
        )}

        {step === "pick-local-runtime" && (
          <>
            <h2>Detected local runtimes</h2>
            <p className="muted" style={{ marginBottom: 16 }}>
              drift-lab probed your machine for OpenAI-compatible runtimes. Pick one
              of the live models, or install something below.
            </p>

            {runtimes === null && <p className="muted">Probing…</p>}

            {runtimes !== null && runtimes.length === 0 && (
              <div className="info-banner" style={{ marginBottom: 14 }}>
                <div style={{ fontWeight: 500, marginBottom: 4 }}>
                  No local runtime detected
                </div>
                <p className="muted" style={{ marginTop: 0, marginBottom: 8, fontSize: 13 }}>
                  Install one of these, then come back and re-scan:
                </p>
                <ul className="muted" style={{ fontSize: 13, marginTop: 0 }}>
                  <li>
                    <strong>Ollama</strong> ·{" "}
                    <a href="https://ollama.com" target="_blank" rel="noreferrer">
                      ollama.com
                    </a>{" "}
                    — then <code>ollama pull llama3.2:1b</code>
                  </li>
                  <li>
                    <strong>LM Studio</strong> ·{" "}
                    <a href="https://lmstudio.ai" target="_blank" rel="noreferrer">
                      lmstudio.ai
                    </a>{" "}
                    — start the local server
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
                    — enable in Settings → AI
                    <DockerSetupHint variant="not-detected" />
                  </li>
                </ul>
              </div>
            )}

            {runtimes !== null && runtimes.length > 0 && (
              <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
                {runtimes.map((rt) => (
                  <div key={rt.presetId} className="onboarding-tile compact">
                    <div className="onboarding-tile-title">{rt.name}</div>
                    <div className="muted" style={{ fontSize: 11, marginBottom: 8 }}>
                      {rt.baseUrl}
                    </div>
                    {rt.note && (
                      <div
                        className="muted"
                        style={{ fontSize: 12, marginBottom: 8, fontStyle: "italic" }}
                      >
                        {rt.note}
                        {rt.presetId === "docker-model-runner" && (
                          <DockerSetupHint variant="needs-tcp" />
                        )}
                      </div>
                    )}
                    {rt.models.length === 0 ? (
                      <div className="muted" style={{ fontSize: 12 }}>
                        Runtime is up but has no models loaded. Pull one (e.g.{" "}
                        <code>ollama pull llama3.2:1b</code> or{" "}
                        <code>docker model pull ai/smollm2</code>) and re-scan.
                      </div>
                    ) : (
                      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                        {rt.models.map((m) => (
                          <button
                            key={m}
                            type="button"
                            className="ghost-btn"
                            onClick={() => activateDiscovered(rt, m)}
                            style={{ justifyContent: "flex-start" }}
                            title={rt.note}
                          >
                            {rt.note ? `${m} (setup needed)` : m}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}

            {error && <div className="onboarding-error">{error}</div>}
            <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
              <button type="button" className="ghost-btn" onClick={refreshRuntimes}>
                ↻ Re-scan
              </button>
              <button type="button" className="ghost-btn" onClick={() => setStep("pick-mode")}>
                ← Back
              </button>
            </div>
          </>
        )}

        {step === "pick-api-preset" && (
          <>
            <h2>Choose a provider</h2>
            <p className="muted" style={{ marginBottom: 20 }}>
              Pick the cloud you have an API key for.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {presets
                .filter((p) => p.requiresApiKey)
                .map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    className="onboarding-tile compact"
                    onClick={() => {
                      setPicked(p);
                      setModel(p.models[0] ?? "");
                      setStep("enter-api-key");
                    }}
                  >
                    <div className="onboarding-tile-title">{p.name}</div>
                    <div className="onboarding-tile-sub">{p.description}</div>
                    <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                      {p.baseUrl}
                    </div>
                  </button>
                ))}
            </div>
            <div style={{ marginTop: 20 }}>
              <button type="button" className="ghost-btn" onClick={() => setStep("pick-mode")}>
                ← Back
              </button>
            </div>
          </>
        )}

        {step === "enter-api-key" && picked && (
          <>
            <h2>{picked.name}</h2>
            {picked.apiKeyUrl && (
              <p className="muted" style={{ marginBottom: 16 }}>
                <a href={picked.apiKeyUrl} target="_blank" rel="noreferrer">
                  Get an API key →
                </a>
              </p>
            )}

            <label className="onboarding-label">Base URL</label>
            <input
              type="text"
              value={picked.baseUrl}
              readOnly
              className="onboarding-input"
              style={{ background: "var(--bg-soft)" }}
            />

            <label className="onboarding-label" style={{ marginTop: 12 }}>
              API key
            </label>
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..."
              className="onboarding-input"
              autoFocus
            />

            <label className="onboarding-label" style={{ marginTop: 12 }}>
              Model
            </label>
            {(fetchedModels ?? (picked.models.length > 0 ? picked.models : null)) ? (
              <select
                value={model}
                onChange={(e) => setModel(e.target.value)}
                className="onboarding-input"
              >
                {(fetchedModels ?? picked.models).map((m) => (
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
                placeholder="model name"
                className="onboarding-input"
              />
            )}

            {picked.mode === "anthropic" ? (
              <p className="muted" style={{ marginTop: 10, fontSize: 12 }}>
                Anthropic doesn't expose an OpenAI-style <code>/v1/models</code>{" "}
                listing — pick from the curated dropdown above, or type a model
                ID from{" "}
                <a
                  href="https://docs.anthropic.com/en/docs/models-overview"
                  target="_blank"
                  rel="noreferrer"
                >
                  the model docs
                </a>
                .
              </p>
            ) : (
              <button
                type="button"
                className="ghost-btn"
                onClick={fetchEndpointModels}
                disabled={fetchingModels || !apiKey.trim()}
                style={{ marginTop: 10 }}
              >
                {fetchingModels ? "Fetching…" : "Fetch available models from endpoint"}
              </button>
            )}

            {error && <div className="onboarding-error">{error}</div>}
            <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
              <button
                type="button"
                className="primary-btn"
                onClick={activateApi}
                disabled={!apiKey.trim() || !model.trim()}
              >
                Continue
              </button>
              <button
                type="button"
                className="ghost-btn"
                onClick={() => setStep("pick-api-preset")}
              >
                ← Back
              </button>
            </div>
          </>
        )}

        {step === "testing" && (
          <>
            <h2>Setting up…</h2>
            <p className="muted">Validating credentials and connecting.</p>
          </>
        )}
      </div>
    </div>
  );
}
