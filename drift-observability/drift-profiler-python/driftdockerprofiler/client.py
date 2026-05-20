# Copyright 2018 Google LLC
# Modifications copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    https://www.apache.org/licenses/LICENSE-2.0
"""Writes profiles to a local JSONL file (fork of googlecloudprofiler.client).

This is a SURGICAL fork of upstream `googlecloudprofiler.client.Client`.
All GCP transport (google.auth, googleapiclient, httplib2, requests,
google.protobuf, GCE metadata probes) was deleted. The two methods
that actually crossed the network had their bodies swapped for local
equivalents:

  - `_create_profile()` — upstream long-polled the GCP CreateProfile
    RPC, blocking until the server said "now". We have no server, so
    we synthesize the spec locally — round-robin profile types every
    `duration_ms` ms. The RETURN SHAPE is preserved verbatim so
    `_collect_and_upload_profile()` is otherwise unchanged.

  - `_collect_and_upload_profile()` — upstream base64-encoded the
    pprof bytes and PATCH'd them to GCP. We append JSONL lines to a
    local file via `JsonlWriter`. The surrounding try/except
    BaseException + the call chain are kept verbatim.

Preserved verbatim from upstream:

  - Class name (`Client`) and file name (`client.py`).
  - `_poll_profiler_service` polling loop (minus the outer
    `_build_service` waiter — there's nothing to build).
  - `try/except BaseException` tolerance in `_collect_and_upload_profile`
    — this is what keeps the polling daemon alive through transient
    failures, and it's still doing exactly that work.
  - `backoff.Backoff` exponential retry on `_create_profile` errors.
  - `_config_cpu_profiling` / `_config_wall_profiling` helpers.
  - Service-name regex + env fallbacks (GAE_SERVICE/K_SERVICE,
    GAE_VERSION/K_REVISION).
  - Profile-type literals `'WALL'`/`'CPU'` (uppercase). Lowercase
    only happens at the JSONL serialization boundary, to match the
    `wall_trace`/`cpu_trace` event-type discriminators in the schema.
  - SIGALRM main-thread registration via `register_handler()`.
  - Daemon thread pattern.

Improvements over upstream:

  - Explicit `stop()` method + `stop_event` (upstream had no shutdown
    path — its daemon thread leaked the JsonlWriter buffer on exit).
  - `atexit.register(self._stop_at_exit)` so a clean process exit
    flushes the writer.
  - Pre-emptive main-thread check before `signal.signal` — clearer
    error than upstream's implicit ValueError.
  - Per-event `cpu` (loadavg) + `memory_bytes` (RSS) labels.
  - Two output modes (`per_trace` / `bundle`) via `profiler_json`.

This module has ZERO external runtime dependencies (the upstream
client.py pulled in ~30 transitive packages).
"""

import atexit
import logging
import os
import platform
import re
import socket
import sys
import threading
import time
import traceback
import uuid

from driftdockerprofiler import backoff
from driftdockerprofiler import metrics
from driftdockerprofiler import profiler_json
from driftdockerprofiler import pythonprofiler
from driftdockerprofiler import tracer
# Single source of truth for filtering / exclude rules — see
# `filters.py` module docstring for the layering diagram. Re-exported
# below for back-compat with code that imports the constants from
# `driftdockerprofiler.client`.
from driftdockerprofiler.filters import (
    BUILTIN_EXCLUDE_PATHS,
    STRICT_USER_CODE_EXCLUDE_PATHS,
    TraceFilter,
    is_system_frame as _is_system_frame,
)
from driftdockerprofiler.sampling import wall_all_threads
from driftdockerprofiler.sinks import JsonlFileSink, Sink
from driftdockerprofiler.writer import frames_to_dicts

# pylint: disable=g-import-not-at-top
if sys.platform.startswith('linux'):
  try:
    from driftdockerprofiler import cpu_profiler
  except ImportError:    # native _profiler ext not built into this install
    cpu_profiler = None
else:
  # CPU profiling is only supported on Linux (SIGPROF semantics on
  # macOS are too limited).
  cpu_profiler = None

# This module sometimes catches the general BaseException — preserved
# verbatim from upstream. Justification (same as upstream's):
#   - The polling loop runs in a daemon thread; Python signals are
#     handled by the main thread, so we are not blocking user
#     interruptions such as Ctrl+C.
#   - We can't predict every exception the underlying profilers
#     (especially the C++ extension) may throw, and a single transient
#     failure must not kill the agent.

_NANOS_PER_SEC = 1000 * 1000 * 1000

# Same service-name regex upstream uses.
_SERVICE_RE = re.compile(r'^[a-z0-9]([-a-z0-9_.]{0,253}[a-z0-9])?$')


def _sink_label(sink):
  """Human-readable identifier for any Sink-protocol object.

  The startup log line wants to say *where events are going*. Each
  sink type names that target differently:

    - `JsonlFileSink` / `JsonlWriter` → on-disk path (`.path`).
    - `SupabaseRealtimeSink`          → redacted WSS URL (`.safe_url`).
    - `TeeSink`                       → recursive list of children.
    - anything else (custom / test doubles) → its class name.

  Falls back to the class name when no known attribute is present, so
  third-party sinks never break the start-up log.
  """
  for attr in ('path', 'safe_url'):
    value = getattr(sink, attr, None)
    if value is not None:
      return str(value)
  children = getattr(sink, 'sinks', None)
  if children is not None:
    return 'TeeSink[%s]' % ', '.join(_sink_label(c) for c in children)
  return type(sink).__name__

# Output discriminator. Upstream had no equivalent — it always produced
# one pprof-binary per profile window.
_EMIT_MODES = ('per_trace', 'bundle')

# Wall-clock sampling strategies. Both produce {trace: count} dicts in
# the same shape; they differ in which threads they observe:
#
#   'all_threads' (default) — daemon thread + sys._current_frames(),
#                             covers EVERY Python thread. Required to
#                             see code that runs on uvicorn / gunicorn
#                             threadpool workers, Django WSGI workers,
#                             loop.run_in_executor, etc. This is the
#                             dd-trace-py / Sentry / Pyroscope shape.
#
#   'signal'                — legacy upstream behaviour: SIGALRM +
#                             ITIMER_REAL. Main thread only. Kept for
#                             back-compat and for niche cases where
#                             ALL request work runs on the main thread
#                             (single-worker gunicorn sync workers).
#
# Default is 'all_threads' because the SIGALRM strategy silently misses
# user code in every popular web stack — the most common production
# deployment pattern. Users who specifically want the legacy shape
# pass `wall_strategy='signal'`.
_WALL_STRATEGIES = ('all_threads', 'signal')

# Exclude / filter rules live in `driftdockerprofiler.filters` —
# imported at the top of this file as `BUILTIN_EXCLUDE_PATHS`,
# `STRICT_USER_CODE_EXCLUDE_PATHS`, `_SYSTEM_PATH_PREFIXES`,
# `TraceFilter`, `_is_system_frame`. See that module's docstring for
# the two-layer "thread-name skip at sample time, file-path filter at
# emit time" architecture. This module is the consumer side: Client
# constructs a `TraceFilter` at config() and applies it at
# _write_window().


def _agent_version():
  """Return the running agent's version string for the F2
  ``profile_metadata`` Generator block. Lazy-imported so a malformed
  ``__version__`` module doesn't fail import of the rest of the
  client; a blank string is preferable to crashing the agent at
  start-up over a labelling field.
  """
  try:
    from driftdockerprofiler import __version__ as version_mod
    return getattr(version_mod, '__version__', '') or ''
  except Exception:  # pragma: no cover - defensive
    return ''


def _now_iso():
  """ISO-8601 UTC timestamp at microsecond precision, ending in 'Z'.
  Same shape ``JsonlWriter._ns_to_iso`` produces — re-uses the
  writer's helper rather than duplicating the format string so
  there's a single source of truth for our timestamp format.
  """
  from driftdockerprofiler.writer import _ns_to_iso
  return _ns_to_iso(time.time_ns())


logger = logging.getLogger(__name__)


def cpu_profiling_available():
  """Return True iff the native CPU profiler can be loaded on this install."""
  return cpu_profiler is not None


class Client:
  """Writes profiles to a local JSONL file.

  Two-phase init mirrors upstream: `Client()` then `client.config(...)`
  then `client.start()`. Upstream had this split because `setup_auth()`
  came in the middle (GCP credentials needed before `config()`); we
  keep the shape even though `setup_auth()` is gone, because it's the
  natural place to do per-deployment overrides via env vars between
  the two calls.
  """

  def __init__(self):
    # Backoff state for `_create_profile` failures. Reset on success,
    # advanced on failure. Same pattern upstream uses. Retained for
    # back-compat with the legacy `_poll_profiler_service` (round-robin)
    # method; the concurrent per-type polling threads each maintain
    # their own local `backoff.Backoff()` so a transient failure in
    # one profile type can't slow another down.
    self._backoff = backoff.Backoff()
    self._started = False
    self._profilers = None
    self._writer = None
    self._stop_event = threading.Event()
    # Legacy single-thread handle — set to the LAST started thread for
    # any external code that still introspects it. The authoritative
    # list lives in `_polling_threads`. Kept so tests / subclasses
    # that read `self._polling_thread` continue to compile.
    self._polling_thread = None
    # One polling thread per profile type. Populated in start(),
    # drained in stop(). Each thread runs `_poll_one_profiler` for
    # one type continuously, so WALL + CPU sample concurrently
    # instead of alternating — closes the "request fell into a
    # CPU window" wall-sampling gap.
    self._polling_threads = []
    self._atexit_registered = False

  def config(self,
             service=None,
             service_version=None,
             output_path=None,
             period_ms=10,
             duration_ms=10_000,
             disable_cpu_profiling=False,
             disable_wall_profiling=False,
             pod=None,
             emit_mode='per_trace',
             builtin_exclude_paths=BUILTIN_EXCLUDE_PATHS,
             exclude_paths=(),
             sink=None,
             wall_strategy='all_threads'):
    """Sets up the client config.

    Mirrors `googlecloudprofiler.Client.config` — the GCP-only args
    (`project_id`, `discovery_service_url`) are replaced with the
    local-sink kwargs (`output_path`, `duration_ms`, `emit_mode`).

    Args of note (others mirror upstream verbatim):
      output_path: Path the default JSONL sink writes to. Ignored when
        `sink` is passed explicitly.
      sink: Pre-built `Sink`-protocol object (anything with `emit(event)`
        and `close()`). When given, replaces the default
        `JsonlFileSink` — use for Supabase WSS streaming
        (`SupabaseRealtimeSink`), fan-out to multiple destinations
        (`TeeSink`), or test doubles. When `None` (default), a
        `JsonlFileSink(output_path)` is built — fully backwards
        compatible with the legacy `output_path`-only API.

    Raises:
      ValueError: If `service` can't be determined, doesn't match the
        regex, no profiling mode is enabled, period_ms / duration_ms
        non-positive, or emit_mode is unknown.
      TypeError: If `sink` is provided but doesn't implement the Sink
        protocol.
    """
    if emit_mode not in _EMIT_MODES:
      raise ValueError(
          'emit_mode must be one of %r, got %r' % (_EMIT_MODES, emit_mode))
    if wall_strategy not in _WALL_STRATEGIES:
      raise ValueError(
          'wall_strategy must be one of %r, got %r' % (
              _WALL_STRATEGIES, wall_strategy))
    if period_ms <= 0:
      raise ValueError('period_ms must be positive')
    if duration_ms <= 0:
      raise ValueError('duration_ms must be positive')

    self._profilers = {}
    self._config_cpu_profiling(disable_cpu_profiling, period_ms)
    self._config_wall_profiling(disable_wall_profiling, period_ms,
                                wall_strategy)
    if not self._profilers:
      raise ValueError('No profiling mode is enabled.')

    service = (service
               or os.environ.get('GAE_SERVICE')
               or os.environ.get('K_SERVICE'))
    if not service:
      raise ValueError(
          'Service name must be provided via configuration or '
          'GAE_SERVICE / K_SERVICE environment variable.')
    if not _SERVICE_RE.match(service):
      raise ValueError(
          'Service name "%s" does not match regular expression "%s"' % (
              service, _SERVICE_RE.pattern))

    service_version = (service_version
                       or os.environ.get('GAE_VERSION')
                       or os.environ.get('K_REVISION'))

    self._service = service
    self._service_version = service_version or ''
    self._pod = pod or os.environ.get('HOSTNAME') or socket.gethostname()
    self._duration_ms = duration_ms
    self._duration_ns = duration_ms * 1_000_000
    self._emit_mode = emit_mode
    # All filter logic lives in `filters.TraceFilter` — single
    # responsibility, single source of truth. We construct it here
    # with the two-layer (builtin + user-extras) config and call it
    # in `_write_window`. The `_exclude_paths` attribute below is
    # kept for tests / external code that read it directly.
    self._trace_filter = TraceFilter(
        builtin_patterns=builtin_exclude_paths,
        extra_patterns=exclude_paths,
    )
    self._exclude_paths = self._trace_filter.patterns
    # Sink resolution — caller may pre-build any Sink-protocol object
    # (TeeSink, SupabaseRealtimeSink, etc.) and inject it directly; if
    # they don't, we build the same JsonlFileSink the legacy
    # output_path path has always built. JsonlFileSink is just
    # JsonlWriter under the new name, so existing tests that inspect
    # `self._writer.path` / `self._writer.emitted` keep working.
    if sink is not None:
      if not isinstance(sink, Sink):
        raise TypeError(
            'sink must implement the Sink protocol (emit(event) + close()), '
            'got %r' % (type(sink).__name__,))
      self._writer = sink
    else:
      self._writer = JsonlFileSink(output_path)
    # Round-robin index for `_create_profile`. Upstream got the next
    # profile type from the server; we cycle locally.
    self._next_profile_idx = 0

  def start(self):
    """Starts collecting profiles.

    Starts a daemon thread that calls `_create_profile()` and
    `_collect_and_upload_profile()` in a loop — same shape as upstream.
    The difference is that both methods now operate locally instead of
    talking to GCP.

    Raises:
      RuntimeError: If called from a non-main thread when WALL
        profiling is enabled (SIGALRM is only delivered to the main
        thread on CPython). Upstream raised a ValueError from
        signal.signal here; we promote that to a clearer error
        message earlier.
    """
    if self._started:
      logger.warning('Profiler already started, will not start again')
      return

    if 'WALL' in self._profilers:
      wall = self._profilers['WALL']
      # Only the SIGALRM strategy requires main-thread install.
      # `WallAllThreadsSampler` sets `REQUIRES_MAIN_THREAD = False`;
      # legacy `WallProfiler` doesn't define the attribute, so we
      # default to True for safety.
      if getattr(wall, 'REQUIRES_MAIN_THREAD', True):
        if threading.current_thread() is not threading.main_thread():
          raise RuntimeError(
              'Client.start() must be called from the main thread when '
              "wall_strategy='signal' (SIGALRM is only delivered to "
              'the main thread on CPython). Use the default '
              "wall_strategy='all_threads' to start from any thread.")
      wall.register_handler()

    # Wire the deterministic-tracer module to our writer so any
    # `@trace`-decorated function call lands in the same JSONL stream
    # as the sampler events. Cleared in stop().
    tracer._configure(
        writer=self._writer,
        service=self._service,
        pod=self._pod,
        service_version=self._service_version,
    )

    # Phase F2: emit a one-per-session header line that mirrors the
    # static profiler's `profile.schema.json` Generator block. The
    # emit happens BEFORE polling threads spawn so it's guaranteed to
    # be the first JSONL line of the session — consumers reading the
    # file linearly get `mode`/`generator`/`source_root` before any
    # sample event arrives. Compatibility: any consumer that doesn't
    # know the new `type` value already filters by `type` and will
    # skip the line cleanly.
    self._emit_profile_metadata()

    self._stop_event.clear()
    # ONE polling thread PER profile type. Upstream googlecloudprofiler
    # round-robins through types in a single thread (still implemented
    # below as `_poll_profiler_service`, kept callable for back-compat),
    # which means with both WALL and CPU configured each gets only ~50%
    # of wall-clock coverage. A 5-second request landing in a CPU
    # window is invisible to wall sampling. The per-type model below
    # runs WALL and CPU concurrently so wall coverage is continuous —
    # this is what dd-trace-py / Sentry / Pyroscope all do.
    #
    # Compatibility note: the C++ SIGPROF CPU sampler uses ITIMER_PROF,
    # the legacy SIGALRM wall sampler uses ITIMER_REAL — different
    # interval timers, no kernel-level conflict. The new
    # `WallAllThreadsSampler` is pure-Python `sys._current_frames()`
    # with no signals at all, so it stacks cleanly under either CPU
    # path.
    self._polling_threads = []
    for profile_type in sorted(self._profilers.keys()):
      t = threading.Thread(
          target=self._poll_one_profiler,
          args=(profile_type,),
          name='driftdockerprofiler-' + profile_type.lower() + '-poller',
          daemon=True,
      )
      t.start()
      self._polling_threads.append(t)
    # Legacy single-thread handle — point at the last thread started
    # so any code that introspects `self._polling_thread` (subclasses,
    # tests pre-Phase-3) still finds *something* live to look at.
    self._polling_thread = (
        self._polling_threads[-1] if self._polling_threads else None)
    # Upstream relied on `daemon=True` to die on process exit — but
    # that doesn't flush the writer. Register a shutdown so the
    # last window's events make it to disk.
    if not self._atexit_registered:
      atexit.register(self._stop_at_exit)
      self._atexit_registered = True
    self._started = True
    logger.info(
        'Profiler started: service=%s period_ms=%d duration_ms=%d '
        'output=%s profilers=%s emit_mode=%s',
        self._service, self._duration_ms, self._duration_ms,
        _sink_label(self._writer), sorted(self._profilers),
        self._emit_mode)

  def stop(self, timeout=None):
    """Signal every polling thread to stop and join each. Idempotent.

    Not in upstream — its daemon thread had no shutdown path. We need
    one because (a) the JsonlWriter holds a buffer that should flush
    on exit, and (b) tests need to tear down between cases.

    Each polling thread may be blocked inside
    `profiler.profile(duration_ns)`, which is uninterruptible — the
    default `timeout` covers one full window plus 2 seconds of slack
    PER THREAD (joined sequentially), so the worst-case total stop
    latency scales with the number of configured profile types
    (typically 1 or 2 — WALL, CPU).
    """
    if not self._started:
      return
    self._stop_event.set()
    if timeout is None:
      # Each profile() call holds for at most one window. Cover it
      # plus 2s of slack PER THREAD — they're joined sequentially.
      per_thread_timeout = (self._duration_ms / 1000.0) + 2.0
    else:
      per_thread_timeout = timeout
    # Join every per-type polling thread. Pre-Phase-3 there was at
    # most one; we still handle that via the same iteration.
    for t in self._polling_threads:
      try:
        t.join(timeout=per_thread_timeout)
      except Exception:  # pylint: disable=broad-except
        # join() shouldn't raise, but a hostile subclass could shim
        # it. Match Google's "agent must never crash user code"
        # invariant — log and move on.
        logger.warning('Failed to join %s; continuing shutdown',
                       t.name, exc_info=True)
    self._polling_threads = []
    self._polling_thread = None
    # Disable the deterministic tracer BEFORE closing the writer —
    # otherwise a @trace'd call from another thread mid-shutdown
    # could still try to emit to a closed handle.
    tracer._reset()
    if self._writer is not None:
      self._writer.close()
    self._started = False
    logger.info('Profiler stopped')

  def _stop_at_exit(self):
    """atexit-safe shutdown — swallows errors so a failing stop()
    doesn't crash interpreter teardown."""
    try:
      self.stop()
    except Exception:  # pylint: disable=broad-except
      pass

  def _emit_profile_metadata(self):
    """Phase F2: emit a one-per-session header line that mirrors the
    static profiler's `profile.schema.json` Generator + top-level
    fields. Lets a viewer reading the JSONL linearly know the
    session's tool/version/host/source_root BEFORE any sample event
    arrives — and identifies the file's `mode` as ``sampled`` so a
    converter can output an exact `profile.schema.json` document.

    Field shapes are byte-for-byte copies of the upstream Generator
    struct so the Rust converter (Phase F4) is a trivial field copy.

    Failures are swallowed: a broken writer at this single emit point
    must not prevent the polling threads from coming up. The
    JsonlWriter's own `emit()` already swallows OSError; this outer
    try is defence-in-depth.
    """
    try:
      python_version = '%d.%d.%d' % sys.version_info[:3]
      event = {
          'type': 'profile_metadata',
          # `time` matches every other event's format — JsonlWriter
          # rewrites int ns → ISO at emit time.
          'time': time.time_ns(),
          'service': self._service,
          'pod': self._pod,
          'mode': 'sampled',
          # Mirrors `profile.schema.json::schema_version` enum.
          # Bump if/when the dynamic-side wire format changes.
          'schema_version': '1.0',
          # Per-session UUID. Stays stable for every event that
          # follows in this JSONL stream; lets a deployment-wide
          # consumer that aggregates across many pods dedup samples
          # from the same Python process restart.
          'service_id': uuid.uuid4().hex,
          'generator': {
              'tool': 'driftdockerprofiler',
              'version': _agent_version(),
              # platform.node() is the most portable hostname source —
              # works on Linux/macOS/Windows without env var fallbacks.
              'host': platform.node() or self._pod,
              # ISO timestamp of session start. Same instant as `time`
              # above; this field exists so the Generator block is
              # self-contained and a viewer can render it without
              # cross-referencing the event envelope.
              'captured_at': _now_iso(),
              # `getcwd()` is what the static profiler also uses as
              # `source_root` — same join key, no translation.
              'source_root': os.getcwd(),
              'language_versions': {'python': python_version},
          },
      }
      if self._service_version:
        event['service_version'] = self._service_version
      self._writer.emit(event)
    except Exception:  # pylint: disable=broad-except
      # Don't let a header emit failure prevent profiling. Log loud
      # because losing the header silently is its own problem (the
      # converter won't have a Generator block).
      logger.warning('failed to emit profile_metadata header: %s',
                     traceback.format_exc())

  def _config_cpu_profiling(self, disable_cpu_profiling, period_ms):
    """Adds CPU profiler if supported and not disabled. Same as upstream."""
    cpu_profiling_supported = cpu_profiler is not None
    if not cpu_profiling_supported:
      logger.info('CPU profiling is not supported on the current Operating '
                  'System. Linux is the only supported Operating System.')
    elif disable_cpu_profiling:
      logger.info('CPU profiling is disabled by disable_cpu_profiling')
    else:
      self._profilers['CPU'] = cpu_profiler.CPUProfiler(period_ms)

  def _config_wall_profiling(self, disable_wall_profiling, period_ms,
                             wall_strategy):
    """Adds the wall profiler if not disabled.

    Branches on `wall_strategy`:
      'all_threads' — `WallAllThreadsSampler` (daemon thread +
                      `sys._current_frames()`, covers every thread).
      'signal'      — legacy `WallProfiler` (SIGALRM, main thread only).

    Both implement the same protocol (`period_ns` + `profile(duration_ns)
    → {trace: count}`), so the polling loop, filter, and writer paths
    don't care which one is configured.
    """
    if disable_wall_profiling:
      logger.info('Wall profiling is disabled by disable_wall_profiling')
      return
    if wall_strategy == 'all_threads':
      self._profilers['WALL'] = wall_all_threads.WallAllThreadsSampler(
          period_ms)
    else:  # 'signal' — validated by config()
      self._profilers['WALL'] = pythonprofiler.WallProfiler(period_ms)

  def _create_profile(self):
    """Synthesize the next profile-spec locally.

    Replaces upstream's CreateProfile RPC. Upstream's version BLOCKED
    waiting for the GCP backend to schedule a profile; we have no
    backend, so we round-robin through configured profile types every
    `duration_ms` ms.

    The RETURN SHAPE is preserved verbatim from upstream
    (`{'profileType', 'duration': {'seconds', 'nanos'}, 'name'}`) so
    `_collect_and_upload_profile()` reads the same as upstream's.

    Returns:
      A profile-spec dict.
    """
    profile_types = sorted(self._profilers.keys())
    profile_type = profile_types[self._next_profile_idx % len(profile_types)]
    self._next_profile_idx += 1
    return {
        'profileType': profile_type,
        'duration': {
            'seconds': self._duration_ms // 1000,
            'nanos': (self._duration_ms % 1000) * 1_000_000,
        },
        'name': 'local',
    }

  def _collect_and_upload_profile(self, profile):
    """Collects a profile and WRITES IT LOCALLY (was: uploads over HTTP).

    The try/except BaseException pattern is preserved verbatim from
    upstream. The body of the try is the only thing that changed:
    instead of `base64.b64encode(...)` + `self._profiler_service.patch(...)`,
    we run the profiler and call `_write_window()`.

    Single transient errors must not kill the daemon thread — same
    invariant upstream relied on.
    """
    profile_type = profile.get('profileType', '<unknown>')
    try:
      if profile_type not in self._profilers:
        logger.warning('Unexpected profile type: %s', profile_type)
        return

      # Upstream parsed the duration with protobuf's Duration message;
      # we use plain dict lookups (server-supplied was always a
      # `{'seconds', 'nanos'}` JSON object).
      duration = profile['duration']
      duration_ns = (int(duration.get('seconds', 0)) * _NANOS_PER_SEC
                     + int(duration.get('nanos', 0)))

      traces = self._profilers[profile_type].profile(duration_ns)
      window_end_ns = time.time_ns()
      self._write_window(profile_type, traces, window_end_ns)
    except BaseException:  # pylint: disable=broad-except
      logger.warning(
          'Failed to collect and write profile whose profile type is %s: %s',
          profile_type, traceback.format_exc())

  def _should_exclude(self, leaf_file):
    """Thin delegate to `self._trace_filter.should_exclude`.

    Retained as an instance method so tests / subclasses that poke at
    the legacy `client._should_exclude(file)` API keep working. The
    real logic lives in `filters.TraceFilter`.
    """
    return self._trace_filter.should_exclude(leaf_file)

  def _filter_traces(self, traces):
    """Thin delegate to `self._trace_filter.filter`.

    Retained as an instance method for the same reason as
    `_should_exclude` — back-compat with code that calls the legacy
    API. Filtering logic lives in `filters.TraceFilter`.
    """
    return self._trace_filter.filter(traces)

  def _write_window(self, profile_type, traces, window_end_ns):
    """Local-write equivalent of upstream's base64 + PATCH step.

    Dispatches by `emit_mode`. `metrics.snapshot()` is called once
    here, at window close, and the values are reused for every line —
    keeps per-event cost flat and avoids syscalls from inside the
    SIGALRM/SIGPROF handlers (which is async-signal-unsafe for most
    of stdlib).
    """
    if not traces:
      return
    # Apply exclude_paths filter BEFORE the per-trace / bundle
    # dispatch — bundle mode needs the filter to short-circuit on an
    # empty result rather than emit an empty samples[] array.
    traces = self._filter_traces(traces)
    if not traces:
      return
    cpu, mem_bytes, mem_peak = metrics.snapshot()
    if self._emit_mode == 'bundle':
      self._write_bundle(profile_type, traces, window_end_ns,
                         cpu, mem_bytes, mem_peak)
    else:
      self._write_per_trace(profile_type, traces, window_end_ns,
                            cpu, mem_bytes, mem_peak)

  def _write_per_trace(self, profile_type, traces, window_end_ns,
                       cpu, memory_bytes, memory_peak_bytes):
    """Emit one JSONL line per unique stack."""
    profiler = self._profilers[profile_type]
    period_ns = profiler.period_ns
    duration_ns = self._duration_ns
    write = self._writer.emit
    # Internal profile_type is uppercase ('WALL', 'CPU') to match
    # upstream; JSONL `type` discriminator is lowercase
    # ('wall_trace', 'cpu_trace') per the event schema.
    event_type = profiler_json.per_trace_event_type(profile_type.lower())
    for trace, count in traces.items():
      event = {
          'type': event_type,
          'time': window_end_ns,
          'service': self._service,
          'pod': self._pod,
          'period_ns': period_ns,
          'duration_ns': duration_ns,
          'count': int(count),
          'cpu': cpu,
          'memory_bytes': memory_bytes,
          'memory_peak_bytes': memory_peak_bytes,
          'frames': frames_to_dicts(trace, is_system_predicate=_is_system_frame),
      }
      if self._service_version:
        event['service_version'] = self._service_version
      write(event)

  def _write_bundle(self, profile_type, traces, window_end_ns,
                    cpu, memory_bytes, memory_peak_bytes):
    """Emit one JSONL line per window carrying the whole `Profile`."""
    profiler = self._profilers[profile_type]
    builder = profiler_json.Builder()
    builder.populate_profile(
        traces,
        profile_type=profile_type.lower(),
        period_unit='nanoseconds',
        period=profiler.period_ns,
        duration_ns=self._duration_ns,
        time_ns=window_end_ns,
        service=self._service,
        pod=self._pod,
        service_version=self._service_version,
        cpu=cpu,
        memory_bytes=memory_bytes,
        is_system_predicate=_is_system_frame,
    )
    event = builder.to_dict()
    event['type'] = profiler_json.bundle_event_type(profile_type.lower())
    event['time'] = window_end_ns  # JsonlWriter rewrites int → ISO
    # Bundle events also get the peak — keep parity with per_trace.
    event['memory_peak_bytes'] = memory_peak_bytes
    self._writer.emit(event)

  def _poll_one_profiler(self, profile_type):
    """Continuous polling loop scoped to ONE profile type.

    Phase-3 successor to `_poll_profiler_service` (the legacy
    round-robin loop, kept below for back-compat). One of these runs
    per configured profile type, in its own daemon thread, so WALL
    and CPU sample CONCURRENTLY instead of taking turns. That is what
    gives wall sampling 100% wall-clock coverage even when the C++
    SIGPROF CPU sampler is active — closing the "the request landed
    in a CPU window so we missed it" gap from the original Google
    design.

    Loop shape is intentionally identical to upstream
    `_poll_profiler_service` (try/except BaseException, per-call
    `Backoff`, `stop_event.wait` interruptible sleep) so the
    defensive properties carry over: a transient failure inside
    `profile()` or the writer can never crash the daemon thread,
    never stalls peer threads, and recovers with exponential backoff.

    Args:
      profile_type: 'WALL' or 'CPU' (case must match `self._profilers`
        keys).
    """
    # PER-TYPE backoff state. Single Responsibility: this loop owns
    # its own retry rhythm; a flapping CPU sampler can't slow WALL
    # down, and vice versa. (Upstream had ONE shared `self._backoff`
    # because there was ONE loop.)
    local_backoff = backoff.Backoff()

    # The "profile spec" upstream got from a CreateProfile RPC. We
    # synthesize it locally and it never changes for this thread —
    # build once.
    duration_ns = self._duration_ms * 1_000_000
    profile_spec = {
        'profileType': profile_type,
        'duration': {
            'seconds': self._duration_ms // 1000,
            'nanos': (self._duration_ms % 1000) * 1_000_000,
        },
        'name': 'local-' + profile_type.lower(),
    }
    logger.debug('Profiler %s polling thread started '
                 '(continuous, duration_ns=%d)',
                 profile_type, duration_ns)

    while not self._stop_event.is_set():
      try:
        # `_collect_and_upload_profile` itself wraps `BaseException`
        # (preserved verbatim from upstream). The outer try/except
        # here is defence-in-depth — if the inner guard ever stops
        # catching (refactor, signal handler reentry, etc.) we still
        # never let an exception escape this thread.
        self._collect_and_upload_profile(profile_spec)
        local_backoff.reset()
      except BaseException as e:  # pylint: disable=broad-except
        backoff_duration = local_backoff.next_backoff(e)
        logger.warning(
            '[%s] poll loop hit an unhandled exception '
            '(retry after %.3fs): %s',
            profile_type, backoff_duration, str(e))
        # `stop_event.wait` returns True if stop fired during the
        # sleep — short-circuit so a full backoff window doesn't
        # drag out shutdown.
        if self._stop_event.wait(backoff_duration):
          break

    logger.debug('Profiler %s polling thread exiting', profile_type)

  def _poll_profiler_service(self):
    """Polls the (local) profile-spec generator. Forks upstream's
    `_poll_profiler_service`.

    Upstream had TWO nested retry loops:

      OUTER — wait for `_build_service()` (Discovery client) to come
              up. Retried forever with `build_service_backoff`. We
              dropped this — there's no service to build.
      INNER — forever: `_create_profile()` (with backoff on failure)
              → `_collect_and_upload_profile()`.

    We KEEP the inner loop verbatim — including the exponential
    backoff on `_create_profile` failures and the `try/except
    BaseException` wrap. `_create_profile` doesn't talk to the network
    anymore so it won't fail in practice, but the pattern stands so
    if anyone re-adds a remote sink, the resilience drops right back in.
    """
    logger.debug('Profiler has started')

    while not self._stop_event.is_set():
      profile = None
      while not profile and not self._stop_event.is_set():
        try:
          logger.debug('Starting to create profile')
          profile = self._create_profile()
          # Same as upstream: reset backoff after success.
          self._backoff.reset()
          logger.debug('Successfully created a %s profile',
                       profile['profileType'])
        except BaseException as e:  # pylint: disable=broad-except
          # Same as upstream: exponential backoff with optional
          # server-hinted retry delay. The local fork never has a
          # server hint; the exponential path covers everything.
          backoff_duration = self._backoff.next_backoff(e)
          logger.debug(
              'Failed to create profile (will retry after %.3fs): %s',
              backoff_duration, str(e))
          # `stop_event.wait` returns True if stop fired during the
          # sleep — short-circuit so we don't drag out shutdown by a
          # full backoff window. Upstream used a bare `time.sleep`.
          if self._stop_event.wait(backoff_duration):
            return

      if profile is not None:
        self._collect_and_upload_profile(profile)
