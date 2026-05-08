//! Stage 3b — capture samples while the workload is under load.
//!
//! This is intentionally separate from `drive_load` so the LLM can sequence
//! them however it wants: warm up → start profiler → drive load → stop. The
//! recommended pattern is:
//!   1. exec the profiler in `detach=true` mode to start recording
//!   2. call `drive_load` to generate traffic
//!   3. call this tool with `mode=Stop` to flush samples to disk
//!
//! Or, for a single-shot capture, call with `mode=OneShot` and a duration —
//! we'll start, sleep, stop, and copy the sample file out automatically.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use super::detect_runtime::Profiler;
use super::{exec_in_container, ToolManifest};

pub const NAME: &str = "run_profiling";
pub const DESCRIPTION: &str =
    "Run a profiler attached to a target PID inside a container and produce a sample file. Use \
     `OneShot` for a fixed-duration capture or `Start`/`Stop` to bracket an external load run.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "container_id": { "type": "string" },
    "profiler": {
      "type": "string",
      "enum": ["py-spy", "async-profiler", "perf", "node-clinic", "rbspy", "dotrace"]
    },
    "binary_path": { "type": "string" },
    "target_pid": { "type": "integer" },
    "mode": {
      "type": "string",
      "enum": ["one_shot", "start", "stop"]
    },
    "duration_secs": {
      "type": "integer",
      "description": "Required for OneShot. Ignored for Start/Stop."
    },
    "sample_format": {
      "type": "string",
      "enum": ["speedscope", "raw", "folded", "jfr"],
      "description": "Output format. Defaults to the profiler's native format."
    }
  },
  "required": ["container_id", "profiler", "binary_path", "target_pid", "mode"]
}"#;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    OneShot,
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    Speedscope,
    Raw,
    Folded,
    Jfr,
}

#[derive(Debug, Deserialize)]
pub struct Args {
    pub container_id: String,
    pub profiler: Profiler,
    pub binary_path: String,
    pub target_pid: i32,
    pub mode: Mode,
    pub duration_secs: Option<u32>,
    pub sample_format: Option<SampleFormat>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    /// Path inside the container where samples were written.
    pub sample_path_in_container: String,
    /// Best-effort sample count parsed from profiler stderr. `None` if unknown.
    pub samples_collected: Option<u64>,
    pub sample_format: SampleFormat,
    pub stdout: String,
    pub stderr: String,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let format = args.sample_format.unwrap_or_else(|| default_format(args.profiler));
    let sample_path = sample_path_for(args.profiler, args.target_pid, format);

    match (args.profiler, args.mode) {
        (Profiler::PySpy, Mode::OneShot) => pyspy_one_shot(&args, &sample_path, format).await,
        (Profiler::PySpy, Mode::Start) => pyspy_start(&args, &sample_path, format).await,
        (Profiler::PySpy, Mode::Stop) => pyspy_stop(&args, &sample_path, format).await,
        (Profiler::AsyncProfiler, Mode::OneShot) => asprof_one_shot(&args, &sample_path).await,
        (Profiler::AsyncProfiler, Mode::Start) => asprof_action(&args, "start", &sample_path).await,
        (Profiler::AsyncProfiler, Mode::Stop) => asprof_action(&args, "stop", &sample_path).await,
        (Profiler::Perf, Mode::OneShot) => perf_one_shot(&args, &sample_path).await,
        (other, _) => Err(anyhow!(
            "run_profiling not yet implemented for profiler {:?}",
            other
        )),
    }
}

fn default_format(p: Profiler) -> SampleFormat {
    match p {
        Profiler::PySpy => SampleFormat::Speedscope,
        Profiler::AsyncProfiler => SampleFormat::Jfr,
        Profiler::Perf => SampleFormat::Folded,
        _ => SampleFormat::Raw,
    }
}

fn sample_path_for(p: Profiler, pid: i32, fmt: SampleFormat) -> String {
    let ext = match fmt {
        SampleFormat::Speedscope => "json",
        SampleFormat::Raw => "raw",
        SampleFormat::Folded => "folded",
        SampleFormat::Jfr => "jfr",
    };
    let prefix = match p {
        Profiler::PySpy => "pyspy",
        Profiler::AsyncProfiler => "asprof",
        Profiler::Perf => "perf",
        _ => "profile",
    };
    format!("/tmp/{prefix}-{pid}.{ext}")
}

async fn pyspy_one_shot(
    args: &Args,
    sample_path: &str,
    format: SampleFormat,
) -> Result<Output> {
    let duration = args
        .duration_secs
        .ok_or_else(|| anyhow!("OneShot requires duration_secs"))?;
    let format_flag = match format {
        SampleFormat::Speedscope => "speedscope",
        SampleFormat::Raw => "raw",
        SampleFormat::Folded => "flamegraph",
        _ => "speedscope",
    };
    let cmd = format!(
        "{} record --pid {} --duration {} --format {} --output {} --rate 100 --nonblocking",
        args.binary_path, args.target_pid, duration, format_flag, sample_path
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(duration as u64 + 30),
    })
    .await?;
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: parse_pyspy_samples(&out.stderr),
        sample_format: format,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

async fn pyspy_start(
    args: &Args,
    sample_path: &str,
    format: SampleFormat,
) -> Result<Output> {
    let format_flag = match format {
        SampleFormat::Speedscope => "speedscope",
        SampleFormat::Raw => "raw",
        SampleFormat::Folded => "flamegraph",
        _ => "speedscope",
    };
    // py-spy doesn't have an explicit start/stop; we run record with a long
    // duration in the background and rely on `Stop` to kill it.
    let cmd = format!(
        "nohup {} record --pid {} --duration 3600 --format {} --output {} --rate 100 --nonblocking >/tmp/pyspy.log 2>&1 & echo $!",
        args.binary_path, args.target_pid, format_flag, sample_path
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(10),
    })
    .await?;
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: None,
        sample_format: format,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

async fn pyspy_stop(args: &Args, sample_path: &str, format: SampleFormat) -> Result<Output> {
    // Send SIGINT to py-spy so it flushes the file cleanly.
    let cmd = "pkill -INT -f 'py-spy record' || true; sleep 1";
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd.into()],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(10),
    })
    .await?;
    let log = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["cat".into(), "/tmp/pyspy.log".into()],
        user: None,
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(5),
    })
    .await
    .ok();
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: log.as_ref().and_then(|l| parse_pyspy_samples(&l.stdout)),
        sample_format: format,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

async fn asprof_one_shot(args: &Args, sample_path: &str) -> Result<Output> {
    let duration = args
        .duration_secs
        .ok_or_else(|| anyhow!("OneShot requires duration_secs"))?;
    let cmd = format!(
        "{} -d {} -f {} {}",
        args.binary_path, duration, sample_path, args.target_pid
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(duration as u64 + 30),
    })
    .await?;
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: None,
        sample_format: SampleFormat::Jfr,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

async fn asprof_action(args: &Args, action: &str, sample_path: &str) -> Result<Output> {
    let cmd = format!(
        "{} {} -f {} {}",
        args.binary_path, action, sample_path, args.target_pid
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(30),
    })
    .await?;
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: None,
        sample_format: SampleFormat::Jfr,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

async fn perf_one_shot(args: &Args, sample_path: &str) -> Result<Output> {
    let duration = args
        .duration_secs
        .ok_or_else(|| anyhow!("OneShot requires duration_secs"))?;
    let raw = format!("{sample_path}.data");
    let cmd = format!(
        "{0} record -F 99 -p {1} -g -o {2} -- sleep {3} && {0} script -i {2} | stackcollapse-perf > {4}",
        args.binary_path, args.target_pid, raw, duration, sample_path
    );
    let out = exec_in_container::run(exec_in_container::Args {
        container_id: args.container_id.clone(),
        cmd: vec!["sh".into(), "-c".into(), cmd],
        user: Some("root".into()),
        workdir: None,
        env: vec![],
        detach: false,
        timeout_secs: Some(duration as u64 + 60),
    })
    .await?;
    Ok(Output {
        sample_path_in_container: sample_path.to_string(),
        samples_collected: None,
        sample_format: SampleFormat::Folded,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

fn parse_pyspy_samples(stderr: &str) -> Option<u64> {
    // py-spy prints e.g. "Process 42 ended; samples: 3047" when finishing.
    for line in stderr.lines().rev() {
        if let Some(rest) = line.split("samples:").nth(1) {
            if let Ok(n) = rest.trim().parse::<u64>() {
                return Some(n);
            }
        }
    }
    None
}

#[allow(dead_code)]
pub fn default_sample_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

#[allow(dead_code)]
pub fn capture_window(d: u32) -> Duration {
    Duration::from_secs(d as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_per_profiler() {
        assert!(matches!(default_format(Profiler::PySpy), SampleFormat::Speedscope));
        assert!(matches!(default_format(Profiler::AsyncProfiler), SampleFormat::Jfr));
        assert!(matches!(default_format(Profiler::Perf), SampleFormat::Folded));
        assert!(matches!(default_format(Profiler::NodeClinic), SampleFormat::Raw));
    }

    #[test]
    fn sample_path_uses_profiler_prefix_and_extension() {
        let p = sample_path_for(Profiler::PySpy, 1234, SampleFormat::Speedscope);
        assert_eq!(p, "/tmp/pyspy-1234.json");

        let p = sample_path_for(Profiler::AsyncProfiler, 99, SampleFormat::Jfr);
        assert_eq!(p, "/tmp/asprof-99.jfr");

        let p = sample_path_for(Profiler::Perf, 7, SampleFormat::Folded);
        assert_eq!(p, "/tmp/perf-7.folded");

        let p = sample_path_for(Profiler::Rbspy, 1, SampleFormat::Raw);
        assert_eq!(p, "/tmp/profile-1.raw");
    }

    #[test]
    fn parse_pyspy_samples_picks_up_count() {
        let stderr = "Process 42 ended successfully\nWriting to file\nsamples: 3047\n";
        assert_eq!(parse_pyspy_samples(stderr), Some(3047));
    }

    #[test]
    fn parse_pyspy_samples_returns_none_when_absent() {
        assert!(parse_pyspy_samples("nothing useful here").is_none());
    }

    #[test]
    fn parse_pyspy_samples_takes_last_match() {
        let stderr = "samples: 1\nsamples: 9999\n";
        assert_eq!(parse_pyspy_samples(stderr), Some(9999));
    }

    #[tokio::test]
    async fn run_one_shot_requires_duration() {
        let err = run(Args {
            container_id: "c".into(),
            profiler: Profiler::PySpy,
            binary_path: "py-spy".into(),
            target_pid: 1,
            mode: Mode::OneShot,
            duration_secs: None,
            sample_format: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("OneShot requires duration_secs"));
    }

    #[tokio::test]
    async fn run_unsupported_profiler_returns_clear_error() {
        let err = run(Args {
            container_id: "c".into(),
            profiler: Profiler::Rbspy,
            binary_path: "rbspy".into(),
            target_pid: 1,
            mode: Mode::OneShot,
            duration_secs: Some(5),
            sample_format: None,
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not yet implemented"));
    }
}
