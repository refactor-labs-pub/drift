# Copyright 2018 Google LLC
# Modifications copyright 2026 drift
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    https://www.apache.org/licenses/LICENSE-2.0
"""Local-file profiler — forked from Google Cloud Profiler.

The original agent authenticated to Google Cloud and uploaded gzipped
pprof bundles to the cloudprofiler v2 service. This fork strips that
transport layer but otherwise stays a close fork of upstream:

  - `client.py` mirrors `googlecloudprofiler.client` (Client class,
    _poll_profiler_service, _create_profile, _collect_and_upload_profile,
    backoff.Backoff exponential retry, try/except BaseException
    tolerance).
  - The two methods that crossed the network upstream had their bodies
    swapped: _create_profile now synthesizes the spec locally,
    _collect_and_upload_profile now writes JSONL to disk.
  - No google-api-python-client, google-auth, google-auth-httplib2,
    protobuf, or requests dependencies — wheel has zero runtime deps.

Usage::

    import driftdockerprofiler
    driftdockerprofiler.start(service='my-service')
    # ... run your app ...

Each profile window (default 10 s) emits one JSONL line per unique
call stack seen during the window, with a `count` field for the
number of samples that hit that stack.
"""

import logging
import os
import sys

from driftdockerprofiler import __version__ as version
from driftdockerprofiler import filters, profiler_json, schemas
from driftdockerprofiler.client import (
    Client,
    cpu_profiling_available,
)
from driftdockerprofiler.filters import (
    BUILTIN_EXCLUDE_PATHS,
    STRICT_USER_CODE_EXCLUDE_PATHS,
    TraceFilter,
    is_internal_thread_name,
    is_system_frame,
)
from driftdockerprofiler.profiler_json import (
    Builder,
    Frame,
    Profile,
    Sample,
    ValueType,
)
from driftdockerprofiler.schemas import (
    VALID_EVENT_TYPES,
    ValidationError,
    all_schemas,
    event_schema,
    schema_for,
    schema_path,
    validate_event,
)
from driftdockerprofiler.sinks import (
    JsonlFileSink,
    Sink,
    SupabaseRealtimeSink,
    TeeSink,
)
from driftdockerprofiler.sinks import supabase as _supabase_sink
from driftdockerprofiler.tracer import trace
from driftdockerprofiler.writer import DEFAULT_OUTPUT_PATH, JsonlWriter

__all__ = [
    'BUILTIN_EXCLUDE_PATHS',
    'Builder',
    'Client',
    'DEFAULT_OUTPUT_PATH',
    'Frame',
    'JsonlFileSink',
    'JsonlWriter',
    'Profile',
    'STRICT_USER_CODE_EXCLUDE_PATHS',
    'Sample',
    'Sink',
    'SupabaseRealtimeSink',
    'TeeSink',
    'TraceFilter',
    'VALID_EVENT_TYPES',
    'ValidationError',
    'ValueType',
    'all_schemas',
    'cpu_profiling_available',
    'event_schema',
    'filters',
    'is_internal_thread_name',
    'is_system_frame',
    'profiler_json',
    'schema_for',
    'schema_path',
    'schemas',
    'start',
    'stop',
    'trace',
    'validate_event',
]

_started = False
_client = None

logger = logging.getLogger(__name__)


def start(service=None,
          service_version=None,
          output_path=None,
          period_ms=10,
          duration_ms=10_000,
          disable_cpu_profiling=False,
          disable_wall_profiling=False,
          emit_mode='per_trace',
          pod=None,
          builtin_exclude_paths=BUILTIN_EXCLUDE_PATHS,
          exclude_paths=(),
          sink=None,
          supabase_url=None,
          supabase_api_key=None,
          supabase_channel=None,
          wall_strategy='all_threads',
          verbose=0):
  """Starts the profiler.

  Module-level convenience that mirrors upstream's
  `googlecloudprofiler.start` shape (single function call to get
  going). Under the hood it builds a `Client`, calls `client.config(...)`,
  then `client.start()` — same two-phase pattern upstream documents.

  Every setting follows the same resolution order: explicit kwarg →
  environment variable → built-in default. Pass nothing and the
  profiler reads everything from the environment; pass a kwarg and it
  overrides whatever the environment says.

  Args:
    service: Service name. Must match
      '^[a-z0-9]([-a-z0-9_.]{0,253}[a-z0-9])?$'. Falls back to the
      GAE_SERVICE / K_SERVICE env vars.
    service_version: Optional version label stamped on every event.
      Falls back to GAE_VERSION / K_REVISION env vars.
    output_path: JSONL file the agent appends to. Falls back to the
      `DRIFT_OUTPUT_PATH` env var, then to `/tmp/drift/events.jsonl`.
      Parent directory is created if needed. Ignored when a Supabase
      sink is built (kwargs/env supply URL + key).
    period_ms: Sampling interval (ms). Default 10.
    duration_ms: Length of each profile window (ms). After the window
      closes, every unique stack becomes one JSONL line. Default
      10_000 (10 s).
    disable_cpu_profiling: When True, skip CPU profiling. Defaults to
      False. Ignored on platforms where the native extension was not
      built (macOS, Windows, source install without a C++ compiler).
    disable_wall_profiling: When True, skip wall profiling.
    emit_mode: `'per_trace'` (default) emits one JSONL line per unique
      stack per window. `'bundle'` emits one JSONL line per window
      carrying the whole `Profile` (all samples inlined). See
      `driftdockerprofiler.profiler_json` for the schema.
    pod: Optional pod / host label. Defaults to $HOSTNAME or
      socket.gethostname().
    builtin_exclude_paths: The "baked-in" filter defaults. Substrings
      matched against each sample's leaf-frame `file`; matching
      samples are dropped. Defaults to `BUILTIN_EXCLUDE_PATHS` which
      hides the profiler itself + Python stdlib + frozen importlib +
      site-packages — i.e. everything that isn't user code. Override
      with `()` to disable, or with a custom tuple to redefine.
    exclude_paths: ADDITIONAL user-supplied patterns stacked on top of
      `builtin_exclude_paths`. Common case: keep the defaults and add
      a couple of project-specific patterns, e.g.
      `exclude_paths=('/sqlalchemy/', '/celery/')`.
    sink: Pre-built `Sink`-protocol object. If given, wins outright —
      the Supabase kwargs / env vars below are ignored. Use for
      `TeeSink`, custom destinations, or test doubles.
    supabase_url: Supabase project URL (e.g.
      ``https://abc123.supabase.co``). Overrides the
      ``SUPABASE_URL`` env var. When this + `supabase_api_key` both
      resolve, the profiler switches into **broadcast mode** —
      `SupabaseRealtimeSink` joins the channel and every event is
      emitted as a Phoenix broadcast instead of being written to disk.
    supabase_api_key: Supabase API key (publishable or service_role
      JWT). Overrides ``SUPABASE_REALTIME_API_KEY``. Required (with
      `supabase_url`) to enable broadcast mode.
    supabase_channel: Channel name joined as ``realtime:<channel>``.
      Overrides ``SUPABASE_REALTIME_CHANNEL``. Defaults to
      ``drift-profiler-events`` when neither kwarg nor env supplies
      one.
    verbose: Logging level. 0=error, 1=warning, 2=info, 3=debug.

  Raises:
    ValueError: invalid service name; both profilers disabled; missing
      service.
    NotImplementedError: unsupported OS (everything other than Linux
      and macOS).
  """
  global _started, _client
  if _started:
    logger.warning(
        'driftdockerprofiler.start() called again after it was '
        'previously called. This function should only be called once. '
        'This call is ignored.')
    return

  logging.basicConfig()
  if not (sys.platform.startswith('linux') or sys.platform.startswith('darwin')):
    raise NotImplementedError('%s OS is not supported.' % (sys.platform,))

  level = [logging.ERROR, logging.WARNING, logging.INFO,
           logging.DEBUG][min(verbose, 3)]
  logger.setLevel(level)

  # output_path: kwarg → DRIFT_OUTPUT_PATH env → Client default.
  # Resolved here (not in Client.config) so the precedence is uniform
  # with the Supabase settings below.
  if output_path is None:
    output_path = os.environ.get('DRIFT_OUTPUT_PATH')

  # Auto-wire Supabase if URL + API key are resolvable (kwarg or env)
  # and no explicit sink was passed. Resolution order per setting:
  #   1. Explicit kwarg (supabase_url / supabase_api_key /
  #      supabase_channel).
  #   2. Environment variable (SUPABASE_URL /
  #      SUPABASE_REALTIME_API_KEY / SUPABASE_REALTIME_CHANNEL).
  #   3. Default (channel only — defaults to
  #      `drift-profiler-events`).
  #
  # Outer precedence:
  #   1. Explicit `sink=` kwarg wins (anything goes — TeeSink, mocks).
  #   2. Else, if URL + API key are both set, stream over WSS instead
  #      of writing to file (broadcast mode).
  #   3. Else, fall through to the legacy `JsonlFileSink(output_path)`
  #      built inside `Client.config()`.
  if sink is None:
    sink = _supabase_sink.from_env(
        supabase_url=supabase_url,
        api_key=supabase_api_key,
        channel=supabase_channel,
    )
    if sink is not None:
      logger.info(
          'driftdockerprofiler: Supabase URL + API key resolved — '
          'streaming events over WSS instead of writing to file')

  _client = Client()
  _client.config(
      service=service,
      service_version=service_version,
      output_path=output_path,
      period_ms=period_ms,
      duration_ms=duration_ms,
      disable_cpu_profiling=disable_cpu_profiling,
      disable_wall_profiling=disable_wall_profiling,
      pod=pod,
      emit_mode=emit_mode,
      builtin_exclude_paths=builtin_exclude_paths,
      exclude_paths=exclude_paths,
      sink=sink,
      wall_strategy=wall_strategy,
  )
  logger.info('Cloud Profiler Python agent version: %s (local-file fork)',
              version.__version__)
  _client.start()
  _started = True


def stop():
  """Stop the running profiler. Idempotent."""
  global _started, _client
  if _client is None:
    return
  _client.stop()
  _client = None
  _started = False
