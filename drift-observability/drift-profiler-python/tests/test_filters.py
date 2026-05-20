# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for the consolidated `driftdockerprofiler.filters` module.

The filters module is the single source of truth for "is this
profiler-internal noise" — used by:

  - the wall sampler (`is_internal_thread_name`, thread-level skip
    BEFORE walking — the new layer added to fix the bug where
    profiler-owned threads blocked in stdlib leaked their parent
    frames into emitted traces);
  - the client (`TraceFilter`, file-path filter applied AFTER traces
    are produced — second-line drop + user-extras + STRICT preset).

Existing `test_exclude_paths.py` exercises the Client-side delegation
(`Client._should_exclude` / `Client._filter_traces`). This file
exercises the filters module directly so a future move of the
delegates won't lose coverage.
"""

import sys
import threading
import time

import pytest

from driftdockerprofiler.filters import (
    BUILTIN_EXCLUDE_PATHS,
    INTERNAL_THREAD_PREFIX,
    STRICT_USER_CODE_EXCLUDE_PATHS,
    SYSTEM_PATH_PREFIXES,
    TraceFilter,
    is_internal_thread_name,
    is_system_frame,
    matches_any,
)
from driftdockerprofiler.sampling.wall_all_threads import WallAllThreadsSampler


# --------------------------------------------------- INTERNAL_THREAD_PREFIX

def test_prefix_is_a_non_empty_string():
    """A regression guard — an empty prefix would mark every thread
    as internal and the sampler would walk nothing."""
    assert isinstance(INTERNAL_THREAD_PREFIX, str)
    assert INTERNAL_THREAD_PREFIX


def test_internal_thread_name_matches_polling_thread():
    """`Client.start` names its polling threads with this exact shape."""
    assert is_internal_thread_name('driftdockerprofiler-wall-poller')
    assert is_internal_thread_name('driftdockerprofiler-cpu-poller')


def test_internal_thread_name_matches_sampler_thread():
    """The wall sampler names itself with this prefix."""
    assert is_internal_thread_name('driftdockerprofiler-wall-all-threads')


def test_internal_thread_name_matches_supabase_worker():
    """The Supabase sink names its websocket worker with the prefix."""
    assert is_internal_thread_name('driftdockerprofiler-supabase-sink')


def test_internal_thread_name_rejects_user_threads():
    """User-owned thread names — anything not starting with the
    prefix — must pass through (we DO want to sample those)."""
    assert not is_internal_thread_name('MainThread')
    assert not is_internal_thread_name('ThreadPoolExecutor-0_1')
    assert not is_internal_thread_name('uvicorn-worker')
    assert not is_internal_thread_name('Thread-3')


def test_internal_thread_name_handles_empty_and_none():
    """Edge cases that show up at interpreter teardown / from C threads."""
    assert not is_internal_thread_name('')
    assert not is_internal_thread_name(None)


# --------------------------------------------------- matches_any

def test_matches_any_finds_substring():
    assert matches_any('/app/foo.py', ('/foo',))
    assert matches_any('/usr/lib/python3.11/asyncio/runners.py',
                       ('/asyncio/', '/threading/'))


def test_matches_any_returns_false_when_no_match():
    assert not matches_any('/app/orders.py', ('/asyncio/',))


def test_matches_any_handles_empty_inputs():
    """All falsy inputs short-circuit to False — never raises."""
    assert not matches_any('', ('/foo',))
    assert not matches_any('/app/foo.py', ())
    assert not matches_any(None, ('/foo',))
    # Empty pattern strings inside the tuple must be IGNORED (an
    # empty string is a substring of every other string — we'd
    # match everything and drop the whole world).
    assert not matches_any('/app/foo.py', ('',))


# --------------------------------------------------- is_system_frame

def test_is_system_frame_matches_profiler_self():
    assert is_system_frame(
        '/usr/local/lib/python3.11/site-packages/driftdockerprofiler/cpu_profiler.py')
    assert is_system_frame('/opt/driftdockerprofiler/_profiler.cpython-311.so')


def test_is_system_frame_matches_frozen_bootstrap():
    assert is_system_frame('<frozen importlib._bootstrap>')


def test_is_system_frame_matches_stdlib_and_site_packages():
    """The STRICT preset is part of `SYSTEM_PATH_PREFIXES`, so stdlib
    and site-packages are labelled `is_system=True` regardless of
    whether the user has the strict preset enabled at filter time."""
    assert is_system_frame('/usr/lib/python3.11/asyncio/runners.py')
    assert is_system_frame(
        '/venv/lib/python3.11/site-packages/fastapi/routing.py')


def test_is_system_frame_returns_false_for_user_code():
    assert not is_system_frame('/app/orders.py')
    assert not is_system_frame('/srv/myproject/handlers.py')


def test_is_system_frame_handles_falsy():
    assert not is_system_frame('')
    assert not is_system_frame(None)


def test_system_path_prefixes_is_union_of_two_lists():
    """Regression: the labelling and filtering rules must stay in
    sync. If `SYSTEM_PATH_PREFIXES` drifts away from the union, the
    `is_system` label will disagree with the strict-preset filter."""
    assert set(SYSTEM_PATH_PREFIXES) == (
        set(BUILTIN_EXCLUDE_PATHS) | set(STRICT_USER_CODE_EXCLUDE_PATHS))


# --------------------------------------------------- TraceFilter

def test_trace_filter_drops_profiler_self_leaf():
    """The default builtin patterns drop a trace whose leaf is in
    the profiler package."""
    f = TraceFilter()
    profiler_self = (
        ('profile', '/opt/site-packages/driftdockerprofiler/cpu_profiler.py', 42),
    )
    user = (('create', '/app/orders.py', 14),)
    out = f.filter({profiler_self: 5, user: 3})
    assert profiler_self not in out
    assert out[user] == 3


def test_trace_filter_default_keeps_stdlib():
    """Stdlib is NOT in the builtin defaults — must pass through."""
    f = TraceFilter()
    asyncio = (('run', '/usr/lib/python3.11/asyncio/runners.py', 43),)
    out = f.filter({asyncio: 1})
    assert asyncio in out


def test_trace_filter_extras_stack_on_top():
    """User-supplied extras are ADDITIVE — both layers drop traces."""
    f = TraceFilter(extra_patterns=('/sqlalchemy/',))
    profiler_self = (('p', '/opt/driftdockerprofiler/x.py', 1),)
    orm = (('q', '/app/sqlalchemy/engine.py', 99),)
    user = (('c', '/app/orders.py', 14),)
    out = f.filter({profiler_self: 1, orm: 1, user: 1})
    assert profiler_self not in out
    assert orm not in out
    assert user in out


def test_trace_filter_disabled_returns_input_unchanged():
    """`builtin=()` + `extras=()` means no filter — pass through, no copy."""
    f = TraceFilter(builtin_patterns=(), extra_patterns=())
    traces = {(('any', '/x.py', 1),): 99}
    assert f.filter(traces) is traces  # identity, not just equality


def test_trace_filter_only_inspects_leaf_frame():
    """A user-code leaf with a profiler frame deeper in the stack is kept."""
    f = TraceFilter()
    mixed = (
        ('create', '/app/orders.py', 14),  # leaf — user code
        ('helper', '/opt/driftdockerprofiler/x.py', 1),
    )
    out = f.filter({mixed: 1})
    assert mixed in out


def test_trace_filter_skips_empty_traces():
    """Empty traces are noise — drop without inspecting."""
    f = TraceFilter()
    out = f.filter({(): 5, (('c', '/app/x.py', 1),): 1})
    assert () not in out


def test_trace_filter_normalizes_patterns_to_tuple_of_str():
    """A list and a Path-like would otherwise sneak in mutable state."""
    f = TraceFilter(builtin_patterns=['/a/'], extra_patterns=['/b/'])
    assert f.patterns == ('/a/', '/b/')
    # Mutating the input list after construction must not change the filter.
    extras = ['/c/']
    f2 = TraceFilter(builtin_patterns=(), extra_patterns=extras)
    extras.append('/d/')
    assert f2.patterns == ('/c/',)


# --------------------------------------------------- sampler-level skip

@pytest.mark.skipif(
    not (sys.platform.startswith('linux') or sys.platform.startswith('darwin')),
    reason='wall sampler uses pthread / GIL semantics; skipped on non-POSIX',
)
def test_sampler_skips_profiler_owned_threads():
    """The new sampler-side fix: a thread named with the profiler
    prefix must NEVER appear in the captured traces, even though it's
    a real Python thread doing real work.

    Before this fix, the wall sampler walked every thread except
    itself. Profiler-owned threads (polling loop, sink worker) sat
    blocked in `threading.Event.wait` / `queue.Queue.get` — leaf in
    stdlib, parents in `/driftdockerprofiler/` — and the leaf-only
    `exclude_paths` filter couldn't catch them. They emitted as
    `wall_trace` events with `driftdockerprofiler.*` frames.

    Setup: spawn a thread with the profiler prefix that does
    something visible (`time.sleep`). Run the sampler. Assert NONE of
    the captured traces reference our marker function.
    """
    sampler = WallAllThreadsSampler(period_ms=5)

    def _fake_internal_worker():
        # Use a distinctive function name we can grep for in the
        # captured traces.
        time.sleep(0.3)

    fake = threading.Thread(
        target=_fake_internal_worker,
        name='driftdockerprofiler-fake-worker-for-test',
    )
    fake.start()
    try:
        traces = sampler.profile(duration_ns=400_000_000)  # 0.4 s
    finally:
        fake.join()

    # Walk every frame of every captured trace; no frame should
    # mention our fake worker function.
    function_names = set()
    for trace in traces:
        for frame in trace:
            function_names.add(frame[0] if frame else '')
    assert '_fake_internal_worker' not in function_names, (
        'sampler must skip threads with the driftdockerprofiler- '
        'name prefix; found %r in captured traces' % sorted(function_names))


@pytest.mark.skipif(
    not (sys.platform.startswith('linux') or sys.platform.startswith('darwin')),
    reason='wall sampler uses pthread / GIL semantics; skipped on non-POSIX',
)
def test_sampler_still_walks_user_threads():
    """Counterpart guard: the skip must NOT over-fire. A user thread
    (no profiler prefix) doing visible work must still appear.
    Without this, a tightening of the prefix check could silently
    blackhole every user thread."""
    sampler = WallAllThreadsSampler(period_ms=5)

    def _visible_user_function():
        time.sleep(0.3)

    user = threading.Thread(
        target=_visible_user_function,
        name='my-user-worker',
    )
    user.start()
    try:
        traces = sampler.profile(duration_ns=400_000_000)
    finally:
        user.join()

    function_names = set()
    for trace in traces:
        for frame in trace:
            function_names.add(frame[0] if frame else '')
    assert '_visible_user_function' in function_names, (
        'user threads (no profiler prefix) must still be sampled; '
        'captured names: %r' % sorted(function_names))
