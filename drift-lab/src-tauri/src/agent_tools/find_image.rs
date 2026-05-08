//! Rig `Tool` adapter for [`crate::tools::find_image`].
//!
//! The underlying function already does the work — this is purely about
//! exposing it with the JSON Schema and tagged error type that rig expects.

use rig::completion::ToolDefinition;
use rig::tool::{Tool, ToolError};
use thiserror::Error;

use crate::tools::find_image as inner;

#[derive(Debug, Error)]
pub enum FindImageError {
    #[error("find_image failed: {0}")]
    Failed(String),
}

/// Zero-sized — `find_image` doesn't need shared state. Tools that need DB or
/// Docker handles take an `Arc<Mutex<…>>` field at construction.
pub struct FindImage;

impl Tool for FindImage {
    const NAME: &'static str = inner::NAME;
    type Error = FindImageError;
    type Args = inner::Args;
    type Output = inner::Output;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        // The existing `tools::find_image::PARAMETERS` is a JSON-schema
        // string; rig wants a `serde_json::Value`.
        let parameters: serde_json::Value =
            serde_json::from_str(inner::PARAMETERS).unwrap_or_else(|e| {
                tracing::error!("find_image PARAMETERS is not valid JSON schema: {e:?}");
                serde_json::json!({"type": "object"})
            });
        ToolDefinition {
            name: inner::NAME.to_string(),
            description: inner::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        inner::run(args)
            .await
            .map_err(|e| FindImageError::Failed(e.to_string()))
    }
}

// `ToolError` is rig's `From<E>`-friendly wrapper. We rely on
// `Tool::Error: std::error::Error` (satisfied via `thiserror`) — rig converts
// internally.
#[allow(dead_code)]
fn _assert_into_tool_error(e: FindImageError) -> ToolError {
    ToolError::ToolCallError(Box::new(e))
}
