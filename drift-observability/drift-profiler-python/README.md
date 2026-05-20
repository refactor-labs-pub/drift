# drift-docker-profiler

[![PyPI version](https://img.shields.io/pypi/v/drift-docker-profiler.svg)](https://pypi.org/project/drift-docker-profiler/)
[![Python versions](https://img.shields.io/pypi/pyversions/drift-docker-profiler.svg)](https://pypi.org/project/drift-docker-profiler/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

> Distribution name on PyPI: **`drift-docker-profiler`**.
> Python import name: **`driftdockerprofiler`** (Python modules can't
> contain dashes). You `pip install drift-docker-profiler`, then
> `import driftdockerprofiler`.

Wall + CPU stack-sampling profiler for Python. Zero runtime
dependencies (base install). Writes append-only JSONL to disk, or
streams events live over Supabase Realtime.

A surgical fork of [`google-cloud-profiler`](https://github.com/GoogleCloudPlatform/cloud-profiler-python)
with the GCP transport stripped out — no `google-api-python-client`,
no `google-auth`, no `protobuf`, no upload to Stackdriver.

> **By [Refactor Labs](https://refactorlab.com).** Refactor Labs is
> the team behind **FinPos** and the broader Drift observability
> stack — production-grade tooling that captures the running shape of
> your code *before* it breaks in front of customers. This profiler
> is the Python instrumentation that powers it.

---

## Table of contents

- [Install](#install)
- [Quickstart](#quickstart)
- [Configuration reference](#configuration-reference)
- [Environment variables](#environment-variables)
- [Sinks (where events go)](#sinks-where-events-go)
- [Sampling strategies](#sampling-strategies)
- [Exclude paths](#exclude-paths)
- [Event format](#event-format)
- [Supported platforms](#supported-platforms)
- [Running on Alpine / multi-stage Docker](#running-on-alpine--multi-stage-docker)
- [Troubleshooting](#troubleshooting)
- [Publishing to PyPI (maintainers)](#publishing-to-pypi-maintainers)
- [License](#license)
- [About Refactor Labs](#about-refactor-labs)

---

## Install

```bash
# base — zero runtime deps on Python 3.8+
pip install drift-docker-profiler

# with the optional Supabase Realtime sink (websocket-client + certifi)
pip install 'drift-docker-profiler[supabase]'
```

## Quickstart

```python
import driftdockerprofiler

driftdockerprofiler.start(
    service='my-service',
    service_version='1.0.0',
    verbose=2,                              # 0=error, 1=warn, 2=info, 3=debug
    # output_path='/tmp/drift/events.jsonl', # default
)

# ... run your app normally ...
# Events are appended to /tmp/drift/events.jsonl as the sampler ticks.
```

That's it. The agent starts a background daemon thread per profile
type (WALL, CPU), samples for `duration_ms` (default 10 s), and
flushes one JSONL line per unique stack to disk on each window close.
`stop()` (or process exit) flushes the writer cleanly.

---

## Configuration reference

Every kwarg of `driftdockerprofiler.start()`:

| Argument                  | Type                              | Default                       | Meaning                                                                                                                                                                                                                                              |
| ------------------------- | --------------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `service`                 | `str`                             | `$GAE_SERVICE` / `$K_SERVICE` | **Required.** Logical name for the service being profiled. Must match `^[a-z0-9]([-a-z0-9_.]{0,253}[a-z0-9])?$`. Stamped on every event.                                                                                                             |
| `service_version`         | `str` \| `None`                   | `$GAE_VERSION` / `$K_REVISION`| Optional version label. Useful for diffing profiles across releases.                                                                                                                                                                                 |
| `output_path`             | `str` \| `None`                   | `$DRIFT_OUTPUT_PATH` / `/tmp/drift/events.jsonl` | JSONL file the agent appends to. Resolution: kwarg → `DRIFT_OUTPUT_PATH` env var → `/tmp/drift/events.jsonl`. Parent directory is created if needed. Ignored when an explicit `sink` is passed or when Supabase auto-wiring kicks in.        |
| `period_ms`               | `int`                             | `10`                          | Sampling interval in milliseconds.                                                                                                                                                                                                                   |
| `duration_ms`             | `int`                             | `10_000`                      | Profile window length in milliseconds. One round of "sample for N ms, then emit" per type.                                                                                                                                                          |
| `disable_cpu_profiling`   | `bool`                            | `False`                       | Skip CPU profiling (native SIGPROF sampler). Ignored on platforms where the C++ extension isn't built (macOS, Windows).                                                                                                                              |
| `disable_wall_profiling`  | `bool`                            | `False`                       | Skip wall-clock profiling. At least one of WALL or CPU must remain enabled.                                                                                                                                                                          |
| `emit_mode`               | `'per_trace'` \| `'bundle'`       | `'per_trace'`                 | `per_trace` — one JSONL line per unique stack per window. `bundle` — one JSONL line per window carrying the whole pprof-shaped `Profile`.                                                                                                            |
| `wall_strategy`           | `'all_threads'` \| `'signal'`     | `'all_threads'`               | `all_threads` — daemon thread + `sys._current_frames()` (covers every thread, no SIGALRM dependency). `signal` — legacy SIGALRM + ITIMER_REAL (main thread only, kept for back-compat).                                                              |
| `pod`                     | `str` \| `None`                   | `$HOSTNAME` / `socket.gethostname()` | Per-host label stamped on every event. Useful when multiple replicas share a log destination.                                                                                                                                                  |
| `builtin_exclude_paths`   | `tuple[str, ...]`                 | `BUILTIN_EXCLUDE_PATHS`       | Substring filter applied to each sample's leaf-frame `file`. Defaults drop the profiler observing itself + frozen importlib bootstrap. Pass `()` to fully disable.                                                                                  |
| `exclude_paths`           | `tuple[str, ...]`                 | `()`                          | Additional user-supplied substrings stacked on top of the builtins. Common: `('/sqlalchemy/', '/celery/')`. Combine with `driftdockerprofiler.STRICT_USER_CODE_EXCLUDE_PATHS` to drop stdlib + site-packages too.                                    |
| `sink`                    | `Sink` \| `None`                  | `None`                        | Pre-built sink (anything with `emit(event)` + `close()`). When given, wins outright — the `supabase_*` kwargs / env vars below are ignored. When `None`, the auto-wiring path is used: see [Auto-wiring](#auto-wiring).                              |
| `supabase_url`            | `str` \| `None`                   | `$SUPABASE_URL`               | Supabase project URL (e.g. `https://abc123.supabase.co`). Resolution: kwarg → `SUPABASE_URL` env var. When this + `supabase_api_key` both resolve and no explicit `sink` was passed, a `SupabaseRealtimeSink` is built instead of a file sink.       |
| `supabase_api_key`        | `str` \| `None`                   | `$SUPABASE_REALTIME_API_KEY`  | Supabase API key — publishable (`sb_publishable_...`) or service_role JWT. Resolution: kwarg → `SUPABASE_REALTIME_API_KEY` env var. Required (with `supabase_url`) to enable broadcast mode.                                                          |
| `supabase_channel`        | `str` \| `None`                   | `$SUPABASE_REALTIME_CHANNEL` / `drift-profiler-events` | Channel name joined as `realtime:<channel>`. Resolution: kwarg → `SUPABASE_REALTIME_CHANNEL` env var → `drift-profiler-events`.                                                                          |
| `verbose`                 | `int`                             | `0`                           | Log level. 0=error, 1=warning, 2=info, 3=debug.                                                                                                                                                                                                       |

```python
driftdockerprofiler.stop()    # idempotent; also called via atexit
```

---

## Environment variables

All env vars the agent consults, in one place:

| Variable                       | Used for                                                                                                  |
| ------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `GAE_SERVICE`                  | Fallback for `service=` (Google App Engine convention; works for any deployment that sets it).            |
| `K_SERVICE`                    | Fallback for `service=` (Knative / Cloud Run convention).                                                 |
| `GAE_VERSION`                  | Fallback for `service_version=`.                                                                          |
| `K_REVISION`                   | Fallback for `service_version=`.                                                                          |
| `HOSTNAME`                     | Fallback for `pod=`. Set by Docker / Kubernetes by default.                                              |
| `DRIFT_OUTPUT_PATH`            | Fallback for `output_path=`. When set, redirects the default `JsonlFileSink` away from `/tmp/drift/events.jsonl`. Overridden by the `output_path` kwarg. Ignored when Supabase auto-wires. |
| `SUPABASE_URL`                 | Fallback for `supabase_url=`. If both this and `SUPABASE_REALTIME_API_KEY` resolve (and no explicit `sink=` was passed), auto-wires the WSS sink instead of writing to file. |
| `SUPABASE_REALTIME_API_KEY`    | Fallback for `supabase_api_key=`. Supabase publishable key (`sb_publishable_...`) or anon JWT. Used for socket auth + channel access token.|
| `SUPABASE_REALTIME_CHANNEL`    | Fallback for `supabase_channel=`. Channel name for the Supabase sink. Defaults to `drift-profiler-events`.                                  |

A copy of these for local dev lives in
[`drift-observability/.env.example`](../.env.example).

---

## Sinks (where events go)

Events are produced by samplers and consumed by **sinks**. The
`Client` doesn't know which sink it holds — it just calls
`sink.emit(event)` / `sink.close()`. Adding a new destination is one
class implementing the `Sink` protocol; no other file changes.

| Sink                    | What it does                                                                  | Extra deps                  |
| ----------------------- | ----------------------------------------------------------------------------- | --------------------------- |
| `JsonlFileSink`         | Append-only, line-buffered JSONL file. The legacy `JsonlWriter` under the hood. | None                       |
| `SupabaseRealtimeSink`  | Joins one Phoenix Channel, broadcasts every event over WSS.                   | `websocket-client`, `certifi` |
| `TeeSink`               | Fan-out: write to several sinks in lock-step.                                 | None                        |
| `Sink` (protocol)       | Implement `emit(event: dict) -> None` and `close() -> None` for your own.     | —                           |

### Auto-wiring

If you don't pass `sink=` explicitly, the agent picks:

1. **`SupabaseRealtimeSink`** — if both `supabase_url` *and*
   `supabase_api_key` resolve. Each resolves in this order:
   explicit kwarg → environment variable
   (`SUPABASE_URL` / `SUPABASE_REALTIME_API_KEY`).
   `supabase_channel` adds a third fallback to
   `drift-profiler-events`.
2. **`JsonlFileSink(output_path)`** — otherwise (back-compat
   default).

An explicit `sink=` kwarg always wins outright — when present, every
`supabase_*` kwarg and the matching env vars are ignored.

### Supabase via kwargs (no env vars)

```python
driftdockerprofiler.start(
    service='my-service',
    supabase_url='https://abc123.supabase.co',
    supabase_api_key='sb_publishable_...',
    supabase_channel='prod-events',          # optional
)
```

Equivalent to setting `SUPABASE_URL` / `SUPABASE_REALTIME_API_KEY` /
`SUPABASE_REALTIME_CHANNEL` in the environment.

### Explicit Tee example

```python
from driftdockerprofiler import JsonlFileSink, SupabaseRealtimeSink, TeeSink

sink = TeeSink([
    JsonlFileSink('/var/log/drift/events.jsonl'),
    SupabaseRealtimeSink(
        supabase_url='https://abc123.supabase.co',
        api_key='sb_publishable_...',
        channel='prod-events',
    ),
])

driftdockerprofiler.start(service='my-service', sink=sink)
```

---

## Sampling strategies

### Wall-clock sampler — `wall_strategy=`

- **`'all_threads'`** *(default)* — daemon thread iterates
  `sys._current_frames()` every `period_ms`. Sees **every** Python
  thread: uvicorn / gunicorn threadpool workers, Django WSGI workers,
  `loop.run_in_executor`, your own `threading.Thread()` instances.
  No signals, safe to start from any thread. This is the
  dd-trace-py / Sentry / Pyroscope shape.
- **`'signal'`** — legacy SIGALRM + ITIMER_REAL. **Main thread only.**
  Misses every framework that off-loads request work to a worker
  thread, which is most of them in production. Kept for parity with
  the upstream Google agent.

### CPU sampler

`ITIMER_PROF`-driven, written in C++ as the `_profiler` extension.
Linux only — SIGPROF semantics on macOS are too limited to be
reliable. Disabled automatically on Darwin / Windows.

### Emit mode — `emit_mode=`

- **`'per_trace'`** *(default)* — one JSONL line per unique stack
  per window. N events per window where N is the number of distinct
  stacks observed. Best for downstream "top hot frames" aggregation.
- **`'bundle'`** — one JSONL line per window carrying the whole
  pprof-shaped `Profile` (all samples inlined under `samples[]`).
  Best for one-event-per-window replay or pprof-compatible tools.

### What stack sampling can't see

Stack samplers — this one, `py-spy`, `gperftools`, Google Cloud
Profiler, `pyinstrument` — share one fundamental blind spot: a
function whose execution time is much smaller than the sample
period is statistically invisible as a leaf. The hit-rate math is:

```
P(catch one call) ≈ call_duration / sample_period
```

With `period_ms=10` (default) and a ~1 µs handler like
`@app.get("/healthz")`, the probability per call is ~0.0001 — you'd
need thousands of requests to expect one sample to land there. The
sampler isn't broken; this is the cost of statistical sampling.

What still works without changes:
- Anything that blocks longer than the sample period (DB queries,
  HTTP calls, `time.sleep`, compute loops) shows up cleanly.
- Sub-period framework calls inside long-running stacks appear
  deeper in the captured stack — you just won't see them as leaves
  on their own.

What you can do when you specifically need per-call visibility
on a fast method (one optional escape hatch, no auto-config):

- Decorate the function with `@driftdockerprofiler.trace` — emits
  one `function_call` event per call regardless of duration
  (~5 µs overhead per decorated call). Deterministic; not a sampler.
- Drop `period_ms` to 1 (10× more samples; still won't see
  microsecond functions but catches sub-millisecond ones).

---

## Exclude paths

Two-layer substring filter applied to each sample's leaf-frame `file`:

```
final_filters = builtin_exclude_paths + exclude_paths
```

A leaf matches if its file path contains **any** substring in the
combined list. Matched samples are dropped before they're written.

```python
import driftdockerprofiler

driftdockerprofiler.start(
    service='my-svc',
    # Drop sqlalchemy + celery noise but keep the rest of the
    # stdlib / site-packages visible. Builtins (profiler self + frozen
    # importlib) are kept implicitly.
    exclude_paths=('/sqlalchemy/', '/celery/'),
)
```

For "only my code" — the most aggressive preset — combine with the
strict preset:

```python
driftdockerprofiler.start(
    service='my-svc',
    exclude_paths=driftdockerprofiler.STRICT_USER_CODE_EXCLUDE_PATHS,
    # → also drops /lib/python3.* and /site-packages/
)
```

> ⚠ Dropping stdlib + site-packages can turn the icicle chart empty
> if your hot methods aren't `@trace`-decorated and the sampler's
> leaf frames all land in framework code (asyncio, uvicorn). Use the
> deterministic `@trace` decorator on the methods you care about
> when you go strict.

---

## Event format

Every line of the output file is one JSON object. Six shapes,
distinguished by top-level `type`:

| `type`             | Emitted by                                            | One per…                  |
| ------------------ | ----------------------------------------------------- | ------------------------- |
| `profile_metadata` | `Client.start()` (once per session, first line)       | session                   |
| `wall_trace`       | Wall sampler in `emit_mode='per_trace'`               | unique stack per window   |
| `cpu_trace`        | CPU sampler in `emit_mode='per_trace'`                | unique stack per window   |
| `wall_profile`     | Wall sampler in `emit_mode='bundle'`                  | window                    |
| `cpu_profile`      | CPU sampler in `emit_mode='bundle'`                   | window                    |
| `function_call`    | `@driftdockerprofiler.trace`-decorated function       | call                      |

Full schema, common fields, per-mode extras, and reader recipes
(`jq`, `tail -F`, SSE relay) live in
[`EVENT_FILE_FORMAT.md`](EVENT_FILE_FORMAT.md).

JSON Schema (Draft 2020-12) and OpenAPI 3.1 definitions ship inside
the wheel:

```python
import driftdockerprofiler, jsonschema, json

with open('/tmp/drift/events.jsonl') as f:
    for line in f:
        event = json.loads(line)
        driftdockerprofiler.validate_event(event)   # raises on drift
```

---

## Supported platforms

| OS                       | CPU sampler | Wall sampler | Notes                                                                  |
| ------------------------ | ----------- | ------------ | ---------------------------------------------------------------------- |
| **Linux x86_64 / aarch64** | ✅          | ✅           | Full support. `glibc` and `musl` (Alpine) both work.                  |
| **macOS** (Intel + Apple Silicon) | ❌  | ✅           | Wall sampler only — SIGPROF on Darwin is too limited.                |
| **Windows**              | ❌          | ❌           | `start()` raises `NotImplementedError`. PRs welcome.                  |

Python: **3.7 – 3.14**.

- 3.7 – 3.12 on Linux → platform wheel with the C++ SIGPROF CPU
  sampler baked in.
- 3.13 + 3.14 → universal `py3-none-any` wheel, wall sampler only.
  CPython 3.13 removed `PyThreadState->cframe` and renamed
  `_PyInterpreterFrame->f_code`, both of which the signal-safe stack
  walker dereferences directly; 3.14 took `_PyInterpreterFrame`
  opaque. `cpu_profiling_available()` returns `False` and callers
  degrade gracefully.

---

## Running on Alpine / multi-stage Docker

The native CPU profiler extension needs `build-base` (gcc, g++,
make) at build time. The base Alpine Python image doesn't ship those.
Easiest path: a two-stage build that compiles the wheel in stage 1
and copies the prebuilt artifact into a clean runtime image.

```dockerfile
FROM python:3.12-alpine AS builder

RUN apk add --update --no-cache build-base

# Pre-build the wheel (and any transitive C deps).
RUN pip3 wheel --wheel-dir=/tmp/wheels drift-docker-profiler


FROM python:3.12-alpine

COPY --from=builder /tmp/wheels /tmp/wheels
RUN pip3 install --no-index --find-links=/tmp/wheels drift-docker-profiler

COPY ./app /app
CMD ["python3", "-u", "/app/main.py"]
```

For Debian / Ubuntu-based images, the prebuilt `manylinux` wheel on
PyPI installs without any compiler.

---

## Troubleshooting

### `BlockingIOError: [Errno 11] Resource temporarily unavailable`

Symptom appears with the legacy `wall_strategy='signal'` (SIGALRM)
path under high signal-delivery rates. Fix: switch to the default
`wall_strategy='all_threads'` (no signals, no problem). If you must
keep SIGALRM, see the upstream
[Google Cloud Profiler note](https://cloud.google.com/profiler/docs/troubleshooting#python-blocking)
on the `signal.set_wakeup_fd` workaround.

### I don't see my fast endpoint (`/healthz` or similar) in events.log

By design — see [What stack sampling can't see](#what-stack-sampling-cant-see)
above. A handler that returns in ~1 µs is statistically invisible at
the default 10 ms sample period (`P ≈ 0.0001` per call). Use
`@driftdockerprofiler.trace` on the function if you specifically need
per-call timing for that endpoint; otherwise this is the expected
behaviour every stack sampler shares.

### `Client.start() must be called from the main thread when wall_strategy='signal'`

The legacy SIGALRM path needs to install its handler on the main
thread. Either start from the main thread, or switch to the default
all-threads strategy:

```python
driftdockerprofiler.start(service='svc', wall_strategy='all_threads')
```

### Supabase sink: `CERTIFICATE_VERIFY_FAILED` on macOS

System Python on macOS often ships without a usable CA bundle.
Install `certifi` (already pulled in by the `[supabase]` extra):

```bash
pip install 'drift-docker-profiler[supabase]'
```

The sink picks it up automatically.

---

## Publishing to PyPI (maintainers)

This package is published to **[https://pypi.org/project/drift-docker-profiler/](https://pypi.org/project/drift-docker-profiler/)**.

The release pipeline is fully automated via GitHub Actions
([`.github/workflows/drift-profiler-python-release.yml`](../../.github/workflows/drift-profiler-python-release.yml))
and follows this contract:

> **Any change inside `drift-observability/drift-profiler-python/` on
> the `main` branch re-runs the full test + build pipeline.** If the
> version in [`driftdockerprofiler/__version__.py`](driftdockerprofiler/__version__.py)
> is new (not yet on PyPI), CI publishes the wheels + sdist and tags
> the commit `drift-profiler-python-v<x.y.z>`. If the version is
> unchanged, CI builds and tests but does not publish.

### Day-to-day release flow

1. Make your code changes inside `drift-observability/drift-profiler-python/`.
2. Bump [`driftdockerprofiler/__version__.py`](driftdockerprofiler/__version__.py)
   (semver — patch / minor / major).
3. Add a `## <new-version>` entry to [`CHANGELOG.md`](CHANGELOG.md)
   summarizing what changed.
4. Commit and push (PR → main, or push directly to main if your
   workflow allows).
5. On merge to `main`, GitHub Actions:
   - Runs the pytest matrix (Python 3.8 → 3.13 on Ubuntu, plus a
     macOS smoke).
   - Builds `manylinux2014_x86_64` wheels for cp37–cp313 via
     [`cibuildwheel`](https://cibuildwheel.pypa.io/).
   - Builds the source distribution (`sdist`).
   - Publishes to PyPI via **OIDC trusted publisher** (no API token
     stored in GitHub secrets).
   - Pushes the tag `drift-profiler-python-v<x.y.z>` and attaches all
     artifacts to a GitHub Release.

### Manual local publish (escape hatch)

If CI is down or you need to publish from your laptop:

```bash
cd drift-observability/drift-profiler-python

# Wheels via Docker (matches CI exactly — same base image, same gcc).
make wheel-driftdockerprofiler          # → dist/*.whl
python -m build --sdist                 # → dist/*.tar.gz

twine check dist/*                      # validate metadata + long_description

# Smoke-test on TestPyPI first.
twine upload --repository testpypi dist/*
pip install --index-url https://test.pypi.org/simple/ driftdockerprofiler

# Then for real.
twine upload dist/*
```

You'll need `pip install build twine` once on your machine, and a
`~/.pypirc` with your PyPI account credentials (or
`TWINE_USERNAME=__token__ TWINE_PASSWORD=pypi-...`).

### Version policy

- **Patch** (`0.0.1 → 0.0.2`) — bug fixes, doc-only changes, no API
  changes.
- **Minor** (`0.0.1 → 0.1.0`) — new features, new public API.
  Backward compatible.
- **Major** (`0.0.1 → 1.0.0`) — breaking changes to public API,
  removed kwargs, changed defaults that meaningfully change output.

Always update [`CHANGELOG.md`](CHANGELOG.md) in the same commit as
the version bump. CI uses the changelog entry as the GitHub Release
body.

---

## License

Apache License 2.0 — see [LICENSE](LICENSE).

This package is a fork of
[`google-cloud-profiler`](https://github.com/GoogleCloudPlatform/cloud-profiler-python)
(Apache 2.0). The upstream copyright notice is preserved in each
source file alongside the Refactor Labs modification notice, per the
license terms.

---

## About Refactor Labs

**[Refactor Labs](https://refactorlab.com)** builds production
observability tools for engineering teams that ship fast.

- **FinPos** — financial-position reconciliation for fintech
  operations.
- **Drift** — captures the *running shape* of your production code
  (call stacks, hot paths, memory pressure) before it causes a
  regression in front of customers.

This profiler is the Python instrumentation layer underneath Drift.
It's open-source so you can run it standalone, audit the agent
yourself, and ship its output anywhere — a local file, your own log
pipeline, Supabase, or the full Drift stack.

Found a bug? Have a feature request?
[Open an issue](https://github.com/refactorlab/drift/issues).

---
