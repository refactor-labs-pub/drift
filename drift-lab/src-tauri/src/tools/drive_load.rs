//! Stage 3a — generate synthetic load against the target service.
//!
//! Two modes:
//!   - `Builtin`: in-process reqwest worker pool with token-bucket pacing.
//!     Zero-dependency, good for HTTP up to a few thousand RPS.
//!   - `Vegeta`: spawn the `vegeta` CLI as a sidecar — preferred for higher
//!     RPS, distribution-accurate latency reports, and replay traffic files.
//!
//! Output is the same shape regardless of backend so the LLM can reason about
//! both uniformly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::ToolManifest;

pub const NAME: &str = "drive_load";
pub const DESCRIPTION: &str =
    "Send synthetic HTTP load to a URL for a fixed duration and return latency percentiles. Use \
     before/while running the profiler so the workload is non-idle.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "target_url": { "type": "string" },
    "rps": { "type": "integer", "description": "Target requests per second." },
    "duration_secs": { "type": "integer" },
    "method": { "type": "string", "description": "HTTP method (default GET)." },
    "headers": {
      "type": "object",
      "additionalProperties": { "type": "string" }
    },
    "body": { "type": "string" },
    "concurrency": {
      "type": "integer",
      "description": "Max concurrent in-flight requests. Default: rps / 5."
    }
  },
  "required": ["target_url", "rps", "duration_secs"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub target_url: String,
    pub rps: u32,
    pub duration_secs: u32,
    pub method: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub concurrency: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub requests: u64,
    pub success: u64,
    pub errors: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub status_codes: HashMap<u16, u64>,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let method_str = args.method.clone().unwrap_or_else(|| "GET".into());
    let method = reqwest::Method::from_bytes(method_str.as_bytes())
        .context("invalid HTTP method")?;

    let client = reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15))
        .build()
        .context("build reqwest client")?;

    let total_secs = args.duration_secs.max(1) as u64;
    let total_requests = (args.rps as u64) * total_secs;
    let interval = Duration::from_nanos(1_000_000_000 / args.rps.max(1) as u64);
    let concurrency = args
        .concurrency
        .unwrap_or((args.rps / 5).max(8)) as usize;

    let stats = Arc::new(Mutex::new(Stats::default()));
    let stop_at = Instant::now() + Duration::from_secs(total_secs);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));

    let mut next_send = Instant::now();
    let mut sent: u64 = 0;
    let mut inflight = FuturesUnordered::new();

    while sent < total_requests && Instant::now() < stop_at {
        let now = Instant::now();
        if now < next_send {
            tokio::time::sleep(next_send - now).await;
        }
        next_send += interval;
        sent += 1;

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let url = args.target_url.clone();
        let method = method.clone();
        let headers = args.headers.clone();
        let body = args.body.clone();
        let stats = Arc::clone(&stats);

        inflight.push(tokio::spawn(async move {
            let _permit = permit;
            let mut req = client.request(method, &url);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            if let Some(b) = body {
                req = req.body(b);
            }
            let started = Instant::now();
            let result = req.send().await;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            let mut s = stats.lock().await;
            match result {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    *s.status_codes.entry(code).or_insert(0) += 1;
                    if resp.status().is_success() {
                        s.success += 1;
                    } else {
                        s.errors += 1;
                    }
                    s.latencies.push(elapsed_ms);
                }
                Err(_) => {
                    s.errors += 1;
                }
            }
            s.requests += 1;
        }));

        // Drain finished tasks opportunistically so we don't accumulate.
        while inflight.len() > concurrency * 2 {
            let _ = inflight.next().await;
        }
    }

    while inflight.next().await.is_some() {}

    let mut s = stats.lock().await;
    let summary = summarise(&mut s.latencies);

    Ok(Output {
        requests: s.requests,
        success: s.success,
        errors: s.errors,
        mean_ms: summary.mean,
        p50_ms: summary.p50,
        p95_ms: summary.p95,
        p99_ms: summary.p99,
        max_ms: summary.max,
        status_codes: std::mem::take(&mut s.status_codes),
    })
}

#[derive(Default)]
struct Stats {
    requests: u64,
    success: u64,
    errors: u64,
    latencies: Vec<f64>,
    status_codes: HashMap<u16, u64>,
}

/// Pure helper — sorts `latencies` in place and returns mean / p50 / p95 / p99 / max.
/// Returns zeros for empty input. Exposed for unit testing.
pub(crate) struct Summary {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

pub(crate) fn summarise(latencies: &mut Vec<f64>) -> Summary {
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if latencies.is_empty() {
        return Summary { mean: 0.0, p50: 0.0, p95: 0.0, p99: 0.0, max: 0.0 };
    }
    let pick = |p: f64| -> f64 {
        let idx = ((latencies.len() as f64 - 1.0) * p).round() as usize;
        latencies[idx]
    };
    let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
    Summary {
        mean,
        p50: pick(0.50),
        p95: pick(0.95),
        p99: pick(0.99),
        max: *latencies.last().unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn summarise_empty_returns_zeros() {
        let mut v: Vec<f64> = vec![];
        let s = summarise(&mut v);
        assert_eq!(s.mean, 0.0);
        assert_eq!(s.p99, 0.0);
        assert_eq!(s.max, 0.0);
    }

    #[test]
    fn summarise_single_value() {
        let mut v = vec![42.0];
        let s = summarise(&mut v);
        assert_eq!(s.mean, 42.0);
        assert_eq!(s.p50, 42.0);
        assert_eq!(s.p99, 42.0);
        assert_eq!(s.max, 42.0);
    }

    #[test]
    fn summarise_percentiles_on_1_to_100() {
        let mut v: Vec<f64> = (1..=100).map(|n| n as f64).collect();
        let s = summarise(&mut v);
        // mean of 1..=100 is 50.5
        assert!((s.mean - 50.5).abs() < 0.001);
        // p50 → idx ((100-1)*0.5).round() = 50 → value 51
        assert_eq!(s.p50, 51.0);
        // p95 → idx 94 → 95
        assert_eq!(s.p95, 95.0);
        // p99 → idx 98 → 99
        assert_eq!(s.p99, 99.0);
        assert_eq!(s.max, 100.0);
    }

    #[test]
    fn summarise_unsorted_input_is_sorted() {
        let mut v = vec![10.0, 1.0, 5.0, 3.0, 7.0];
        let s = summarise(&mut v);
        assert_eq!(v, vec![1.0, 3.0, 5.0, 7.0, 10.0]);
        assert_eq!(s.max, 10.0);
    }

    /// Tiny TCP server that replies "200 OK" to any request and stops itself
    /// after `expected` accepts. Returns its address.
    async fn spawn_mock_http(expected: usize) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..expected {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    ).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn run_drives_load_against_mock_server() {
        let addr = spawn_mock_http(100).await;
        let url = format!("http://{}/", addr);

        let out = run(Args {
            target_url: url,
            rps: 20,
            duration_secs: 1,
            method: None,
            headers: Default::default(),
            body: None,
            concurrency: Some(8),
        })
        .await
        .unwrap();

        assert!(out.requests > 0, "expected at least one request, got {}", out.requests);
        assert!(out.success > 0, "expected at least one success");
        // p99 must be sane (<10s) — the server is local and instantaneous.
        assert!(out.p99_ms < 10_000.0);
        // Status code map should record 200s.
        assert!(out.status_codes.get(&200).copied().unwrap_or(0) > 0);
    }
}
