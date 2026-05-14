//! Read a window of source around a specific line.
//!
//! Companion to `read_file_excerpt`, but **anchor-centric**: callers pass the
//! line they care about and a half-window size; we return `line ± context`.
//! The static-scan suggester calls this for every finding to feed the LLM
//! exactly the code around the smell — the model gets only what it needs to
//! suggest a fix, not the whole file.
//!
//! This is the **only** filesystem touch the suggestion phase needs; the LLM
//! itself never decides what to read. That keeps the responsibility clean:
//!   - static analyzer → produces the finding (file, line, kind)
//!   - this tool → produces the code window
//!   - LLM → proposes a code change
//!
//! Default context is 30 lines either side (61-line window total) — wide
//! enough to see the enclosing function on most languages, narrow enough to
//! stay within a small-model context budget.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::ToolManifest;

pub const NAME: &str = "read_file_lines";
pub const DESCRIPTION: &str =
    "Read a window of `2*context + 1` lines centred on `line` (1-indexed). Use this when you have \
     a known line number from a static-analysis finding and want to see the surrounding code. \
     Default context is 30 lines each side; max 200.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "path": {"type": "string", "description": "Absolute path to the file."},
    "line": {"type": "integer", "description": "1-indexed line to centre the window on."},
    "context": {"type": "integer", "description": "Lines either side of `line` (default 30, max 200)."}
  },
  "required": ["path", "line"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub path: String,
    pub line: u32,
    pub context: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub path: String,
    /// 1-indexed line at the centre of the window.
    pub anchor_line: u32,
    /// 1-indexed line of the first returned line (anchor − context, ≥ 1).
    pub start_line: u32,
    /// 1-indexed line of the last returned line.
    pub end_line: u32,
    /// Total lines in the file.
    pub total_lines: u32,
    /// Each entry is one line **without** trailing newline. Index `0` is at
    /// `start_line`. The UI can `lines.join("\n")` to render, or zip with
    /// indices to render gutter line numbers.
    pub lines: Vec<String>,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

const DEFAULT_CONTEXT: u32 = 30;
const MAX_CONTEXT: u32 = 200;
/// Same cap as `read_file_excerpt` — 5 MB is well past any sensible source
/// file; anything bigger is almost certainly a build artefact.
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

pub async fn run(args: Args) -> Result<Output> {
    if args.line == 0 {
        anyhow::bail!("`line` is 1-indexed and must be ≥ 1");
    }
    let path = PathBuf::from(&args.path);
    let meta = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("stat {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("not a file: {}", path.display());
    }
    if meta.len() > MAX_FILE_SIZE {
        anyhow::bail!(
            "file too large: {} bytes (cap {} bytes)",
            meta.len(),
            MAX_FILE_SIZE
        );
    }

    // Binary sniff — same heuristic as read_file_excerpt.
    let probe = tokio::fs::read(&path)
        .await
        .with_context(|| format!("probe {}", path.display()))?;
    if probe.iter().take(4096).any(|b| *b == 0) {
        anyhow::bail!("looks binary: {}", path.display());
    }

    let ctx = args.context.unwrap_or(DEFAULT_CONTEXT).min(MAX_CONTEXT);
    let start = args.line.saturating_sub(ctx).max(1);
    let end = args.line.saturating_add(ctx);

    let f = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(f).lines();
    let mut current: u32 = 0;
    let mut total: u32 = 0;
    let mut lines = Vec::with_capacity(((end - start) + 1) as usize);
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                current += 1;
                total += 1;
                if current >= start && current <= end {
                    lines.push(line);
                }
                // Don't break early — we still want a correct `total_lines`
                // for the caller. Files are small (≤ 5 MB) so the scan cost
                // is negligible compared to the rest of the agent loop.
            }
            Ok(None) => break,
            Err(e) => return Err(e).context("reading lines"),
        }
    }

    // Clamp end to the actual EOF so callers see a truthful upper bound.
    let end_clamped = end.min(total).max(start);

    Ok(Output {
        path: path.display().to_string(),
        anchor_line: args.line,
        start_line: start,
        end_line: end_clamped,
        total_lines: total,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join(format!("drift-rfl-{name}-{}.txt", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[tokio::test]
    async fn returns_window_around_anchor() {
        let body: String = (1..=100).map(|i| format!("line{i}\n")).collect();
        let p = write_tmp("centred", &body);
        let out = run(Args {
            path: p.display().to_string(),
            line: 50,
            context: Some(2),
        })
        .await
        .unwrap();
        assert_eq!(out.start_line, 48);
        assert_eq!(out.end_line, 52);
        assert_eq!(out.total_lines, 100);
        assert_eq!(out.lines, vec!["line48", "line49", "line50", "line51", "line52"]);
    }

    #[tokio::test]
    async fn clamps_start_to_one_near_top() {
        let body: String = (1..=10).map(|i| format!("L{i}\n")).collect();
        let p = write_tmp("top", &body);
        let out = run(Args {
            path: p.display().to_string(),
            line: 2,
            context: Some(5),
        })
        .await
        .unwrap();
        assert_eq!(out.start_line, 1);
        assert_eq!(out.end_line, 7);
        assert_eq!(out.lines.first().unwrap(), "L1");
    }

    #[tokio::test]
    async fn clamps_end_to_total_near_eof() {
        let body: String = (1..=10).map(|i| format!("L{i}\n")).collect();
        let p = write_tmp("eof", &body);
        let out = run(Args {
            path: p.display().to_string(),
            line: 9,
            context: Some(5),
        })
        .await
        .unwrap();
        assert_eq!(out.start_line, 4);
        assert_eq!(out.end_line, 10);
    }

    #[tokio::test]
    async fn rejects_zero_line() {
        let p = write_tmp("zero", "x\n");
        let err = run(Args {
            path: p.display().to_string(),
            line: 0,
            context: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1-indexed"));
    }

    #[tokio::test]
    async fn caps_context_at_hard_max() {
        let body: String = (1..=100).map(|i| format!("L{i}\n")).collect();
        let p = write_tmp("cap", &body);
        let out = run(Args {
            path: p.display().to_string(),
            line: 50,
            context: Some(1000),
        })
        .await
        .unwrap();
        // 1..=100 clamps both ends.
        assert_eq!(out.start_line, 1);
        assert_eq!(out.end_line, 100);
    }
}
