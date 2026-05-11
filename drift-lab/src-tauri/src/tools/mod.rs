//! Arsenal of LLM-callable tools.
//!
//! Each submodule defines one self-contained tool with:
//! - `Args`: deserialised from the LLM's JSON arguments
//! - `Output`: serialised back to the LLM as the tool result
//! - `run(args) -> Result<Output>`: the actual implementation, also callable
//!   directly from `workflow.rs` and tests
//! - `NAME` / `DESCRIPTION` / `PARAMETERS`: the manifest used to register the
//!   tool with the agent (rig / OpenAI tool-use schema)
//!
//! The split between primitives (`list_containers`, `exec_in_container`,
//! `copy_to_container`) and high-level stages (`find_image`, `detect_runtime`,
//! `install_profiler`, `drive_load`, `run_profiling`, `analyze_samples`) is
//! deliberate: the LLM gets both the safe one-shot stages AND the building
//! blocks, so it can recover from edge cases without us having to anticipate
//! every workflow.

pub mod analyze_samples;
pub mod ask_user;
pub mod check_docker;
pub mod copy_to_container;
pub mod detect_runtime;
pub mod discover_project;
pub mod drive_load;
pub mod ensure_image;
pub mod exec_in_container;
pub mod find_image;
pub mod find_test_runner_for_profiling;
pub mod install_profiler;
pub mod list_containers;
pub mod list_directory;
pub mod read_file_excerpt;
pub mod run_profiling;

use serde::Serialize;

/// Manifest entry shipped to the LLM at agent-construction time.
#[derive(Debug, Clone, Serialize)]
pub struct ToolManifest {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON-schema (draft-07 subset) string describing `Args`.
    pub parameters: &'static str,
}

/// Full manifest of every tool available to the agent. Order is the
/// suggested reasoning order for the standard profiling workflow, but the
/// LLM is free to call them in any order.
pub fn manifest() -> Vec<ToolManifest> {
    vec![
        // Read-only project investigation — the agent uses these first to
        // understand a new codebase before deciding which heavyweight tools
        // (profiler, load driver) to invoke. `check_docker` runs first so we
        // can surface a clear "install Docker" prompt before any tool that
        // touches the daemon fails with a cryptic error.
        check_docker::manifest(),
        list_directory::manifest(),
        read_file_excerpt::manifest(),
        discover_project::manifest(),
        find_image::manifest(),
        // `ensure_image` runs after `find_image` and before `detect_runtime` —
        // it materialises the image (build or pull) so the inspect call that
        // detect_runtime needs doesn't 404 on a never-built tag.
        ensure_image::manifest(),
        detect_runtime::manifest(),
        find_test_runner_for_profiling::manifest(),
        // Mutating profiler workflow.
        install_profiler::manifest(),
        drive_load::manifest(),
        run_profiling::manifest(),
        analyze_samples::manifest(),
        list_containers::manifest(),
        exec_in_container::manifest(),
        copy_to_container::manifest(),
        // Meta — the agent calls this to park on a free-text human question
        // when it genuinely can't proceed. Last in the list because it's
        // the recovery hatch, not the happy path.
        ask_user::manifest(),
    ]
}
