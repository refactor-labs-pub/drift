//! Live model discovery — two flavours:
//!
//! 1. [`search_hf_models`] / [`list_hf_quants`] — query the public HuggingFace
//!    API for GGUF repos and the GGUF files inside each one. No auth needed.
//! 2. [`list_models_from_endpoint`] — `GET /v1/models` against any
//!    OpenAI-compatible endpoint (cloud or local: Ollama, LM Studio, Docker
//!    Model Runner, vLLM, llama-server, ...). Used by the Add Provider form
//!    to populate the model dropdown live.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HfModelHit {
    pub repo_id: String,
    pub author: Option<String>,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct HfModelsApi {
    #[serde(rename = "id")]
    id: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default, rename = "lastModified")]
    last_modified: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HfQuantFile {
    /// Filename, e.g. `gemma-3-1b-it-Q4_K_M.gguf`.
    pub filename: String,
    /// Quant tag extracted from the filename (best-effort): `Q4_K_M`, `IQ3_M`,
    /// `Q8_0`, etc.
    pub quant: Option<String>,
    /// Size in bytes if HF reported it.
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct HfRepoInfo {
    siblings: Vec<HfSibling>,
}

#[derive(Deserialize)]
struct HfSibling {
    rfilename: String,
    #[serde(default)]
    size: Option<u64>,
}

/// Search HuggingFace for GGUF repos.
///
/// We hit the public list endpoint with `filter=gguf` and sort by downloads
/// — that's the same heuristic LM Studio / Jan use to rank popular community
/// quants. Up to 30 hits to keep the payload small.
#[tauri::command]
pub async fn search_hf_models(query: String) -> Result<Vec<HfModelHit>, String> {
    do_search(&query).await.map_err(|e| e.to_string())
}

async fn do_search(query: &str) -> Result<Vec<HfModelHit>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    let url = format!(
        "https://huggingface.co/api/models?search={}&filter=gguf&limit=30&sort=downloads&direction=-1",
        urlencode(trimmed)
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .header("user-agent", "drift-lab")
        .send()
        .await
        .context("HuggingFace search request")?;
    if !resp.status().is_success() {
        anyhow::bail!("HuggingFace search returned {}", resp.status());
    }
    let raw: Vec<HfModelsApi> = resp.json().await.context("parsing HuggingFace response")?;
    Ok(raw
        .into_iter()
        .map(|m| HfModelHit {
            repo_id: m.id,
            author: m.author,
            downloads: m.downloads,
            likes: m.likes,
            last_modified: m.last_modified,
            tags: m.tags,
        })
        .collect())
}

/// List the GGUF files inside a HuggingFace repo, with the quant tag parsed
/// out of each filename. The result feeds the "pick a quant" dropdown after
/// the user selects a search hit.
#[tauri::command]
pub async fn list_hf_quants(repo_id: String) -> Result<Vec<HfQuantFile>, String> {
    do_list_quants(&repo_id).await.map_err(|e| e.to_string())
}

async fn do_list_quants(repo_id: &str) -> Result<Vec<HfQuantFile>> {
    let url = format!("https://huggingface.co/api/models/{repo_id}");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("user-agent", "drift-lab")
        .send()
        .await
        .context("HuggingFace repo info")?;
    if !resp.status().is_success() {
        anyhow::bail!("HuggingFace repo `{repo_id}` returned {}", resp.status());
    }
    let info: HfRepoInfo = resp.json().await.context("parsing HuggingFace repo info")?;
    Ok(info
        .siblings
        .into_iter()
        .filter(|s| s.rfilename.to_lowercase().ends_with(".gguf"))
        .map(|s| HfQuantFile {
            quant: extract_quant(&s.rfilename),
            filename: s.rfilename,
            size: s.size,
        })
        .collect())
}

fn extract_quant(filename: &str) -> Option<String> {
    // Match common llama.cpp quant tags: Q4_K_M, Q5_K_S, Q8_0, IQ3_XS, F16, BF16…
    // We scan filename segments and return the first one that looks like a quant.
    let stem = filename.trim_end_matches(".gguf");
    for chunk in stem.rsplit(|c: char| c == '-' || c == '_' || c == '.') {
        let upper = chunk.to_uppercase();
        if upper.starts_with("IQ")
            || upper.starts_with('Q')
            || upper == "F16"
            || upper == "BF16"
            || upper == "F32"
        {
            // Q4 alone isn't useful — require either a digit or an underscore form
            if upper.chars().any(|c| c.is_ascii_digit()) || upper.contains('_') {
                return Some(upper);
            }
        }
    }
    None
}

#[derive(Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

/// Hit `<base_url>/models` (OpenAI shape) and return the model ids. Works
/// for OpenAI itself, Ollama, LM Studio, Docker Model Runner, vLLM, etc.
/// Returns `Err` if the endpoint is unreachable or doesn't speak the protocol.
#[tauri::command]
pub async fn list_models_from_endpoint(
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    do_list_endpoint(&base_url, api_key.as_deref())
        .await
        .map_err(|e| e.to_string())
}

async fn do_list_endpoint(base_url: &str, api_key: Option<&str>) -> Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = reqwest::Client::new()
        .get(&url)
        .header("user-agent", "drift-lab");
    if let Some(k) = api_key {
        if !k.is_empty() && k != "not-needed" {
            req = req.bearer_auth(k);
        }
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("connecting to {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("{} returned HTTP {}", url, resp.status());
    }
    let list: OpenAiModelList = resp.json().await.context("parsing /v1/models response")?;
    Ok(list.data.into_iter().map(|m| m.id).collect())
}

/// Minimal URL-encoder for the `search` parameter. Tiny crate-free encoder
/// over the printable ASCII space; we only ever feed it user search text.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::extract_quant;

    #[test]
    fn parses_quant_tags() {
        assert_eq!(
            extract_quant("gemma-3-1b-it-Q4_K_M.gguf"),
            Some("Q4_K_M".into())
        );
        assert_eq!(
            extract_quant("Llama-3.2-3B-Instruct-Q5_K_S.gguf"),
            Some("Q5_K_S".into())
        );
        assert_eq!(extract_quant("model-IQ3_M.gguf"), Some("IQ3_M".into()));
        assert_eq!(extract_quant("model-F16.gguf"), Some("F16".into()));
        assert_eq!(extract_quant("README.gguf"), None);
    }
}
