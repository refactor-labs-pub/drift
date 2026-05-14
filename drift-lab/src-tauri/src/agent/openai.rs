//! OpenAI-compatible streaming provider.
//!
//! The same impl drives both API mode (api.openai.com or compatible) and
//! Local mode (llama-server's `/v1/chat/completions`). The wire shape is
//! identical — only `base_url` and `api_key` differ.
//!
//! The fiddly bit is **streamed tool calls**. SSE chunks each carry a partial
//! delta:
//!   { choices: [{ delta: { tool_calls: [{ index, id?, function: { name?, arguments? } }] } }] }
//! `arguments` arrives as JSON-string tokens which must be concatenated
//! across chunks for the same `index`. Only when the stream terminates do we
//! emit a single `MessageContent::ToolRequest` per accumulated index.
//!
//! See `goose_examples/plan-iterative-agent.md` §2 (provider trait) and §6
//! (where tool-call extraction lives).

use std::collections::BTreeMap;

use async_stream::try_stream;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;

use super::provider::{MessageStream, Provider};
use super::types::{Message, MessageContent, ProviderError, Role, ToolDef, Usage};

/// Which JSON field a model expects for "stop generating after N tokens".
///
/// OpenAI deprecated `max_tokens` for its reasoning and next-gen families
/// (o1/o3/o4, gpt-5+) — they reject the parameter outright with HTTP 400:
///   `Unsupported parameter: 'max_tokens' is not supported with this model.
///    Use 'max_completion_tokens' instead.`
/// Older OpenAI models and every local OpenAI-compatible runtime (llama-server,
/// Ollama, LM Studio, vLLM) still expect `max_tokens`. One value object owns
/// this decision so callers don't sprinkle string matching at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenLimitParam {
    MaxTokens,
    MaxCompletionTokens,
}

impl TokenLimitParam {
    pub fn field_name(self) -> &'static str {
        match self {
            Self::MaxTokens => "max_tokens",
            Self::MaxCompletionTokens => "max_completion_tokens",
        }
    }

    /// Pick the right parameter for the given model identifier. Matches
    /// case-insensitively, tolerates a `vendor/` prefix, and falls back to
    /// `max_tokens` for anything unrecognised — that keeps local runtimes
    /// (which only know `max_tokens`) working.
    pub fn for_model(model: &str) -> Self {
        let trimmed = model.trim().to_ascii_lowercase();
        let bare = trimmed.rsplit('/').next().unwrap_or(&trimmed);
        if requires_completion_tokens(bare) {
            Self::MaxCompletionTokens
        } else {
            Self::MaxTokens
        }
    }

    /// Inject the token limit into a chat-completions request body using
    /// the model-appropriate field name. No-op if `body` isn't a JSON object.
    pub fn apply(self, body: &mut serde_json::Value, limit: u32) {
        if let Some(obj) = body.as_object_mut() {
            obj.insert(self.field_name().into(), serde_json::Value::from(limit));
        }
    }

    /// As a JSON object suitable for clients that won't let us set the field
    /// directly. Specifically `rig::agent::AgentBuilder` hardcodes
    /// `"max_tokens"` in its OpenAI serializer (see
    /// `rig-core/src/providers/openai/completion/mod.rs`) — for GPT-5 /
    /// o-series we must override that via `.additional_params(json)` so the
    /// wire body carries `max_completion_tokens` instead.
    pub fn as_additional_params(self, limit: u32) -> serde_json::Value {
        serde_json::json!({ self.field_name(): limit })
    }
}

fn requires_completion_tokens(bare_model_lower: &str) -> bool {
    bare_model_lower.starts_with("o1")
        || bare_model_lower.starts_with("o3")
        || bare_model_lower.starts_with("o4")
        || bare_model_lower.starts_with("gpt-5")
}

/// Returns true if the response body looks like a context-size overrun from
/// any of the OpenAI-compatible runtimes we target.
fn is_context_error(body: &str) -> bool {
    body.contains("context_length_exceeded")
        || body.contains("ContextLength")
        || body.contains("exceed_context_size_error")
        || body.contains("available context size")
}

/// Best-effort extraction of `(prompt_tokens, context_size)` from a provider
/// error body. Docker Model Runner returns `n_prompt_tokens` / `n_ctx`; other
/// runtimes report nothing structured — we tolerate that and return `None`s.
fn extract_context_numbers(body: &str) -> (Option<u64>, Option<u64>) {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let err = parsed.get("error").unwrap_or(&parsed);
    let prompt = err
        .get("n_prompt_tokens")
        .or_else(|| err.get("prompt_tokens"))
        .and_then(|n| n.as_u64());
    let ctx = err
        .get("n_ctx")
        .or_else(|| err.get("context_size"))
        .and_then(|n| n.as_u64());
    (prompt, ctx)
}

/// Per the official OpenAI Chat Completions reference
/// (<https://platform.openai.com/docs/api-reference/chat/create>),
/// `max_tokens` is **optional and deprecated** in favour of
/// `max_completion_tokens`. The server picks a model-appropriate default
/// (model max output minus prompt tokens for classic models) when neither
/// field is sent, which is the right behaviour for almost every workload —
/// hardcoding a number forces a cap that may clash with the model's own
/// limit or with strict-validation deployments (Azure, LiteLLM proxies).
///
/// So we keep the budget **opt-in** via [`OpenAiProvider::with_max_tokens`].
/// Callers that need a deterministic cap can set one; everyone else lets
/// the server decide.
pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: Option<u32>,
    http: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            max_tokens: None,
            http: reqwest::Client::new(),
        }
    }

    /// Cap the response to `max_tokens` tokens. The right wire field
    /// (`max_tokens` vs `max_completion_tokens`) is picked automatically per
    /// the [`TokenLimitParam`] policy, so this works for both classic and
    /// reasoning models.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    fn build_request_body(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> serde_json::Value {
        let mut wire_messages: Vec<serde_json::Value> = Vec::with_capacity(messages.len() + 1);
        if !system.is_empty() {
            wire_messages.push(serde_json::json!({"role": "system", "content": system}));
        }
        for m in messages {
            wire_messages.push(message_to_wire(m));
        }

        let wire_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": wire_messages,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        // Only send a token limit if the caller explicitly set one — OpenAI's
        // own reference recommends omitting `max_tokens` so the server picks
        // a model-appropriate default. When set, route through
        // [`TokenLimitParam`] so reasoning models get `max_completion_tokens`
        // and classic models get `max_tokens`.
        if let Some(limit) = self.max_tokens {
            TokenLimitParam::for_model(&self.model).apply(&mut body, limit);
        }
        if !wire_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(wire_tools);
        }
        body
    }
}

/// Lower one of our `Message`s into the OpenAI chat-completion wire shape.
fn message_to_wire(m: &Message) -> serde_json::Value {
    match m.role {
        Role::System => serde_json::json!({"role": "system", "content": m.flat_text()}),
        Role::User => serde_json::json!({"role": "user", "content": m.flat_text()}),
        Role::Tool => {
            // Tool messages carry exactly one ToolResponse — the format
            // requires `tool_call_id` and a string `content`.
            for c in &m.content {
                if let MessageContent::ToolResponse { id, content, .. } = c {
                    return serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": content,
                    });
                }
            }
            serde_json::json!({"role": "tool", "content": ""})
        }
        Role::Assistant => {
            // Assistant turns may carry both text and tool calls. Tool calls
            // serialise as `tool_calls[]`; arguments are a JSON-encoded string
            // (OpenAI's quirk).
            let mut text = String::new();
            let mut tool_calls: Vec<serde_json::Value> = Vec::new();
            for c in &m.content {
                match c {
                    MessageContent::Text(t) => text.push_str(t),
                    MessageContent::ToolRequest { id, name, arguments } => {
                        tool_calls.push(serde_json::json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": arguments.to_string(),
                            }
                        }));
                    }
                    MessageContent::ToolResponse { .. } => {}
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert("role".into(), "assistant".into());
            // OpenAI accepts null content when tool_calls is present. For
            // pure-text turns, send the text. For mixed turns, both fields go.
            if text.is_empty() && !tool_calls.is_empty() {
                obj.insert("content".into(), serde_json::Value::Null);
            } else {
                obj.insert("content".into(), serde_json::Value::String(text));
            }
            if !tool_calls.is_empty() {
                obj.insert("tool_calls".into(), serde_json::Value::Array(tool_calls));
            }
            serde_json::Value::Object(obj)
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn stream(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<MessageStream, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = self.build_request_body(system, messages, tools);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            // Heuristic match across providers:
            //   - OpenAI: `context_length_exceeded`
            //   - llama-server: `ContextLength`
            //   - Docker Model Runner: `exceed_context_size_error` / `available context size`
            if is_context_error(&body) {
                let (prompt_tokens, context_size) = extract_context_numbers(&body);
                return Err(ProviderError::ContextLengthExceeded {
                    prompt_tokens,
                    context_size,
                    model: Some(self.model.clone()),
                });
            }
            return Err(ProviderError::Other(format!("HTTP {status}: {body}")));
        }

        // Convert the bytes-stream into a line-buffered SSE iterator. Each
        // SSE event is `data: {...}\n\n`. We split on newlines and parse
        // every `data:` line as JSON until we see `[DONE]`.
        let byte_stream = resp.bytes_stream();
        let stream = try_stream! {
            let mut buf = String::new();
            let mut accumulator = StreamAccumulator::default();
            let mut s = byte_stream;

            while let Some(chunk) = s.next().await {
                let chunk = chunk.map_err(|e| ProviderError::Network(e.to_string()))?;
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| ProviderError::Malformed(e.to_string()))?;
                buf.push_str(text);

                // Drain whole SSE events out of the buffer (split on \n\n).
                while let Some(idx) = buf.find("\n\n") {
                    let raw_event: String = buf.drain(..idx + 2).collect();
                    for line in raw_event.lines() {
                        let line = line.trim_start();
                        let payload = match line.strip_prefix("data:") {
                            Some(p) => p.trim(),
                            None => continue,
                        };
                        if payload == "[DONE]" {
                            // Final flush: emit any pending text + tool calls.
                            if let Some((msg, usage)) = accumulator.finalize() {
                                yield (Some(msg), usage);
                            }
                            return;
                        }
                        let parsed: WireChunk = match serde_json::from_str(payload) {
                            Ok(p) => p,
                            Err(_) => continue, // tolerate keep-alives / unknown frames
                        };
                        if let Some((msg, usage)) = accumulator.absorb(parsed) {
                            yield (Some(msg), usage);
                        }
                    }
                }
            }

            // Stream ended without [DONE] — flush whatever we have.
            if let Some((msg, usage)) = accumulator.finalize() {
                yield (Some(msg), usage);
            }
        };

        Ok(Box::pin(stream))
    }
}

// ---- SSE parsing -----------------------------------------------------------

#[derive(Deserialize, Debug)]
struct WireChunk {
    #[serde(default)]
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize, Debug)]
struct WireChoice {
    #[serde(default)]
    delta: WireDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
struct WireDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Deserialize, Debug)]
struct WireToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireFunctionDelta>,
}

#[derive(Deserialize, Default, Debug)]
struct WireFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize, Debug)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

/// Builds up one assistant message across many SSE chunks. Streams text
/// out incrementally (as `Text` deltas) but holds tool-call deltas back —
/// they're only complete once the stream finishes per `finish_reason`.
#[derive(Default)]
struct StreamAccumulator {
    /// In-flight tool calls keyed by SSE index. BTreeMap to preserve order.
    pending_tools: BTreeMap<u32, PendingToolCall>,
    /// Whether we've already emitted the terminal aggregated message.
    finalised: bool,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl StreamAccumulator {
    fn absorb(&mut self, chunk: WireChunk) -> Option<(Message, Option<Usage>)> {
        // Yield text deltas immediately so the UI sees the model typing.
        let mut text_delta = String::new();
        let mut finished = false;
        for choice in &chunk.choices {
            if let Some(t) = &choice.delta.content {
                text_delta.push_str(t);
            }
            if let Some(deltas) = &choice.delta.tool_calls {
                for d in deltas {
                    let entry = self.pending_tools.entry(d.index).or_default();
                    if let Some(id) = &d.id {
                        entry.id = id.clone();
                    }
                    if let Some(f) = &d.function {
                        if let Some(n) = &f.name {
                            entry.name.push_str(n);
                        }
                        if let Some(a) = &f.arguments {
                            entry.arguments.push_str(a);
                        }
                    }
                }
            }
            if choice.finish_reason.is_some() {
                finished = true;
            }
        }

        let usage = chunk.usage.map(|u| Usage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        if finished && !self.finalised {
            self.finalised = true;
            // Emit one aggregated message: text first, tool calls after.
            let mut content: Vec<MessageContent> = Vec::new();
            if !text_delta.is_empty() {
                content.push(MessageContent::Text(text_delta));
            }
            for (_, p) in std::mem::take(&mut self.pending_tools) {
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&p.arguments);
                let arguments = parsed.unwrap_or(serde_json::Value::String(p.arguments));
                content.push(MessageContent::ToolRequest {
                    id: p.id,
                    name: p.name,
                    arguments,
                });
            }
            return Some((
                Message {
                    role: Role::Assistant,
                    content,
                },
                usage,
            ));
        }

        // Incremental text: emit only the delta, no tool calls yet.
        if !text_delta.is_empty() {
            return Some((
                Message {
                    role: Role::Assistant,
                    content: vec![MessageContent::Text(text_delta)],
                },
                usage,
            ));
        }

        // Pure usage frame (no choices).
        if usage.is_some() {
            return Some((
                Message {
                    role: Role::Assistant,
                    content: vec![],
                },
                usage,
            ));
        }
        None
    }

    fn finalize(&mut self) -> Option<(Message, Option<Usage>)> {
        if self.finalised || self.pending_tools.is_empty() {
            return None;
        }
        self.finalised = true;
        let mut content = Vec::new();
        for (_, p) in std::mem::take(&mut self.pending_tools) {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&p.arguments);
            let arguments = parsed.unwrap_or(serde_json::Value::String(p.arguments));
            content.push(MessageContent::ToolRequest {
                id: p.id,
                name: p.name,
                arguments,
            });
        }
        Some((
            Message {
                role: Role::Assistant,
                content,
            },
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_param_defaults_to_max_tokens_for_classic_and_local_models() {
        for model in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
            "llama-3.1-8b-instruct",
            "qwen2.5-coder-32b",
            "mistral-7b",
        ] {
            assert_eq!(
                TokenLimitParam::for_model(model),
                TokenLimitParam::MaxTokens,
                "{model} should use max_tokens",
            );
        }
    }

    #[test]
    fn token_param_picks_max_completion_tokens_for_o_series() {
        for model in ["o1", "o1-mini", "o1-preview", "o1-pro", "o3-mini", "o4-mini"] {
            assert_eq!(
                TokenLimitParam::for_model(model),
                TokenLimitParam::MaxCompletionTokens,
                "{model} should use max_completion_tokens",
            );
        }
    }

    #[test]
    fn token_param_picks_max_completion_tokens_for_gpt5_family() {
        for model in [
            "gpt-5",
            "gpt-5-mini",
            "gpt-5-nano",
            "gpt-5.5",
            "gpt-5-2025-01-01",
        ] {
            assert_eq!(
                TokenLimitParam::for_model(model),
                TokenLimitParam::MaxCompletionTokens,
                "{model} should use max_completion_tokens",
            );
        }
    }

    #[test]
    fn token_param_is_case_insensitive_and_tolerates_vendor_prefix() {
        assert_eq!(
            TokenLimitParam::for_model("GPT-5"),
            TokenLimitParam::MaxCompletionTokens,
        );
        assert_eq!(
            TokenLimitParam::for_model("openai/gpt-5-mini"),
            TokenLimitParam::MaxCompletionTokens,
        );
        assert_eq!(
            TokenLimitParam::for_model("  o1-pro  "),
            TokenLimitParam::MaxCompletionTokens,
        );
    }

    #[test]
    fn streaming_body_omits_token_limit_by_default() {
        // Per the OpenAI Chat Completions API reference, `max_tokens` is
        // optional and the server picks a model-appropriate default when it
        // isn't sent. Don't hardcode a cap that may clash with the model's
        // own limit or with a strict-validation deployment.
        for model in ["gpt-4o", "gpt-3.5-turbo", "gpt-5", "gpt-5.5", "o1-mini"] {
            let p = OpenAiProvider::new("https://api.openai.com/v1", "sk", model);
            let body = p.build_request_body("", &[Message::user("hi")], &[]);
            assert!(
                body.get("max_tokens").is_none(),
                "{model}: must not send max_tokens unless caller explicitly opts in",
            );
            assert!(
                body.get("max_completion_tokens").is_none(),
                "{model}: must not send max_completion_tokens unless caller opts in",
            );
        }
    }

    #[test]
    fn with_max_tokens_routes_to_max_tokens_for_classic_models() {
        let p = OpenAiProvider::new("https://api.openai.com/v1", "sk", "gpt-4o")
            .with_max_tokens(8000);
        let body = p.build_request_body("", &[Message::user("hi")], &[]);
        assert_eq!(body["max_tokens"], 8000);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn with_max_tokens_routes_to_max_completion_tokens_for_reasoning_models() {
        // The right wire field is still picked automatically — `max_tokens`
        // on GPT-5 / o-series returns `400 unsupported_parameter`.
        for model in ["gpt-5", "gpt-5.5", "gpt-5-mini", "o1", "o3-mini", "o4-mini"] {
            let p = OpenAiProvider::new("https://api.openai.com/v1", "sk", model)
                .with_max_tokens(32_000);
            let body = p.build_request_body("", &[Message::user("hi")], &[]);
            assert_eq!(
                body["max_completion_tokens"], 32_000,
                "{model}: explicit cap must route to max_completion_tokens",
            );
            assert!(
                body.get("max_tokens").is_none(),
                "{model}: never send the deprecated max_tokens field on reasoning models",
            );
        }
    }

    #[test]
    fn as_additional_params_emits_field_with_limit() {
        let reasoning = TokenLimitParam::for_model("gpt-5.5").as_additional_params(25_000);
        assert_eq!(reasoning["max_completion_tokens"], 25_000);
        assert!(reasoning.get("max_tokens").is_none());

        let classic = TokenLimitParam::for_model("gpt-4o").as_additional_params(25_000);
        assert_eq!(classic["max_tokens"], 25_000);
        assert!(classic.get("max_completion_tokens").is_none());
    }

    #[test]
    fn as_additional_params_object_shape_matches_rig_additional_params_contract() {
        // rig::agent::AgentBuilder::additional_params(serde_json::Value) merges
        // the value into the request body as-is. The value must be a JSON object
        // (not an array, not a string) for the merge to land the right field.
        let v = TokenLimitParam::for_model("o3-mini").as_additional_params(25_000);
        assert!(v.is_object(), "rig requires a JSON object for additional_params merging");
    }

    #[test]
    fn apply_writes_the_model_specific_field_only() {
        let mut body = serde_json::json!({"model": "gpt-5.5"});
        TokenLimitParam::for_model("gpt-5.5").apply(&mut body, 1);
        assert_eq!(body["max_completion_tokens"], 1);
        assert!(body.get("max_tokens").is_none());

        let mut body = serde_json::json!({"model": "gpt-4o"});
        TokenLimitParam::for_model("gpt-4o").apply(&mut body, 1);
        assert_eq!(body["max_tokens"], 1);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn is_context_error_matches_docker_model_runner_shape() {
        // The exact payload Docker Model Runner returned in the field:
        let body = r#"{"error":{"code":400,"message":"request (4125 tokens) exceeds the available context size (4096 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":4125,"n_ctx":4096}}"#;
        assert!(is_context_error(body));
        let (prompt, ctx) = extract_context_numbers(body);
        assert_eq!(prompt, Some(4125));
        assert_eq!(ctx, Some(4096));
    }

    #[test]
    fn is_context_error_matches_openai_shape() {
        let body = r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        assert!(is_context_error(body));
    }

    #[test]
    fn extract_context_numbers_tolerates_missing_fields() {
        // Generic shape with neither n_prompt_tokens nor n_ctx — must not panic.
        let body = r#"{"error":{"message":"context length exceeded","type":"context_length_exceeded"}}"#;
        let (p, c) = extract_context_numbers(body);
        assert!(p.is_none());
        assert!(c.is_none());
    }

    #[test]
    fn accumulator_concatenates_text() {
        let mut acc = StreamAccumulator::default();
        let chunk1 = WireChunk {
            choices: vec![WireChoice {
                delta: WireDelta {
                    content: Some("Hel".into()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        let chunk2 = WireChunk {
            choices: vec![WireChoice {
                delta: WireDelta {
                    content: Some("lo".into()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        };
        let m1 = acc.absorb(chunk1).unwrap().0;
        let m2 = acc.absorb(chunk2).unwrap().0;
        // First chunk is the partial delta; second is the finalised message.
        assert_eq!(m1.flat_text(), "Hel");
        assert_eq!(m2.flat_text(), "lo");
    }

    #[test]
    fn accumulator_assembles_streamed_tool_call() {
        let mut acc = StreamAccumulator::default();
        // Frame 1: id + name + start of args.
        acc.absorb(WireChunk {
            choices: vec![WireChoice {
                delta: WireDelta {
                    content: None,
                    tool_calls: Some(vec![WireToolCallDelta {
                        index: 0,
                        id: Some("call_a".into()),
                        function: Some(WireFunctionDelta {
                            name: Some("find_image".into()),
                            arguments: Some(r#"{"path""#.into()),
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
        });
        // Frame 2: more args, then finish.
        let final_msg = acc
            .absorb(WireChunk {
                choices: vec![WireChoice {
                    delta: WireDelta {
                        content: None,
                        tool_calls: Some(vec![WireToolCallDelta {
                            index: 0,
                            id: None,
                            function: Some(WireFunctionDelta {
                                name: None,
                                arguments: Some(r#":"/tmp"}"#.into()),
                            }),
                        }]),
                    },
                    finish_reason: Some("tool_calls".into()),
                }],
                usage: None,
            })
            .unwrap()
            .0;

        let reqs = final_msg.tool_requests();
        assert_eq!(reqs.len(), 1);
        let (id, name, args) = reqs[0];
        assert_eq!(id, "call_a");
        assert_eq!(name, "find_image");
        assert_eq!(args["path"], serde_json::json!("/tmp"));
    }

    #[test]
    fn message_to_wire_assistant_with_tool_call_uses_null_content() {
        let m = Message {
            role: Role::Assistant,
            content: vec![MessageContent::ToolRequest {
                id: "x".into(),
                name: "find_image".into(),
                arguments: serde_json::json!({"path": "/tmp"}),
            }],
        };
        let wire = message_to_wire(&m);
        assert_eq!(wire["role"], "assistant");
        assert!(wire["content"].is_null());
        assert_eq!(wire["tool_calls"][0]["function"]["name"], "find_image");
        // Arguments must be a JSON string, not a JSON object — OpenAI quirk.
        assert!(wire["tool_calls"][0]["function"]["arguments"].is_string());
    }

    #[test]
    fn message_to_wire_tool_response() {
        let m = Message::tool_response("x", "result", false);
        let wire = message_to_wire(&m);
        assert_eq!(wire["role"], "tool");
        assert_eq!(wire["tool_call_id"], "x");
        assert_eq!(wire["content"], "result");
    }
}
