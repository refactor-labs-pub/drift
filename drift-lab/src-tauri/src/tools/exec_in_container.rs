//! Primitive — run a command inside a running container.
//!
//! Wraps bollard's exec API and aggregates stdout/stderr into a single
//! transcript so the LLM can read the result directly. For long-running
//! processes (e.g. py-spy record) prefer `detach=true` and poll separately.

use anyhow::{Context, Result};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use super::ToolManifest;
use crate::docker;

pub const NAME: &str = "exec_in_container";
pub const DESCRIPTION: &str =
    "Run a command inside a container via `docker exec`. Returns combined stdout+stderr and the \
     exit code. Use `detach=true` for background tasks (e.g. profiler workers) and read the \
     output later via the container's filesystem.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "container_id": { "type": "string" },
    "cmd": {
      "type": "array",
      "items": { "type": "string" },
      "description": "argv for the command (e.g. [\"sh\", \"-c\", \"pip install py-spy\"])."
    },
    "user": { "type": "string", "description": "Optional user to exec as (e.g. \"root\")." },
    "workdir": { "type": "string" },
    "env": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Extra environment variables (KEY=VALUE)."
    },
    "detach": { "type": "boolean", "description": "Fire-and-forget; default false." },
    "timeout_secs": {
      "type": "integer",
      "description": "Kill the read loop after this many seconds. Default 30."
    }
  },
  "required": ["container_id", "cmd"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub container_id: String,
    pub cmd: Vec<String>,
    pub user: Option<String>,
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub detach: bool,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
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

    let create = CreateExecOptions {
        cmd: Some(args.cmd.clone()),
        attach_stdout: Some(!args.detach),
        attach_stderr: Some(!args.detach),
        user: args.user.clone(),
        working_dir: args.workdir.clone(),
        env: if args.env.is_empty() { None } else { Some(args.env.clone()) },
        ..Default::default()
    };

    let exec = docker
        .create_exec(&args.container_id, create)
        .await
        .context("create_exec")?;

    let start_opts = StartExecOptions {
        detach: args.detach,
        ..Default::default()
    };

    let started = docker
        .start_exec(&exec.id, Some(start_opts))
        .await
        .context("start_exec")?;

    if args.detach {
        return Ok(Output {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        });
    }

    let timeout = std::time::Duration::from_secs(args.timeout_secs.unwrap_or(30));
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut timed_out = false;

    if let StartExecResults::Attached { mut output, .. } = started {
        let drain = async {
            while let Some(chunk) = output.next().await {
                match chunk {
                    Ok(bollard::container::LogOutput::StdOut { message })
                    | Ok(bollard::container::LogOutput::Console { message }) => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    Ok(bollard::container::LogOutput::StdErr { message }) => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        stderr.push_str(&format!("\n[stream error: {e}]"));
                        break;
                    }
                }
            }
        };
        match tokio::time::timeout(timeout, drain).await {
            Ok(()) => {}
            Err(_) => timed_out = true,
        }
    }

    let exit_code = docker
        .inspect_exec(&exec.id)
        .await
        .ok()
        .and_then(|i| i.exit_code);

    Ok(Output {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse_minimal() {
        let raw = r#"{"container_id": "abc", "cmd": ["sh", "-c", "echo hi"]}"#;
        let args: Args = serde_json::from_str(raw).unwrap();
        assert_eq!(args.container_id, "abc");
        assert_eq!(args.cmd, vec!["sh", "-c", "echo hi"]);
        assert!(!args.detach);
        assert!(args.env.is_empty());
        assert!(args.timeout_secs.is_none());
    }

    #[test]
    fn args_parse_with_extras() {
        let raw = r#"{
            "container_id": "abc",
            "cmd": ["true"],
            "user": "root",
            "workdir": "/tmp",
            "env": ["FOO=bar"],
            "detach": true,
            "timeout_secs": 60
        }"#;
        let args: Args = serde_json::from_str(raw).unwrap();
        assert_eq!(args.user.as_deref(), Some("root"));
        assert_eq!(args.workdir.as_deref(), Some("/tmp"));
        assert_eq!(args.env, vec!["FOO=bar"]);
        assert!(args.detach);
        assert_eq!(args.timeout_secs, Some(60));
    }

    #[test]
    fn manifest_is_well_formed() {
        let m = manifest();
        assert_eq!(m.name, "exec_in_container");
        let v: serde_json::Value = serde_json::from_str(m.parameters).unwrap();
        assert_eq!(v["required"], serde_json::json!(["container_id", "cmd"]));
    }

    /// Live-Docker integration test. Spawns a long-lived alpine container,
    /// execs `echo hi`, and asserts the captured stdout. Run with
    /// `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires running docker daemon + alpine image"]
    async fn exec_echo_against_live_container() {
        use bollard::container::{Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions};

        let docker = crate::docker::connect().unwrap();
        let id = docker
            .create_container(
                Some(CreateContainerOptions::<&str> { name: "drift-test-exec", platform: None }),
                Config {
                    image: Some("alpine:3"),
                    cmd: Some(vec!["sleep", "30"]),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .id;
        docker
            .start_container(&id, None::<StartContainerOptions<&str>>)
            .await
            .unwrap();

        let out = run(Args {
            container_id: id.clone(),
            cmd: vec!["sh".into(), "-c".into(), "echo hi".into()],
            user: None,
            workdir: None,
            env: vec![],
            detach: false,
            timeout_secs: Some(10),
        })
        .await
        .unwrap();

        let _ = docker
            .remove_container(&id, Some(RemoveContainerOptions { force: true, ..Default::default() }))
            .await;

        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("hi"));
        assert!(!out.timed_out);
    }
}
