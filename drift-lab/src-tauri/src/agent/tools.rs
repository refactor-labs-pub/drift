//! Tool registry — the funnel between `MessageContent::ToolRequest` and the
//! existing `crate::tools::*` modules.
//!
//! Goose's `dispatch_tool_call` routes through an MCP extension manager that
//! talks to subprocess-hosted tools. Drift's tools are all in-process Rust
//! functions, so dispatch is a `match` on the tool name plus
//! `serde_json::from_value` to decode `Args`. Same shape, no IPC.
//!
//! Each tool also carries a `Permission` tag. Read-only tools auto-approve;
//! destructive ones surface a `NeedsApproval` decision the loop can hand
//! back to the UI. See `goose_examples/plan-iterative-agent.md` §8.

use serde::Serialize;

use super::types::ToolDef;
use crate::tools;

/// Coarse classification — minimal version of goose's tool inspection chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Reads filesystem / docker state but doesn't change anything.
    ReadOnly,
    /// Spawns processes, writes files, mutates containers.
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    NeedsApproval,
}

/// Static metadata for one tool. The agent loop hands this list to the
/// provider on every turn (so the model knows which tools exist), and uses
/// `permission` for the auto-approve/needs-approval gate before dispatch.
#[derive(Debug, Clone, Serialize)]
pub struct RegisteredTool {
    pub def: ToolDef,
    pub permission: Permission,
}

/// Hand-rolled registry. The order matches the suggested workflow but doesn't
/// constrain the LLM — it can call them in any order.
pub fn registry() -> Vec<RegisteredTool> {
    let mut out = Vec::new();
    for m in tools::manifest() {
        let permission = classify(m.name);
        let parameters: serde_json::Value = serde_json::from_str(m.parameters)
            .unwrap_or_else(|_| serde_json::json!({"type": "object"}));
        out.push(RegisteredTool {
            def: ToolDef {
                name: m.name.to_string(),
                description: m.description.to_string(),
                parameters,
            },
            permission,
        });
    }
    out
}

fn classify(name: &str) -> Permission {
    match name {
        // Pure observation: scan files, list containers, parse profiler output.
        // `ask_user` lives here because it never mutates anything — it's a
        // pause waiting on a human, which is always safe; the user is the
        // approver by definition.
        "check_docker"
        | "list_directory"
        | "read_file_excerpt"
        | "read_file_lines"
        | "discover_project"
        | "find_image"
        | "detect_runtime"
        | "find_test_runner_for_profiling"
        | "list_containers"
        | "analyze_samples"
        | "ask_user" => Permission::ReadOnly,

        // Anything that runs commands inside the container, writes files, or
        // launches a subprocess: requires approval in the default mode.
        // `ensure_image` shells out to `docker build` / `docker pull`, which
        // mutates the local Docker daemon's state — destructive by the same
        // bar we use for install_profiler.
        "ensure_image"
        | "install_profiler"
        | "drive_load"
        | "run_profiling"
        | "exec_in_container"
        | "copy_to_container" => Permission::Destructive,

        // Unknown tool — be conservative.
        _ => Permission::Destructive,
    }
}

/// Apply the configured `Mode` to one tool call. Read-only mode rejects
/// everything destructive; auto mode approves everything; default approves
/// read-only and asks for the rest.
pub fn decide(perm: Permission, mode: Mode) -> Decision {
    match (mode, perm) {
        (Mode::Auto, _) => Decision::Approved,
        (Mode::Default, Permission::ReadOnly) => Decision::Approved,
        (Mode::Default, Permission::Destructive) => Decision::NeedsApproval,
        (Mode::ReadOnly, Permission::ReadOnly) => Decision::Approved,
        (Mode::ReadOnly, Permission::Destructive) => Decision::NeedsApproval,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Read-only auto-approved; everything else asks for approval. **Default.**
    #[default]
    Default,
    /// Every tool runs without prompting. Use when the user kicked off an
    /// autonomous run and explicitly opted in.
    Auto,
    /// Anything destructive is denied outright. The model must finish using
    /// only read-only tools or text.
    ReadOnly,
}

/// Run an approved tool by name. Returns a JSON-stringified payload (the
/// model expects strings) plus an error flag — the agent loop wraps this
/// into a `MessageContent::ToolResponse` either way.
pub async fn dispatch(name: &str, arguments: serde_json::Value) -> (String, bool) {
    let started = std::time::Instant::now();
    tracing::info!(
        target: "drift::tool",
        tool = %name,
        args = %{
            let s = arguments.to_string();
            if s.len() > 240 { format!("{}…(+{} chars)", &s[..240], s.len() - 240) } else { s }
        },
        "▶ tool start"
    );
    let (payload, is_err) = match dispatch_inner(name, arguments).await {
        Ok(payload) => (payload, false),
        Err(e) => (format!("error: {e:#}"), true),
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if is_err {
        tracing::warn!(
            target: "drift::tool",
            tool = %name,
            elapsed_ms,
            err = %{
                if payload.len() > 240 { format!("{}…(+{} chars)", &payload[..240], payload.len() - 240) } else { payload.clone() }
            },
            "✗ tool failed"
        );
    } else {
        tracing::info!(
            target: "drift::tool",
            tool = %name,
            elapsed_ms,
            result_len = payload.len(),
            "✓ tool ok"
        );
    }
    (payload, is_err)
}

async fn dispatch_inner(name: &str, arguments: serde_json::Value) -> anyhow::Result<String> {
    use anyhow::Context;

    // Each arm parses `arguments` into the tool's `Args` struct, runs the
    // existing async impl, then JSON-encodes its `Output`. The model only
    // sees a string — same shape for every tool.
    let payload: String = match name {
        n if n == tools::check_docker::NAME => {
            let args: tools::check_docker::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::check_docker::run(args).await?)?
        }
        n if n == tools::list_directory::NAME => {
            let args: tools::list_directory::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::list_directory::run(args).await?)?
        }
        n if n == tools::read_file_excerpt::NAME => {
            let args: tools::read_file_excerpt::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::read_file_excerpt::run(args).await?)?
        }
        n if n == tools::read_file_lines::NAME => {
            let args: tools::read_file_lines::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::read_file_lines::run(args).await?)?
        }
        n if n == tools::discover_project::NAME => {
            let args: tools::discover_project::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::discover_project::run(args).await?)?
        }
        n if n == tools::find_test_runner_for_profiling::NAME => {
            let args: tools::find_test_runner_for_profiling::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::find_test_runner_for_profiling::run(args).await?)?
        }
        n if n == tools::find_image::NAME => {
            let args: tools::find_image::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::find_image::run(args).await?)?
        }
        n if n == tools::ensure_image::NAME => {
            let args: tools::ensure_image::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::ensure_image::run(args).await?)?
        }
        n if n == tools::detect_runtime::NAME => {
            let args: tools::detect_runtime::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::detect_runtime::run(args).await?)?
        }
        n if n == tools::install_profiler::NAME => {
            let args: tools::install_profiler::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::install_profiler::run(args).await?)?
        }
        n if n == tools::drive_load::NAME => {
            let args: tools::drive_load::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::drive_load::run(args).await?)?
        }
        n if n == tools::run_profiling::NAME => {
            let args: tools::run_profiling::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::run_profiling::run(args).await?)?
        }
        n if n == tools::analyze_samples::NAME => {
            let args: tools::analyze_samples::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::analyze_samples::run(args).await?)?
        }
        n if n == tools::list_containers::NAME => {
            let args: tools::list_containers::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::list_containers::run(args).await?)?
        }
        n if n == tools::exec_in_container::NAME => {
            let args: tools::exec_in_container::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::exec_in_container::run(args).await?)?
        }
        n if n == tools::copy_to_container::NAME => {
            let args: tools::copy_to_container::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::copy_to_container::run(args).await?)?
        }
        n if n == tools::ask_user::NAME => {
            let args: tools::ask_user::Args = serde_json::from_value(arguments)
                .with_context(|| format!("invalid args for {n}"))?;
            serde_json::to_string(&tools::ask_user::run(args).await?)?
        }
        other => anyhow::bail!("unknown tool: {other}"),
    };
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_every_tool() {
        let names: Vec<_> = registry().iter().map(|t| t.def.name.clone()).collect();
        assert!(names.contains(&"find_image".to_string()));
        assert!(names.contains(&"analyze_samples".to_string()));
        assert!(names.contains(&"exec_in_container".to_string()));
        assert_eq!(names.len(), tools::manifest().len());
    }

    #[test]
    fn classify_marks_destructive_writes() {
        assert_eq!(classify("find_image"), Permission::ReadOnly);
        assert_eq!(classify("install_profiler"), Permission::Destructive);
        assert_eq!(classify("exec_in_container"), Permission::Destructive);
    }

    #[test]
    fn decide_default_mode_only_passes_read_only() {
        assert_eq!(decide(Permission::ReadOnly, Mode::Default), Decision::Approved);
        assert_eq!(decide(Permission::Destructive, Mode::Default), Decision::NeedsApproval);
    }

    #[test]
    fn decide_auto_mode_passes_everything() {
        assert_eq!(decide(Permission::Destructive, Mode::Auto), Decision::Approved);
    }

    #[test]
    fn decide_read_only_mode_blocks_destructive() {
        assert_eq!(decide(Permission::Destructive, Mode::ReadOnly), Decision::NeedsApproval);
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_payload() {
        let (payload, is_err) = dispatch("nonexistent_tool", serde_json::json!({})).await;
        assert!(is_err);
        assert!(payload.contains("unknown tool"));
    }
}
