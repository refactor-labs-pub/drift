//! Rig `Tool` adapters around the [`crate::tools`] module. Each existing
//! `tools::*` already exposes `NAME`, `DESCRIPTION`, `PARAMETERS` (raw JSON
//! Schema) plus an `async fn run(args) -> Result<Output>` — wrapping each one
//! as a [`rig::tool::Tool`] is mechanical (~30 LoC per tool).
//!
//! Today we ship one canonical adapter (`find_image`) plus the [`Toolset`]
//! registry. Adding the rest follows the same pattern; track the gap below.

mod find_image;

use rig::agent::{AgentBuilder, NoToolConfig, WithBuilderTools};
use rig::completion::CompletionModel;
use serde::{Deserialize, Serialize};

/// Named bundle of tools. Set per-conversation so users can opt out of
/// destructive tools or scope an agent to read-only operations.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Toolset {
    /// No tools — pure chat. Useful for general help that doesn't touch the
    /// project.
    None,
    /// High-level profiling workflow only (find_image, detect_runtime, etc.).
    /// **Default** — safe enough that the LLM can drive a full profile run.
    #[default]
    Profiling,
    /// Profiling + low-level Docker primitives (exec_in_container,
    /// copy_to_container, list_containers). Power-user mode.
    Full,
}

/// Attach the tools for the given [`Toolset`] to a rig agent builder.
///
/// We take the builder by value and return it because `.tool()` changes the
/// builder's type generic from `()` to `WithBuilderTools` — chaining is the
/// only ergonomic shape.
pub fn install<M, P>(
    builder: AgentBuilder<M, P, NoToolConfig>,
    toolset: Toolset,
) -> AgentBuilder<M, P, WithBuilderTools>
where
    M: CompletionModel + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    match toolset {
        Toolset::None => {
            // No tools — but the chat code expects a `WithBuilderTools`
            // builder regardless. `.tools(vec![])` flips the type generic
            // without registering anything.
            builder.tools(vec![])
        }
        Toolset::Profiling | Toolset::Full => {
            // For now, only `find_image` is wrapped. Other adapters slot in
            // here in the same shape: `.tool(detect_runtime::DetectRuntime)`,
            // etc. The Toolset gate is wired so adding them is a one-liner.
            builder.tool(find_image::FindImage)
        }
    }
}
