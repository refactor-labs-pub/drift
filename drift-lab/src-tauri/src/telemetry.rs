//! Live container telemetry — polls `docker stats` for one container at ~2 Hz
//! and pushes parsed [`TelemetrySample`]s onto a channel. The workflow drains
//! the channel and forwards samples to the UI via [`crate::events::topic::TELEMETRY`].
//!
//! Why shell out to `docker stats` instead of bollard's `/containers/.../stats`
//! endpoint? The CLI does the same cgroup math we'd otherwise have to recreate
//! (CPU% across NCPUs, mem percentages), gives us pre-formatted "1.5MB / 2GiB"
//! pairs, and is one less moving piece. The cost (a ~10ms `fork+exec` per
//! sample) is well below the sample interval.
//!
//! The sampler exits as soon as either: the `cancel` token fires, the channel
//! receiver is dropped (workflow shut down), or `docker stats` repeatedly
//! errors. A single failed snapshot is logged and skipped — most are due to
//! the container restarting mid-sample.

use std::time::Duration;

use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::TelemetrySample;

/// Poll cadence. Matches the UI's 60-sample / ~30s rolling window in
/// `TelemetryPane`; tighter means more docker-cli forks for little extra
/// signal, slower drops the resolution of short CPU spikes.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Spawn the background sampler. Returns immediately; the task runs until
/// `cancel` fires or `tx` is closed. Errors are logged, not surfaced — the
/// telemetry pane is best-effort decoration, not a hard dependency of the
/// scan.
pub fn spawn_sampler(
    run_id: String,
    container_id: String,
    tx: mpsc::UnboundedSender<TelemetrySample>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        tracing::info!(
            target: "drift::telemetry",
            run_id = %run_id,
            container_id = %container_id,
            "telemetry sampler started"
        );
        let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
        // First tick fires immediately — give docker a moment so the very
        // first `docker stats` call doesn't race with container startup.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let output = Command::new("docker")
                .args([
                    "stats",
                    "--no-stream",
                    "--format",
                    "{{json .}}",
                    &container_id,
                ])
                .kill_on_drop(true)
                .output()
                .await;
            let raw = match output {
                Ok(o) if o.status.success() => o.stdout,
                Ok(o) => {
                    tracing::debug!(
                        target: "drift::telemetry",
                        run_id = %run_id,
                        container_id = %container_id,
                        exit = ?o.status.code(),
                        stderr = %String::from_utf8_lossy(&o.stderr),
                        "docker stats non-zero exit (likely container restart)"
                    );
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "drift::telemetry",
                        run_id = %run_id,
                        error = %e,
                        "docker stats spawn failed"
                    );
                    continue;
                }
            };
            let text = String::from_utf8_lossy(&raw);
            for line in text.lines() {
                let Some(sample) = parse_stats_line(&run_id, &container_id, line) else {
                    continue;
                };
                if tx.send(sample).is_err() {
                    tracing::info!(
                        target: "drift::telemetry",
                        run_id = %run_id,
                        "receiver dropped — sampler exiting"
                    );
                    return;
                }
            }
        }
        tracing::info!(
            target: "drift::telemetry",
            run_id = %run_id,
            "telemetry sampler stopped"
        );
    });
}

fn parse_stats_line(run_id: &str, container_id: &str, line: &str) -> Option<TelemetrySample> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let cpu_pct = v.get("CPUPerc").and_then(|s| s.as_str()).and_then(parse_pct).unwrap_or(0.0);
    let mem_pct = v.get("MemPerc").and_then(|s| s.as_str()).and_then(parse_pct).unwrap_or(0.0);
    let (mem_mb, _mem_limit_mb) = v
        .get("MemUsage")
        .and_then(|s| s.as_str())
        .map(parse_size_pair_mb)
        .unwrap_or((0.0, 0.0));
    let (net_rx, net_tx) = v
        .get("NetIO")
        .and_then(|s| s.as_str())
        .map(parse_byte_pair)
        .unwrap_or((0, 0));
    let (blk_r, blk_w) = v
        .get("BlockIO")
        .and_then(|s| s.as_str())
        .map(parse_byte_pair)
        .unwrap_or((0, 0));
    Some(TelemetrySample {
        run_id: run_id.to_string(),
        ts_ms: chrono::Utc::now().timestamp_millis(),
        container_id: container_id.to_string(),
        cpu_pct,
        mem_mb,
        mem_pct,
        net_rx_bytes: net_rx,
        net_tx_bytes: net_tx,
        block_read_bytes: blk_r,
        block_write_bytes: blk_w,
    })
}

fn parse_pct(s: &str) -> Option<f32> {
    s.trim_end_matches('%').trim().parse::<f32>().ok()
}

/// Parse "150MiB / 2GiB" → (used_mb, total_mb). Returns (0, 0) on parse error.
fn parse_size_pair_mb(s: &str) -> (f32, f32) {
    let mut parts = s.split('/');
    let used = parts.next().map(parse_size_to_mb).unwrap_or(0.0);
    let total = parts.next().map(parse_size_to_mb).unwrap_or(0.0);
    (used, total)
}

fn parse_size_to_mb(raw: &str) -> f32 {
    (parse_size_to_bytes(raw) as f32) / (1024.0 * 1024.0)
}

/// Parse "1.5MB / 700kB" → (rx_bytes, tx_bytes) absolute (cumulative).
fn parse_byte_pair(s: &str) -> (u64, u64) {
    let mut parts = s.split('/');
    let a = parts.next().map(parse_size_to_bytes).unwrap_or(0);
    let b = parts.next().map(parse_size_to_bytes).unwrap_or(0);
    (a, b)
}

/// Parse "1.5MB", "81.4MiB", "0B" → bytes. Docker (via `units.HumanSize`)
/// uses SI prefixes (`kB`, `MB`, `GB` = 1000-base) and IEC prefixes (`KiB`,
/// `MiB`, `GiB` = 1024-base) interchangeably, so we have to distinguish.
fn parse_size_to_bytes(raw: &str) -> u64 {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let (num, unit) = raw.split_at(split);
    let n: f64 = num.parse().unwrap_or(0.0);
    let unit = unit.trim();
    // Case-sensitive: `kB`/`KB` = 1000-base, `KiB` = 1024-base. Lowercase the
    // comparison key to fold KB/kB but keep the `i` distinction intact.
    let key = unit.to_ascii_lowercase();
    let factor: f64 = match key.as_str() {
        "b" | "" => 1.0,
        "kb" => 1_000.0,
        "kib" => 1_024.0,
        "mb" => 1_000_000.0,
        "mib" => 1_024.0 * 1_024.0,
        "gb" => 1_000_000_000.0,
        "gib" => 1_024.0_f64.powi(3),
        "tb" => 1_000_000_000_000.0,
        "tib" => 1_024.0_f64.powi(4),
        _ => 1.0,
    };
    (n * factor) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_docker_stats_line() {
        // Shape of `docker stats --no-stream --format '{{json .}}'`.
        let line = r#"{"BlockIO":"1.2MB / 0B","CPUPerc":"3.42%","Container":"a1b2","ID":"a1b2","MemPerc":"4.05%","MemUsage":"81.4MiB / 1.94GiB","Name":"svc","NetIO":"2.1kB / 1.8kB","PIDs":"7"}"#;
        let s = parse_stats_line("r", "a1b2", line).expect("parses");
        assert!((s.cpu_pct - 3.42).abs() < 0.001);
        assert!((s.mem_pct - 4.05).abs() < 0.001);
        assert!((s.mem_mb - 81.4).abs() < 0.1);
        assert_eq!(s.block_read_bytes, 1_200_000);
        assert_eq!(s.block_write_bytes, 0);
        assert_eq!(s.net_rx_bytes, 2_100);
        assert_eq!(s.net_tx_bytes, 1_800);
    }

    #[test]
    fn parse_size_handles_si_vs_iec_units() {
        // Docker uses SI (1000-base) for kB/MB/GB and IEC (1024-base) for
        // KiB/MiB/GiB — the parser must distinguish, or memory MB will be
        // 4.9% off and net I/O wildly inconsistent across units.
        assert_eq!(parse_size_to_bytes("0B"), 0);
        assert_eq!(parse_size_to_bytes("512kB"), 512_000);
        assert_eq!(parse_size_to_bytes("512KiB"), 512 * 1024);
        assert_eq!(parse_size_to_bytes("2MB"), 2_000_000);
        assert_eq!(parse_size_to_bytes("2MiB"), 2 * 1024 * 1024);
        assert_eq!(parse_size_to_bytes("1.5GB"), 1_500_000_000);
    }

    #[test]
    fn ignores_blank_or_malformed_lines() {
        assert!(parse_stats_line("r", "c", "").is_none());
        assert!(parse_stats_line("r", "c", "not json").is_none());
    }
}
