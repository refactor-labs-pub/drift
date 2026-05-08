//! Curated provider + local-model presets surfaced to the UI's onboarding
//! flow. Hardcoded today; can graduate to a JSON resource later.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// Statically known model ids. Empty for local endpoints — the UI fetches
    /// `/v1/models` to populate the dropdown live.
    pub models: &'static [&'static str],
    /// Where the user goes to fetch an API key. Empty for local providers.
    pub api_key_url: &'static str,
    /// `false` for local OpenAI-compatible endpoints (Ollama, Docker Model
    /// Runner, LM Studio) — the UI hides the key input and auto-fills
    /// `not-needed`.
    pub requires_api_key: bool,
    /// One-line copy explaining how to install/start this provider. Shown
    /// under the preset name in the picker. Every entry — cloud or local —
    /// uses the same OpenAI-compatible HTTP shape downstream, this field
    /// just helps the user tell *where* the endpoint comes from.
    pub description: &'static str,
}

/// All providers — cloud or local — talk the same OpenAI HTTP shape:
/// `<base_url>/chat/completions`, `<base_url>/models`, bearer auth (or
/// `not-needed` for local). The `requires_api_key` flag is the only thing
/// the UI branches on.
pub const PRESETS: &[ProviderPreset] = &[
    // ----- Cloud APIs -----
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        models: &["gpt-4o", "gpt-4o-mini", "gpt-4-turbo"],
        api_key_url: "https://platform.openai.com/api-keys",
        requires_api_key: true,
        description: "OpenAI's hosted API. Bring your own key.",
    },
    ProviderPreset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        models: &["llama-3.3-70b-versatile", "qwen/qwen3-32b"],
        api_key_url: "https://console.groq.com/keys",
        requires_api_key: true,
        description: "Hosted Llama / Qwen on Groq's LPU. OpenAI-compatible.",
    },
    ProviderPreset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        models: &["openai/gpt-4o", "anthropic/claude-opus-4-7"],
        api_key_url: "https://openrouter.ai/keys",
        requires_api_key: true,
        description: "Aggregator — one key, hundreds of models.",
    },
    // ----- Local OpenAI-compatible servers -----
    ProviderPreset {
        id: "ollama",
        name: "Ollama (local)",
        base_url: "http://localhost:11434/v1",
        models: &[],
        api_key_url: "https://ollama.com",
        requires_api_key: false,
        description: "Runs models on this machine. Install Ollama, then `ollama pull <model>`.",
    },
    ProviderPreset {
        id: "docker-model-runner",
        name: "Docker Model Runner (local)",
        // Canonical OpenAI-compat path; engine-agnostic. (NOT the
        // engine-specific `/engines/llama.cpp/v1` — that path bypasses
        // Docker's engine routing and isn't what most clients should use.)
        base_url: "http://localhost:12434/engines/v1",
        models: &[],
        api_key_url: "https://docs.docker.com/ai/model-runner/",
        requires_api_key: false,
        description: "Docker Desktop's built-in model runner. Enable in Settings → AI.",
    },
    ProviderPreset {
        id: "lm-studio",
        name: "LM Studio (local)",
        base_url: "http://localhost:1234/v1",
        models: &[],
        api_key_url: "https://lmstudio.ai",
        requires_api_key: false,
        description: "Desktop app for local models. Start the local server in LM Studio.",
    },
    ProviderPreset {
        id: "custom",
        name: "Custom (OpenAI-compatible)",
        base_url: "http://localhost:8080/v1",
        models: &[],
        api_key_url: "",
        requires_api_key: false,
        description: "Any OpenAI-compatible HTTP endpoint — vLLM, llama-server, TGI, etc.",
    },
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelPreset {
    /// `repo_id:quant`, fed to `llama-server -hf`.
    pub spec: &'static str,
    pub name: &'static str,
    pub size_gb: f32,
    pub description: &'static str,
    /// Display badges. Common values: "Recommended", "Vision", "Tools",
    /// "Code", "Long context".
    pub tags: &'static [&'static str],
}

/// Top-tier featured catalog. Intentionally short (4-5 picks) — anything else
/// the user wants is one HF search away via [`crate::model_discovery`]. Gemma
/// 4 is pinned first per user request; `llama-server -hf` resolves the spec
/// at runtime so availability is checked when activating, not at compile time.
pub const LOCAL_PRESETS: &[LocalModelPreset] = &[
    LocalModelPreset {
        spec: "unsloth/gemma-4-26B-A4B-it-GGUF:Q4_K_M",
        name: "Gemma 4 26B A4B",
        size_gb: 15.8,
        description:
            "Mixture-of-experts (~26B total, ~4B active). Strong general + vision. ~16 GB on disk.",
        tags: &["Recommended", "Vision"],
    },
    LocalModelPreset {
        spec: "unsloth/Llama-3.3-70B-Instruct-GGUF:Q4_K_M",
        name: "Llama 3.3 70B",
        size_gb: 42.0,
        description: "Meta's top open chat model. Needs 64 GB+ RAM.",
        tags: &["Tools"],
    },
    LocalModelPreset {
        spec: "Qwen/Qwen3-32B-GGUF:Q4_K_M",
        name: "Qwen 3 32B",
        size_gb: 19.0,
        description: "Latest Qwen flagship. Strong reasoning + multilingual.",
        tags: &[],
    },
    LocalModelPreset {
        spec: "unsloth/DeepSeek-R1-Distill-Llama-70B-GGUF:Q4_K_M",
        name: "DeepSeek R1 Distill 70B",
        size_gb: 42.0,
        description: "Distilled reasoning model. Shows its thinking trace.",
        tags: &[],
    },
    LocalModelPreset {
        spec: "unsloth/phi-4-GGUF:Q4_K_M",
        name: "Phi-4 14B",
        size_gb: 8.5,
        description: "Microsoft Phi-4. Strong reasoning per param.",
        tags: &[],
    },
];

#[tauri::command]
pub fn list_presets() -> Vec<ProviderPreset> {
    PRESETS.to_vec()
}

#[tauri::command]
pub fn list_local_presets() -> Vec<LocalModelPreset> {
    LOCAL_PRESETS.to_vec()
}
