//! Stage 2 — install the language-specific profiler inside a running container.
//!
//! For Python: `pip install py-spy` (fast path), with `apk add py-spy` and a
//! pre-built static binary copy as fallbacks for Alpine / minimal images.
//! For Java: download async-profiler tarball and extract under /opt.
//! For Node, Go, Ruby, .NET: stubs that emit a clear "not yet implemented"
//! error so the LLM can fall back gracefully or hand control to the user.
//!
//! The actual execution always goes through `exec_in_container::run` so the
//! agent's exec sandboxing applies uniformly.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::detect_runtime::{Language, Profiler};
use super::{exec_in_container, ToolManifest};

pub const NAME: &str = "install_profiler";
pub const DESCRIPTION: &str =
    "Install a profiler inside a running container. Picks the right tool for the detected \
     language unless `profiler` is overridden. Returns the profiler binary path and the PID of \
     the workload to attach to.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "container_id": { "type": "string" },
    "language": {
      "type": "string",
      "enum": ["python", "node", "java", "go", "ruby", "dotnet", "unknown"]
    },
    "profiler": {
      "type": "string",
      "enum": ["py-spy", "async-profiler", "perf", "node-clinic", "rbspy", "dotrace"],
      "description": "Override the auto-selected profiler."
    },
    "target_pid": {
      "type": "integer",
      "description": "If known, the PID of the workload inside the container. Otherwise we use PID 1."
    }
  },
  "required": ["container_id", "language"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub container_id: String,
    pub language: Language,
    #[serde(default)]
    pub profiler: Option<Profiler>,
    pub target_pid: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub profiler: Profiler,
    pub binary_path: String,
    pub version: Option<String>,
    pub target_pid: i32,
    /// Anything we logged while installing — useful for debugging.
    pub install_log: String,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let profiler = args
        .profiler
        .or_else(|| default_profiler(args.language))
        .ok_or_else(|| anyhow!("no profiler available for language {:?}", args.language))?;

    let target_pid = args.target_pid.unwrap_or(1);
    let mut log = String::new();

    let (binary_path, version) = match profiler {
        Profiler::PySpy => install_pyspy(&args.container_id, &mut log).await?,
        Profiler::AsyncProfiler => install_async_profiler(&args.container_id, &mut log).await?,
        Profiler::Perf => install_perf(&args.container_id, &mut log).await?,
        Profiler::NodeClinic => bail_unsupported(profiler)?,
        Profiler::Rbspy => bail_unsupported(profiler)?,
        Profiler::Dotrace => bail_unsupported(profiler)?,
        Profiler::None => return Err(anyhow!("language has no recommended profiler")),
    };

    Ok(Output {
        profiler,
        binary_path,
        version,
        target_pid,
        install_log: log,
    })
}

fn default_profiler(lang: Language) -> Option<Profiler> {
    match lang {
        Language::Python => Some(Profiler::PySpy),
        Language::Java => Some(Profiler::AsyncProfiler),
        Language::Go => Some(Profiler::Perf),
        Language::Node => Some(Profiler::NodeClinic),
        Language::Ruby => Some(Profiler::Rbspy),
        Language::Dotnet => Some(Profiler::Dotrace),
        Language::Unknown => None,
    }
}

fn bail_unsupported(p: Profiler) -> Result<(String, Option<String>)> {
    Err(anyhow!(
        "profiler {:?} install path not yet implemented — copy the binary in via `copy_to_container` and re-run",
        p
    ))
}

async fn install_pyspy(container_id: &str, log: &mut String) -> Result<(String, Option<String>)> {
    // Try `pip install --quiet py-spy` first. If pip is missing or wheels
    // can't be found, the LLM is expected to fall back to copy_to_container
    // with a static binary.
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: container_id.to_string(),
        cmd: vec![
            "sh".into(),
            "-c".into(),
            "pip install --quiet py-spy >/tmp/py-spy-install.log 2>&1 && command -v py-spy".into(),
        ],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(120),
    })
    .await
    .context("exec pip install py-spy")?;
    log.push_str(&out.stdout);
    log.push_str(&out.stderr);

    if out.exit_code != Some(0) {
        return Err(anyhow!(
            "pip install py-spy failed (exit {:?}); fall back to copy_to_container",
            out.exit_code
        ));
    }
    let binary_path = out.stdout.trim().to_string();
    let version = exec_version(container_id, &binary_path, "--version").await;
    Ok((binary_path, version))
}

async fn install_async_profiler(
    container_id: &str,
    log: &mut String,
) -> Result<(String, Option<String>)> {
    let url = "https://github.com/async-profiler/async-profiler/releases/latest/download/async-profiler-linux-x64.tar.gz";
    let cmd = format!(
        "set -eu; \
         mkdir -p /opt/async-profiler && \
         (command -v curl >/dev/null && curl -sSL {url} -o /tmp/ap.tgz \
          || wget -q -O /tmp/ap.tgz {url}) && \
         tar -xzf /tmp/ap.tgz -C /opt/async-profiler --strip-components=1 && \
         echo /opt/async-profiler/bin/asprof"
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: container_id.to_string(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(180),
    })
    .await?;
    log.push_str(&out.stdout);
    log.push_str(&out.stderr);
    if out.exit_code != Some(0) {
        return Err(anyhow!("async-profiler install failed (exit {:?})", out.exit_code));
    }
    let binary_path = out.stdout.trim().lines().last().unwrap_or("").to_string();
    let version = exec_version(container_id, &binary_path, "--version").await;
    Ok((binary_path, version))
}

async fn install_perf(container_id: &str, log: &mut String) -> Result<(String, Option<String>)> {
    // perf is usually packaged as `linux-perf-tools` / `perf`. Try common
    // distros; bail with a helpful message if all fail.
    let cmd = "set -eu; \
        if command -v perf >/dev/null; then command -v perf; \
        elif command -v apt-get >/dev/null; then apt-get update -qq && apt-get install -y --no-install-recommends linux-perf >/dev/null && command -v perf; \
        elif command -v apk >/dev/null; then apk add --no-cache perf >/dev/null && command -v perf; \
        else echo 'no package manager' >&2; exit 1; fi";
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: container_id.to_string(),
        cmd: vec!["sh".into(), "-c".into(), cmd.into()],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(180),
    })
    .await?;
    log.push_str(&out.stdout);
    log.push_str(&out.stderr);
    if out.exit_code != Some(0) {
        return Err(anyhow!("perf install failed (exit {:?})", out.exit_code));
    }
    let binary_path = out.stdout.trim().lines().last().unwrap_or("perf").to_string();
    let version = exec_version(container_id, &binary_path, "--version").await;
    Ok((binary_path, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiler_per_language() {
        assert!(matches!(default_profiler(Language::Python), Some(Profiler::PySpy)));
        assert!(matches!(default_profiler(Language::Java), Some(Profiler::AsyncProfiler)));
        assert!(matches!(default_profiler(Language::Go), Some(Profiler::Perf)));
        assert!(matches!(default_profiler(Language::Node), Some(Profiler::NodeClinic)));
        assert!(matches!(default_profiler(Language::Ruby), Some(Profiler::Rbspy)));
        assert!(matches!(default_profiler(Language::Dotnet), Some(Profiler::Dotrace)));
        assert!(default_profiler(Language::Unknown).is_none());
    }

    #[test]
    fn bail_unsupported_mentions_copy_to_container() {
        let err = bail_unsupported(Profiler::Rbspy).unwrap_err();
        assert!(err.to_string().contains("copy_to_container"));
    }

    #[tokio::test]
    async fn run_unknown_language_without_override_errors() {
        let err = run(Args {
            container_id: "c".into(),
            language: Language::Unknown,
            profiler: None,
            target_pid: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no profiler available"));
    }

    #[tokio::test]
    async fn run_pinned_none_profiler_errors() {
        let err = run(Args {
            container_id: "c".into(),
            language: Language::Python,
            profiler: Some(Profiler::None),
            target_pid: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no recommended profiler"));
    }

    #[tokio::test]
    async fn run_unsupported_profiler_returns_install_hint() {
        // Selecting Rbspy explicitly hits the bail_unsupported path without
        // touching Docker.
        let err = run(Args {
            container_id: "c".into(),
            language: Language::Ruby,
            profiler: Some(Profiler::Rbspy),
            target_pid: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not yet implemented"));
    }
}

async fn exec_version(container_id: &str, binary: &str, flag: &str) -> Option<String> {
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: container_id.to_string(),
        cmd: vec![binary.into(), flag.into()],
        user: None,
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(15),
    })
    .await
    .ok()?;
    let v = out.stdout.trim();
    if v.is_empty() { None } else { Some(v.to_string()) }
}
