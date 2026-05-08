//! Stage 0 — locate the Docker image the user wants profiled.
//!
//! Resolution order:
//!   1. `docker-compose.yml` / `compose.yaml` — pick the first service with a
//!      named `image:` field, or the first service with a `build:` block.
//!   2. `Dockerfile` at the project root — image must be built; we report the
//!      build context and synthesise a tag of the form `drift-lab/<dirname>`.
//!   3. Otherwise return `None` so the caller can ask the user.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::ToolManifest;

pub const NAME: &str = "find_image";
pub const DESCRIPTION: &str =
    "Scan a project directory for a Dockerfile or docker-compose file and resolve which image \
     should be profiled. Returns the image reference plus where it came from.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "Absolute path to the project directory to scan."
    }
  },
  "required": ["path"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Compose,
    Dockerfile,
}

#[derive(Debug, Serialize)]
pub struct Output {
    /// Image reference suitable for `docker pull` / `docker run`.
    /// May be a "to-be-built" synthetic tag if only a Dockerfile was found.
    pub image_ref: String,
    /// `compose` or `dockerfile`.
    pub source: Source,
    /// Path of the manifest we resolved from.
    pub manifest_path: String,
    /// For compose: which service we picked. For Dockerfile: `None`.
    pub compose_service: Option<String>,
    /// Build context relative to the project root, if the image needs building.
    pub build_context: Option<String>,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let root = PathBuf::from(&args.path);
    if !root.is_dir() {
        anyhow::bail!("project path is not a directory: {}", root.display());
    }

    if let Some(out) = try_compose(&root)? {
        return Ok(out);
    }
    if let Some(out) = try_dockerfile(&root)? {
        return Ok(out);
    }
    anyhow::bail!(
        "no Dockerfile or compose manifest found under {}",
        root.display()
    )
}

fn try_compose(root: &Path) -> Result<Option<Output>> {
    for name in ["docker-compose.yml", "docker-compose.yaml", "compose.yaml", "compose.yml"] {
        let candidate = root.join(name);
        if !candidate.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&candidate)
            .with_context(|| format!("read {}", candidate.display()))?;
        let (service, image, build_ctx) = parse_compose(&raw);
        if service.is_none() {
            continue;
        }
        let image_ref = image.unwrap_or_else(|| synthetic_tag(root));
        return Ok(Some(Output {
            image_ref,
            source: Source::Compose,
            manifest_path: candidate.display().to_string(),
            compose_service: service,
            build_context: build_ctx,
        }));
    }
    Ok(None)
}

fn try_dockerfile(root: &Path) -> Result<Option<Output>> {
    let dockerfile = root.join("Dockerfile");
    if !dockerfile.is_file() {
        return Ok(None);
    }
    Ok(Some(Output {
        image_ref: synthetic_tag(root),
        source: Source::Dockerfile,
        manifest_path: dockerfile.display().to_string(),
        compose_service: None,
        build_context: Some(".".to_string()),
    }))
}

/// Tiny line-based compose parser — pulls out the first service block,
/// its `image:` (if any) and its `build:` context. We deliberately avoid a
/// full YAML dependency here; this is a heuristic, not a validator.
fn parse_compose(raw: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut in_services = false;
    let mut current_service: Option<String> = None;
    let mut first_service: Option<String> = None;
    let mut image: Option<String> = None;
    let mut build_ctx: Option<String> = None;

    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();

        if indent == 0 {
            in_services = trimmed.starts_with("services:");
            continue;
        }
        if !in_services {
            continue;
        }

        // Service name lines look like `  myservice:` at indent 2.
        if indent == 2 && trimmed.trim_end().ends_with(':') {
            let name = trimmed.trim().trim_end_matches(':').to_string();
            current_service = Some(name.clone());
            if first_service.is_none() {
                first_service = Some(name);
            }
            continue;
        }

        // Per-service keys at indent 4.
        if indent == 4 && current_service == first_service {
            let body = trimmed.trim_start();
            if let Some(rest) = body.strip_prefix("image:") {
                image = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if let Some(rest) = body.strip_prefix("build:") {
                let val = rest.trim();
                if !val.is_empty() {
                    build_ctx = Some(val.trim_matches('"').trim_matches('\'').to_string());
                }
            }
        } else if indent == 6 && current_service == first_service {
            // `build:\n      context: .`
            let body = trimmed.trim_start();
            if let Some(rest) = body.strip_prefix("context:") {
                build_ctx = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }

    (first_service, image, build_ctx)
}

fn synthetic_tag(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_lowercase();
    format!("drift-lab/{name}:latest")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("drift-lab-find-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_compose_with_image() {
        let raw = "services:\n  api:\n    image: my/api:1.2\n    ports:\n      - \"8080:80\"\n";
        let (svc, image, _) = parse_compose(raw);
        assert_eq!(svc.as_deref(), Some("api"));
        assert_eq!(image.as_deref(), Some("my/api:1.2"));
    }

    #[test]
    fn parses_compose_with_build_context() {
        let raw = "services:\n  worker:\n    build:\n      context: ./worker\n";
        let (svc, image, ctx) = parse_compose(raw);
        assert_eq!(svc.as_deref(), Some("worker"));
        assert!(image.is_none());
        assert_eq!(ctx.as_deref(), Some("./worker"));
    }

    #[test]
    fn parse_compose_skips_comments_and_blanks() {
        let raw = "# top-level comment\nservices:\n\n  api:\n    # service comment\n    image: \"foo:1\"\n";
        let (svc, image, _) = parse_compose(raw);
        assert_eq!(svc.as_deref(), Some("api"));
        assert_eq!(image.as_deref(), Some("foo:1"));
    }

    #[test]
    fn synthetic_tag_lowercases_dirname() {
        let path = std::path::Path::new("/tmp/Checkout-Service");
        assert_eq!(synthetic_tag(path), "drift-lab/checkout-service:latest");
    }

    #[tokio::test]
    async fn run_returns_compose_output_when_compose_present() {
        let dir = tempdir("compose");
        std::fs::write(
            dir.join("docker-compose.yml"),
            "services:\n  api:\n    image: registry/svc:42\n",
        )
        .unwrap();

        let out = run(Args { path: dir.display().to_string() }).await.unwrap();
        assert!(matches!(out.source, Source::Compose));
        assert_eq!(out.image_ref, "registry/svc:42");
        assert_eq!(out.compose_service.as_deref(), Some("api"));
    }

    #[tokio::test]
    async fn run_falls_back_to_dockerfile_with_synthetic_tag() {
        let dir = tempdir("docker");
        std::fs::write(dir.join("Dockerfile"), "FROM python:3.11\n").unwrap();

        let out = run(Args { path: dir.display().to_string() }).await.unwrap();
        assert!(matches!(out.source, Source::Dockerfile));
        assert!(out.image_ref.starts_with("drift-lab/"));
        assert_eq!(out.build_context.as_deref(), Some("."));
        assert!(out.compose_service.is_none());
    }

    #[tokio::test]
    async fn run_errors_when_nothing_found() {
        let dir = tempdir("empty");
        let err = run(Args { path: dir.display().to_string() }).await.unwrap_err();
        assert!(err.to_string().contains("no Dockerfile or compose"));
    }

    #[tokio::test]
    async fn run_errors_when_path_is_not_a_dir() {
        let err = run(Args { path: "/definitely/not/a/dir/zzz".into() })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }
}
