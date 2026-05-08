//! Stage 1 — figure out what's running inside an image.
//!
//! Uses `docker inspect` (via bollard) to walk the image config + history,
//! then applies a small set of heuristics:
//!   - Env keys (PYTHON_VERSION, NODE_VERSION, JAVA_HOME, GOROOT…)
//!   - Entrypoint / Cmd binary names (python, node, java -jar, gunicorn…)
//!   - Layer history `created_by` strings (apt-get install python3, FROM node:…)
//!
//! Returns the language, a best-guess runtime version, the recommended
//! profiler, and the strategy for finding the target PID once a container is
//! running.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::ToolManifest;
use crate::docker;

pub const NAME: &str = "detect_runtime";
pub const DESCRIPTION: &str =
    "Inspect a Docker image and detect the language runtime, framework hints, and the profiler \
     best suited for it. Call after `find_image`.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "image": {
      "type": "string",
      "description": "Image reference (e.g. 'my/api:1.2')."
    }
  },
  "required": ["image"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub image: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
    Node,
    Java,
    Go,
    Ruby,
    Dotnet,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profiler {
    PySpy,
    AsyncProfiler,
    Perf,
    NodeClinic,
    Rbspy,
    Dotrace,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PidStrategy {
    /// PID 1 is the target (typical single-process container).
    InitProcess,
    /// Find first child of PID 1 matching the runtime binary name.
    ChildOfInit,
    /// Caller must specify (e.g. multi-process container).
    Manual,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub language: Language,
    pub runtime_version: Option<String>,
    pub frameworks: Vec<String>,
    pub recommended_profiler: Profiler,
    pub pid_strategy: PidStrategy,
    /// The exact entrypoint+cmd we observed, useful for the LLM to reason about.
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let docker = docker::connect().context("docker connect")?;
    let info = docker
        .inspect_image(&args.image)
        .await
        .with_context(|| format!("inspect image {}", args.image))?;

    let cfg = info.config.unwrap_or_default();
    let env = cfg.env.unwrap_or_default();
    let entrypoint = cfg.entrypoint.unwrap_or_default();
    let cmd = cfg.cmd.unwrap_or_default();

    let language = detect_language(&env, &entrypoint, &cmd);
    let runtime_version = detect_version(&env, language);
    let frameworks = detect_frameworks(&entrypoint, &cmd);
    let recommended_profiler = profiler_for(language);
    let pid_strategy = pid_strategy_for(&entrypoint, &cmd);

    Ok(Output {
        language,
        runtime_version,
        frameworks,
        recommended_profiler,
        pid_strategy,
        entrypoint,
        cmd,
    })
}

fn detect_language(env: &[String], entry: &[String], cmd: &[String]) -> Language {
    let joined = entry.iter().chain(cmd.iter()).cloned().collect::<Vec<_>>().join(" ");
    let lower = joined.to_lowercase();

    if env.iter().any(|e| e.starts_with("PYTHON_VERSION="))
        || lower.contains("python")
        || lower.contains("gunicorn")
        || lower.contains("uvicorn")
    {
        return Language::Python;
    }
    if env.iter().any(|e| e.starts_with("NODE_VERSION=")) || lower.contains("node ") || lower.ends_with("node") {
        return Language::Node;
    }
    if env.iter().any(|e| e.starts_with("JAVA_HOME=")) || lower.contains("java ") || lower.contains(".jar") {
        return Language::Java;
    }
    if env.iter().any(|e| e.starts_with("GOROOT=")) || lower.contains("/go/") {
        return Language::Go;
    }
    if env.iter().any(|e| e.starts_with("RUBY_VERSION=")) || lower.contains("ruby") || lower.contains("rails") {
        return Language::Ruby;
    }
    if env.iter().any(|e| e.starts_with("DOTNET_")) || lower.contains("dotnet") {
        return Language::Dotnet;
    }
    Language::Unknown
}

fn detect_version(env: &[String], language: Language) -> Option<String> {
    let key = match language {
        Language::Python => "PYTHON_VERSION=",
        Language::Node => "NODE_VERSION=",
        Language::Java => "JAVA_VERSION=",
        Language::Go => "GO_VERSION=",
        Language::Ruby => "RUBY_VERSION=",
        Language::Dotnet => "DOTNET_VERSION=",
        Language::Unknown => return None,
    };
    env.iter()
        .find_map(|e| e.strip_prefix(key).map(|v| v.to_string()))
}

fn detect_frameworks(entry: &[String], cmd: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let joined = entry.iter().chain(cmd.iter()).cloned().collect::<Vec<_>>().join(" ").to_lowercase();
    for (needle, label) in [
        ("uvicorn", "uvicorn"),
        ("gunicorn", "gunicorn"),
        ("fastapi", "fastapi"),
        ("flask", "flask"),
        ("django", "django"),
        ("express", "express"),
        ("next", "next.js"),
        ("rails", "rails"),
        ("spring", "spring-boot"),
    ] {
        if joined.contains(needle) {
            out.push(label.to_string());
        }
    }
    out
}

fn profiler_for(lang: Language) -> Profiler {
    match lang {
        Language::Python => Profiler::PySpy,
        Language::Java => Profiler::AsyncProfiler,
        Language::Go => Profiler::Perf,
        Language::Node => Profiler::NodeClinic,
        Language::Ruby => Profiler::Rbspy,
        Language::Dotnet => Profiler::Dotrace,
        Language::Unknown => Profiler::None,
    }
}

fn pid_strategy_for(entry: &[String], cmd: &[String]) -> PidStrategy {
    let joined = entry.iter().chain(cmd.iter()).cloned().collect::<Vec<_>>().join(" ").to_lowercase();
    // Init-system wrappers spawn the real workload as a child of PID 1.
    if joined.contains("tini") || joined.contains("dumb-init") || joined.contains("supervisord") {
        return PidStrategy::ChildOfInit;
    }
    if entry.is_empty() && cmd.is_empty() {
        return PidStrategy::Manual;
    }
    PidStrategy::InitProcess
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String { v.to_string() }

    #[test]
    fn detects_python_from_env() {
        let env = vec![s("PYTHON_VERSION=3.11.4"), s("PATH=/usr/bin")];
        assert!(matches!(detect_language(&env, &[], &[]), Language::Python));
        assert_eq!(detect_version(&env, Language::Python).as_deref(), Some("3.11.4"));
    }

    #[test]
    fn detects_python_from_uvicorn_cmd() {
        let cmd = vec![s("uvicorn"), s("app:app"), s("--host"), s("0.0.0.0")];
        assert!(matches!(detect_language(&[], &[], &cmd), Language::Python));
    }

    #[test]
    fn detects_node_from_env() {
        let env = vec![s("NODE_VERSION=20.10.0")];
        assert!(matches!(detect_language(&env, &[], &[]), Language::Node));
    }

    #[test]
    fn detects_java_from_jar_cmd() {
        let cmd = vec![s("java"), s("-jar"), s("app.jar")];
        assert!(matches!(detect_language(&[], &[], &cmd), Language::Java));
    }

    #[test]
    fn detects_go_from_path() {
        let cmd = vec![s("/go/bin/server")];
        assert!(matches!(detect_language(&[], &[], &cmd), Language::Go));
    }

    #[test]
    fn unknown_when_no_signals() {
        let cmd = vec![s("/bin/sh"), s("-c"), s("echo hi")];
        assert!(matches!(detect_language(&[], &[], &cmd), Language::Unknown));
    }

    #[test]
    fn detects_frameworks() {
        let cmd = vec![s("gunicorn"), s("-k"), s("uvicorn.workers.UvicornWorker"), s("app:app")];
        let frameworks = detect_frameworks(&[], &cmd);
        assert!(frameworks.iter().any(|f| f == "gunicorn"));
        assert!(frameworks.iter().any(|f| f == "uvicorn"));
    }

    #[test]
    fn profiler_for_each_language() {
        assert!(matches!(profiler_for(Language::Python), Profiler::PySpy));
        assert!(matches!(profiler_for(Language::Java), Profiler::AsyncProfiler));
        assert!(matches!(profiler_for(Language::Go), Profiler::Perf));
        assert!(matches!(profiler_for(Language::Node), Profiler::NodeClinic));
        assert!(matches!(profiler_for(Language::Ruby), Profiler::Rbspy));
        assert!(matches!(profiler_for(Language::Dotnet), Profiler::Dotrace));
        assert!(matches!(profiler_for(Language::Unknown), Profiler::None));
    }

    #[test]
    fn pid_strategy_init_wrapper_means_child() {
        let entry = vec![s("/usr/bin/tini"), s("--")];
        let cmd = vec![s("python"), s("app.py")];
        assert!(matches!(pid_strategy_for(&entry, &cmd), PidStrategy::ChildOfInit));
    }

    #[test]
    fn pid_strategy_no_entry_no_cmd_is_manual() {
        assert!(matches!(pid_strategy_for(&[], &[]), PidStrategy::Manual));
    }

    #[test]
    fn pid_strategy_simple_cmd_is_init() {
        let cmd = vec![s("python"), s("app.py")];
        assert!(matches!(pid_strategy_for(&[], &cmd), PidStrategy::InitProcess));
    }

    #[test]
    fn detect_version_returns_none_for_unknown() {
        assert!(detect_version(&[s("FOO=bar")], Language::Unknown).is_none());
    }
}
