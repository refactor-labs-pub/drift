//! Stage 4 — turn raw profiler output into a ranked list of issues.
//!
//! Supports two input formats today:
//!   - **folded** (collapsed): one `;`-separated stack per line followed by
//!     a sample count. This is what `stackcollapse-perf` and async-profiler's
//!     `-o collapsed` emit. The lingua franca of flame-graph tooling.
//!   - **speedscope** JSON: py-spy's default `--format speedscope`. We pull
//!     out the first sampled profile and convert it to folded form internally.
//!
//! Heuristics rank frames by *self time* (samples where the frame is the
//! leaf), then categorise common patterns: database I/O, network I/O, lock
//! contention, GC, JSON / serde, pure CPU. The LLM gets a small, structured
//! list it can reason about — not a flame graph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::ToolManifest;

pub const NAME: &str = "analyze_samples";
pub const DESCRIPTION: &str =
    "Parse a profiler sample file and return the top-N ranked hotspots, each with a category \
     (db, network, cpu, lock, gc, serde, unknown), self-time and total-time percentages.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "sample_path": { "type": "string", "description": "Path on the host filesystem." },
    "format": {
      "type": "string",
      "enum": ["folded", "speedscope"],
      "description": "Auto-detected from the file extension if omitted."
    },
    "top_n": { "type": "integer", "description": "How many issues to return. Default 10." }
  },
  "required": ["sample_path"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub sample_path: String,
    pub format: Option<Format>,
    pub top_n: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Folded,
    Speedscope,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Database,
    Network,
    Cpu,
    Lock,
    Gc,
    Serde,
    Filesystem,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub rank: u32,
    pub function: String,
    pub category: Category,
    pub severity: Severity,
    pub self_pct: f64,
    pub total_pct: f64,
    pub samples: u64,
    /// One representative stack ending at this frame, for context.
    pub example_stack: String,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub total_samples: u64,
    pub issues: Vec<Issue>,
    pub critical_count: u32,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let path = PathBuf::from(&args.sample_path);
    let format = args.format.unwrap_or_else(|| detect_format(&path));
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;

    let folded = match format {
        Format::Folded => raw,
        Format::Speedscope => speedscope_to_folded(&raw)?,
    };

    let mut frames: HashMap<String, FrameStats> = HashMap::new();
    let mut total_samples: u64 = 0;
    for line in folded.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (stack, count) = match split_folded_line(line) {
            Some(parts) => parts,
            None => continue,
        };
        total_samples += count;

        let frames_in_stack: Vec<&str> = stack.split(';').collect();
        // Self-time goes to the leaf only; total-time to every frame in the stack.
        if let Some(leaf) = frames_in_stack.last() {
            let entry = frames.entry((*leaf).to_string()).or_default();
            entry.self_samples += count;
            if entry.example_stack.is_empty() {
                entry.example_stack = stack.to_string();
            }
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for frame in &frames_in_stack {
            if seen.insert(*frame) {
                let entry = frames.entry((*frame).to_string()).or_default();
                entry.total_samples += count;
            }
        }
    }

    if total_samples == 0 {
        return Err(anyhow!("sample file contained no folded stacks"));
    }

    let mut sorted: Vec<(String, FrameStats)> = frames.into_iter().collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.1.self_samples));

    let top_n = args.top_n.unwrap_or(10);
    let mut issues = Vec::with_capacity(top_n);
    let mut critical_count: u32 = 0;
    for (rank, (function, stats)) in sorted.into_iter().take(top_n).enumerate() {
        let self_pct = (stats.self_samples as f64 / total_samples as f64) * 100.0;
        let total_pct = (stats.total_samples as f64 / total_samples as f64) * 100.0;
        let category = categorise(&function);
        let severity = severity_of(self_pct, total_pct);
        if matches!(severity, Severity::Critical) {
            critical_count += 1;
        }
        issues.push(Issue {
            rank: rank as u32 + 1,
            function,
            category,
            severity,
            self_pct,
            total_pct,
            samples: stats.self_samples,
            example_stack: stats.example_stack,
        });
    }

    Ok(Output {
        total_samples,
        issues,
        critical_count,
    })
}

#[derive(Default)]
struct FrameStats {
    self_samples: u64,
    total_samples: u64,
    example_stack: String,
}

fn detect_format(path: &Path) -> Format {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => Format::Speedscope,
        _ => Format::Folded,
    }
}

fn split_folded_line(line: &str) -> Option<(&str, u64)> {
    let last_space = line.rfind(' ')?;
    let (stack, count) = line.split_at(last_space);
    let count: u64 = count.trim().parse().ok()?;
    Some((stack, count))
}

/// Convert speedscope's "sampled" profile into folded form. We rebuild each
/// stack from the `frames` table and emit one line per unique stack.
fn speedscope_to_folded(raw: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(raw).context("parse speedscope json")?;
    let frames = v
        .get("shared")
        .and_then(|s| s.get("frames"))
        .and_then(|f| f.as_array())
        .ok_or_else(|| anyhow!("speedscope: missing shared.frames"))?;
    let frame_names: Vec<String> = frames
        .iter()
        .map(|f| {
            f.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("?")
                .to_string()
        })
        .collect();

    let profiles = v
        .get("profiles")
        .and_then(|p| p.as_array())
        .ok_or_else(|| anyhow!("speedscope: missing profiles"))?;
    let profile = profiles
        .first()
        .ok_or_else(|| anyhow!("speedscope: empty profiles array"))?;

    let samples = profile
        .get("samples")
        .and_then(|s| s.as_array())
        .ok_or_else(|| anyhow!("speedscope: profile has no samples (only sampled profiles supported)"))?;
    let weights = profile
        .get("weights")
        .and_then(|w| w.as_array())
        .map(|arr| arr.iter().filter_map(|w| w.as_u64()).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut buckets: HashMap<String, u64> = HashMap::new();
    for (i, sample) in samples.iter().enumerate() {
        let stack: Vec<String> = sample
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|idx| idx.as_u64())
            .filter_map(|idx| frame_names.get(idx as usize).cloned())
            .collect();
        if stack.is_empty() {
            continue;
        }
        let key = stack.join(";");
        let weight = weights.get(i).copied().unwrap_or(1);
        *buckets.entry(key).or_insert(0) += weight;
    }

    let mut out = String::new();
    for (stack, count) in buckets {
        out.push_str(&stack);
        out.push(' ');
        out.push_str(&count.to_string());
        out.push('\n');
    }
    Ok(out)
}

fn categorise(frame: &str) -> Category {
    let f = frame.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|n| f.contains(n));

    if any(&["psycopg", "sqlalchemy", "pymongo", "redis", "asyncpg", "mysql", "sqlite", "execute_query", "select * from", "jdbc", "hibernate"]) {
        return Category::Database;
    }
    if any(&["aiohttp", "httpx", "requests.api", "urllib", "socket.recv", "socket.send", "tcp_", "okhttp", "reqwest"]) {
        return Category::Network;
    }
    if any(&["lock", "mutex", "rwlock", "synchron", "semaphore", "condvar"]) {
        return Category::Lock;
    }
    if any(&["gc.", "garbage", "mark_sweep", "g1_", "young_gen", "old_gen"]) {
        return Category::Gc;
    }
    if any(&["json", "serde", "pickle", "msgpack", "protobuf", "marshal"]) {
        return Category::Serde;
    }
    if any(&["read", "write", "fsync", "open(", "fs::", "fileio"]) {
        return Category::Filesystem;
    }
    if any(&["loop", "compute", "encode", "decode", "hash", "regex"]) {
        return Category::Cpu;
    }
    Category::Unknown
}

fn severity_of(self_pct: f64, total_pct: f64) -> Severity {
    if self_pct >= 20.0 || total_pct >= 60.0 {
        Severity::Critical
    } else if self_pct >= 10.0 || total_pct >= 35.0 {
        Severity::High
    } else if self_pct >= 3.0 || total_pct >= 15.0 {
        Severity::Medium
    } else {
        Severity::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_ranks_self_time() {
        let folded = "main;handle;psycopg.cursor.execute 100\nmain;handle;psycopg.cursor.execute;send 50\nmain;handle;render 25\n";
        let path = std::env::temp_dir().join("drift-test-folded.txt");
        std::fs::write(&path, folded).unwrap();

        let out = futures_executor_block(run(Args {
            sample_path: path.display().to_string(),
            format: Some(Format::Folded),
            top_n: Some(5),
        }));

        let report = out.unwrap();
        // psycopg.execute is a leaf for 100 samples → highest self-time.
        assert_eq!(report.issues[0].function, "psycopg.cursor.execute");
        assert!(matches!(report.issues[0].category, Category::Database));
        assert_eq!(report.total_samples, 175);
    }

    #[test]
    fn split_folded_line_parses_count() {
        let (stack, count) = split_folded_line("a;b;c 42").unwrap();
        assert_eq!(stack, "a;b;c");
        assert_eq!(count, 42);
    }

    #[test]
    fn split_folded_line_rejects_non_numeric() {
        assert!(split_folded_line("a;b;c xyz").is_none());
    }

    #[test]
    fn split_folded_line_rejects_no_space() {
        assert!(split_folded_line("just_a_stack").is_none());
    }

    #[test]
    fn categorise_database_frames() {
        assert!(matches!(categorise("psycopg2.execute"), Category::Database));
        assert!(matches!(categorise("SQLAlchemy.session.flush"), Category::Database));
        assert!(matches!(categorise("redis.get"), Category::Database));
    }

    #[test]
    fn categorise_network_frames() {
        assert!(matches!(categorise("aiohttp.connect"), Category::Network));
        assert!(matches!(categorise("socket.recv"), Category::Network));
    }

    #[test]
    fn categorise_lock_gc_serde_filesystem() {
        assert!(matches!(categorise("std::sync::Mutex::lock"), Category::Lock));
        assert!(matches!(categorise("gc.collect"), Category::Gc));
        assert!(matches!(categorise("json.dumps"), Category::Serde));
        assert!(matches!(categorise("os.read"), Category::Filesystem));
    }

    #[test]
    fn categorise_unknown_when_nothing_matches() {
        assert!(matches!(categorise("my_business_logic"), Category::Unknown));
    }

    #[test]
    fn severity_thresholds() {
        assert!(matches!(severity_of(25.0, 0.0), Severity::Critical));
        assert!(matches!(severity_of(0.0, 70.0), Severity::Critical));
        assert!(matches!(severity_of(15.0, 0.0), Severity::High));
        assert!(matches!(severity_of(5.0, 0.0), Severity::Medium));
        assert!(matches!(severity_of(0.5, 1.0), Severity::Low));
    }

    #[test]
    fn detect_format_from_extension() {
        use std::path::PathBuf;
        assert!(matches!(detect_format(&PathBuf::from("a.json")), Format::Speedscope));
        assert!(matches!(detect_format(&PathBuf::from("a.folded")), Format::Folded));
        assert!(matches!(detect_format(&PathBuf::from("a")), Format::Folded));
    }

    #[test]
    fn speedscope_converts_to_folded_with_weights() {
        // Two-frame profile: stack [0,1] sampled twice with weights 3 and 7.
        let raw = r#"{
            "shared": { "frames": [{"name": "main"}, {"name": "work"}] },
            "profiles": [{
                "samples": [[0, 1], [0, 1]],
                "weights": [3, 7]
            }]
        }"#;
        let folded = speedscope_to_folded(raw).unwrap();
        // Both samples collapse to the same stack `main;work`, weights sum to 10.
        let line = folded.lines().next().unwrap();
        let (stack, count) = split_folded_line(line).unwrap();
        assert_eq!(stack, "main;work");
        assert_eq!(count, 10);
    }

    #[test]
    fn analyse_speedscope_end_to_end() {
        let raw = r#"{
            "shared": { "frames": [{"name": "psycopg.execute"}, {"name": "render"}] },
            "profiles": [{
                "samples": [[0], [1]],
                "weights": [80, 20]
            }]
        }"#;
        let path = std::env::temp_dir().join("drift-test-speedscope.json");
        std::fs::write(&path, raw).unwrap();

        let report = futures_executor_block(run(Args {
            sample_path: path.display().to_string(),
            format: None,
            top_n: Some(5),
        }))
        .unwrap();
        assert_eq!(report.total_samples, 100);
        assert_eq!(report.issues[0].function, "psycopg.execute");
        assert!(matches!(report.issues[0].category, Category::Database));
        // 80% self-time → Critical
        assert!(matches!(report.issues[0].severity, Severity::Critical));
    }

    #[test]
    fn empty_sample_file_errors() {
        let path = std::env::temp_dir().join("drift-test-empty.folded");
        std::fs::write(&path, "").unwrap();
        let err = futures_executor_block(run(Args {
            sample_path: path.display().to_string(),
            format: Some(Format::Folded),
            top_n: None,
        }))
        .unwrap_err();
        assert!(err.to_string().contains("no folded stacks"));
    }

    fn futures_executor_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }
}
