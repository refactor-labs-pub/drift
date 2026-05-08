//! Primitive — list running (or all) containers on the local Docker daemon.
//!
//! The agent uses this to map an image reference to a concrete container ID
//! before exec'ing into it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use bollard::container::ListContainersOptions;
use serde::{Deserialize, Serialize};

use super::ToolManifest;
use crate::docker;

pub const NAME: &str = "list_containers";
pub const DESCRIPTION: &str =
    "List local Docker containers, optionally filtered by image reference. Returns running \
     containers by default; pass `all=true` to include stopped ones.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "image": {
      "type": "string",
      "description": "Optional image reference to filter on (matches Image or ImageID)."
    },
    "all": {
      "type": "boolean",
      "description": "Include stopped containers (default false)."
    }
  }
}"#;

#[derive(Debug, Default, Deserialize)]
pub struct Args {
    pub image: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Serialize)]
pub struct Container {
    pub id: String,
    pub image: String,
    pub names: Vec<String>,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub containers: Vec<Container>,
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

    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(image) = &args.image {
        filters.insert("ancestor".to_string(), vec![image.clone()]);
    }

    let opts = ListContainersOptions::<String> {
        all: args.all,
        filters,
        ..Default::default()
    };

    let summaries = docker
        .list_containers(Some(opts))
        .await
        .context("docker list_containers")?;

    let containers = summaries
        .into_iter()
        .map(|c| Container {
            id: c.id.unwrap_or_default(),
            image: c.image.unwrap_or_default(),
            names: c.names.unwrap_or_default(),
            state: c.state.unwrap_or_default(),
            status: c.status.unwrap_or_default(),
        })
        .collect();

    Ok(Output { containers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_default_all_is_false() {
        let args: Args = serde_json::from_str("{}").unwrap();
        assert!(!args.all);
        assert!(args.image.is_none());
    }

    #[test]
    fn args_parse_with_image_filter() {
        let args: Args = serde_json::from_str(r#"{"image": "my/svc:1", "all": true}"#).unwrap();
        assert_eq!(args.image.as_deref(), Some("my/svc:1"));
        assert!(args.all);
    }

    #[test]
    fn manifest_is_well_formed() {
        let m = manifest();
        assert_eq!(m.name, "list_containers");
        // Schema must be valid JSON the LLM can ingest.
        let v: serde_json::Value = serde_json::from_str(m.parameters).unwrap();
        assert_eq!(v["type"], "object");
    }

    /// Live-Docker integration test. Run with `cargo test -- --ignored`.
    /// Skipped by default since CI may not have a Docker daemon.
    #[tokio::test]
    #[ignore = "requires running docker daemon"]
    async fn lists_containers_against_live_daemon() {
        let out = run(Args::default()).await.expect("docker daemon reachable");
        // Don't assert on container count — environment-dependent. Just check
        // the call returns sensibly-shaped data.
        for c in &out.containers {
            assert!(!c.id.is_empty());
        }
    }
}
