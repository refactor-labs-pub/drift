import { useEffect, useState } from "react";

import {
  LocalModelPreset,
  ModelBackendConfig,
  ProviderPreset,
  listLocalPresets,
  listModelsFromEndpoint,
  listPresets,
  saveProvider,
  testProvider,
} from "../lib/tauri";
import Orbs from "./Orbs";

type Step =
  | "pick-mode"
  | "pick-api-preset"
  | "enter-api-key"
  | "pick-local-model"
  | "testing";

export function ModelTag({ tag }: { tag: string }) {
  const lc = tag.toLowerCase();
  const cls = lc.includes("recommend")
    ? "model-tag tag-recommended"
    : lc.includes("vision")
      ? "model-tag tag-vision"
      : lc.includes("code")
        ? "model-tag tag-code"
        : lc.includes("tool")
          ? "model-tag tag-tools"
          : "model-tag";
  return <span className={cls}>{tag}</span>;
}

export default function Onboarding({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<Step>("pick-mode");
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [localPresets, setLocalPresets] = useState<LocalModelPreset[]>([]);
  const [picked, setPicked] = useState<ProviderPreset | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);

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

  useEffect(() => {
    listPresets().then(setPresets);
    listLocalPresets().then(setLocalPresets);
  }, []);

  async function activateApi() {
    if (!picked) return;
    setError(null);
    setStep("testing");
    // Local OpenAI-compatible endpoints (Ollama, Docker Model Runner,
    // LM Studio) don't need a real key — accept anything truthy.
    const effectiveKey = picked.requiresApiKey ? apiKey : "not-needed";
    const config: ModelBackendConfig = {
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

  async function activateLocal(p: LocalModelPreset) {
    setError(null);
    setStep("testing");
    const config: ModelBackendConfig = { mode: "local", spec: p.spec, port: 8080 };
    try {
      await saveProvider(p.name, config, true);
      onComplete();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStep("pick-local-model");
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
              onClick={() => setStep("pick-api-preset")}
            >
              <div className="onboarding-tile-title">Use a cloud API</div>
              <div className="onboarding-tile-sub">
                OpenAI, Groq, OpenRouter, or any OpenAI-compatible URL. Requires an
                API key.
              </div>
            </button>
            <button
              type="button"
              className="onboarding-tile"
              onClick={() => setStep("pick-local-model")}
            >
              <div className="onboarding-tile-title">Run locally</div>
              <div className="onboarding-tile-sub">
                Download a GGUF model and run it on this machine via{" "}
                <code>llama-server</code>. Free and private after the first download.
              </div>
            </button>
          </>
        )}

        {step === "pick-api-preset" && (
          <>
            <h2>Choose a provider</h2>
            <p className="muted" style={{ marginBottom: 20 }}>
              Pick the cloud you have an API key for.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {presets.map((p) => (
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
                {picked.requiresApiKey ? (
                  <a href={picked.apiKeyUrl} target="_blank" rel="noreferrer">
                    Get an API key →
                  </a>
                ) : (
                  <span>
                    No API key needed. Make sure {picked.name.replace(" (local)", "")}{" "}
                    is running on this machine.{" "}
                    <a href={picked.apiKeyUrl} target="_blank" rel="noreferrer">
                      Docs →
                    </a>
                  </span>
                )}
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

            {picked.requiresApiKey && (
              <>
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
              </>
            )}

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

            {!picked.requiresApiKey && (
              <button
                type="button"
                className="ghost-btn"
                onClick={fetchEndpointModels}
                disabled={fetchingModels}
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
                disabled={
                  (picked.requiresApiKey && !apiKey.trim()) || !model.trim()
                }
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

        {step === "pick-local-model" && (
          <>
            <h2>Pick a local model</h2>
            <p className="muted" style={{ marginBottom: 20 }}>
              The model downloads on first use. Make sure <code>llama-server</code>{" "}
              is on your <code>PATH</code> (<code>brew install llama.cpp</code>).
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {localPresets.map((p) => (
                <button
                  key={p.spec}
                  type="button"
                  className="onboarding-tile compact"
                  onClick={() => activateLocal(p)}
                >
                  <div className="onboarding-tile-title">
                    {p.name}{" "}
                    <span className="muted" style={{ fontSize: 12, fontWeight: 400 }}>
                      · {p.sizeGb} GB
                    </span>
                    {p.tags.map((t) => (
                      <ModelTag key={t} tag={t} />
                    ))}
                  </div>
                  <div className="onboarding-tile-sub">{p.description}</div>
                </button>
              ))}
            </div>
            {error && <div className="onboarding-error">{error}</div>}
            <div style={{ marginTop: 20 }}>
              <button type="button" className="ghost-btn" onClick={() => setStep("pick-mode")}>
                ← Back
              </button>
            </div>
          </>
        )}

        {step === "testing" && (
          <>
            <h2>Setting up…</h2>
            <p className="muted">
              Validating credentials and resolving the backend. Local models can take
              several minutes the first time (downloading from HuggingFace).
            </p>
          </>
        )}
      </div>
    </div>
  );
}
