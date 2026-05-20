# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for `WallAllThreadsSampler` — the bug-fix for the user's
"I don't see my code" complaint.

The headline test (`test_threadpool_worker_code_is_visible`) is the
ship gate: it spawns a thread doing `time.sleep`, runs the sampler,
asserts the sleeping thread's code shows up as a leaf frame. The
legacy SIGALRM `WallProfiler` cannot pass this test by construction.
"""

import concurrent.futures
import sys
import threading
import time

import pytest

from driftdockerprofiler import Client
from driftdockerprofiler.sampling.wall_all_threads import (
    WallAllThreadsSampler,
    _walk_frame,
)


# ----------------------------------------------------------------- the headline test

def test_threadpool_worker_code_is_visible():
    """The user's bug, distilled. A function running on a
    ThreadPoolExecutor worker (== how uvicorn dispatches sync FastAPI
    routes) MUST appear as a leaf frame in the wall traces.

    The legacy SIGALRM-based `WallProfiler` cannot satisfy this — its
    handler only ever fires on the main thread.
    """
    sampler = WallAllThreadsSampler(period_ms=5)

    def slow_user_code():
        # The exact pattern the user complained about: a synchronous
        # sleep simulating slow work. `time.sleep` is a C builtin so
        # the topmost Python frame at sample time is THIS function.
        time.sleep(0.3)

    def run_in_threadpool():
        with concurrent.futures.ThreadPoolExecutor() as ex:
            ex.submit(slow_user_code).result()

    # Kick off the workload in a side thread so the sampler (which
    # runs in ANOTHER background thread) can observe it.
    worker = threading.Thread(target=run_in_threadpool)
    worker.start()
    try:
        traces = sampler.profile(duration_ns=500_000_000)  # 0.5 s
    finally:
        worker.join()

    # Collect every name that appeared anywhere in any captured
    # trace. We don't care about `line` — line numbers shift when
    # the test file is edited. Frame is 3-tuple pre-F1b, 5-tuple
    # post-F1b; index [0] is always the name in both shapes.
    seen_names = set()
    for trace in traces:
        for f in trace:
            seen_names.add(f[0])

    assert 'slow_user_code' in seen_names, (
        'slow_user_code MUST appear in wall traces — this is the '
        'whole reason WallAllThreadsSampler exists. Sampled names: '
        + ', '.join(sorted(seen_names))[:500]
    )


# ----------------------------------------------------------------- interface contract

def test_implements_sampler_shape():
    """Same external shape as the legacy WallProfiler."""
    s = WallAllThreadsSampler(period_ms=10)
    assert s.period_ns == 10_000_000
    assert hasattr(s, 'profile')
    assert hasattr(s, 'register_handler')   # no-op, but callable
    s.register_handler()                     # must not raise


def test_does_not_require_main_thread():
    """The flag `Client.start()` reads to decide whether to enforce
    main-thread caller. The all-threads sampler clears it."""
    assert WallAllThreadsSampler.REQUIRES_MAIN_THREAD is False


def test_period_ms_must_be_positive():
    with pytest.raises(ValueError, match='period_ms'):
        WallAllThreadsSampler(period_ms=0)
    with pytest.raises(ValueError, match='period_ms'):
        WallAllThreadsSampler(period_ms=-5)


# ----------------------------------------------------------------- behavior

def test_profile_returns_dict_of_trace_to_count():
    s = WallAllThreadsSampler(period_ms=5)
    traces = s.profile(duration_ns=100_000_000)  # 0.1 s
    assert isinstance(traces, dict)
    for trace, count in traces.items():
        assert isinstance(trace, tuple) and trace
        assert isinstance(count, int) and count >= 1
        for frame in trace:
            # Phase F1b: frame is 5-tuple now
            # `(name, file, line, qualified_name, module)`. The first
            # three positions are unchanged so downstream consumers
            # that only need `(name, file, line)` keep working via
            # positional access.
            assert len(frame) == 5
            name, file, line, qualname, module = frame
            assert isinstance(name, str)
            assert isinstance(file, str)
            assert isinstance(line, int)
            assert isinstance(qualname, str)
            assert isinstance(module, str)


def test_sampler_does_not_sample_itself():
    """The daemon-thread loop runs in 'driftdockerprofiler-wall-
    all-threads'. It MUST skip its own thread or every tick emits a
    sample whose leaf is the sampler — pure noise."""
    s = WallAllThreadsSampler(period_ms=5)
    traces = s.profile(duration_ns=200_000_000)
    sampler_self_seen = False
    for trace in traces:
        # Frame is 3-tuple pre-F1b, 5-tuple post-F1b. Positional
        # indexing tolerates both — we only need name + file here.
        for f in trace:
            name, file = f[0], f[1]
            if name == '_sample_loop' or name == '_take_sample':
                sampler_self_seen = True
            if 'wall_all_threads.py' in file and name in (
                    '_sample_loop', '_take_sample', '_walk_frame'):
                sampler_self_seen = True
    assert not sampler_self_seen, (
        'Sampler must not appear in its own traces')


def test_unknown_backfill_matches_wall_profiler():
    """If the sampler caught fewer ticks than expected (interpreter
    busy doing something we can't observe), the difference is
    accounted for as an `unknown` trace. Matches `WallProfiler`."""
    s = WallAllThreadsSampler(period_ms=10)
    traces = s.profile(duration_ns=50_000_000)  # 50 ms — only ~5 ticks
    # We expect at least one trace OR the unknown backfill. Empty
    # output would mean the sampler is broken.
    assert traces, 'profile() returned an empty dict'


# ----------------------------------------------------------------- _walk_frame

def test_walk_frame_returns_leaf_first():
    """Top of stack at position 0; deeper frames after."""
    def inner():
        # sys._getframe(0) is `inner` itself.
        return _walk_frame(sys._getframe(0))

    def outer():
        return inner()

    trace = outer()
    # First frame is the leaf — `inner`. The next ones include
    # `outer`, then `test_walk_frame_returns_leaf_first`, etc.
    # Frame is 3- or 5-tuple — index [0] is always the name.
    names = [f[0] for f in trace]
    assert names[0] == 'inner'
    assert 'outer' in names
    assert 'test_walk_frame_returns_leaf_first' in names


def test_walk_frame_respects_max_depth():
    def recurse(n):
        if n == 0:
            return _walk_frame(sys._getframe(0), max_depth=5)
        return recurse(n - 1)

    trace = recurse(20)
    assert len(trace) == 5


# ----------------------------------------------------------------- Client wiring

def test_client_defaults_to_all_threads_strategy():
    """The default `wall_strategy` is `'all_threads'` so the user's
    bug fixes itself with zero configuration changes."""
    c = Client()
    c.config(
        service='t',
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
    )
    assert isinstance(c._profilers['WALL'], WallAllThreadsSampler)


def test_client_can_opt_into_legacy_signal_strategy():
    from driftdockerprofiler.pythonprofiler import WallProfiler
    c = Client()
    c.config(
        service='t',
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
        wall_strategy='signal',
    )
    assert isinstance(c._profilers['WALL'], WallProfiler)


def test_client_rejects_unknown_strategy():
    c = Client()
    with pytest.raises(ValueError, match='wall_strategy'):
        c.config(
            service='t',
            period_ms=10,
            duration_ms=200,
            disable_cpu_profiling=True,
            wall_strategy='bogus',
        )


def test_all_threads_strategy_can_start_off_main_thread():
    """The whole point of the new sampler — no main-thread
    requirement. The Client.start() guard must let us through."""
    import io
    import contextlib

    errors = []

    def boot_off_main():
        try:
            c = Client()
            c.config(
                service='t',
                period_ms=10,
                duration_ms=100,
                disable_cpu_profiling=True,
                output_path='/tmp/drift-test-off-main.jsonl',
                # wall_strategy defaults to 'all_threads'
            )
            # We DON'T call start() here because that spawns the
            # polling thread and we'd need to coordinate shutdown.
            # The relevant guard fires inside start(); test by
            # calling start() and immediately stop().
            c.start()
            c.stop()
        except RuntimeError as e:
            errors.append(e)

    t = threading.Thread(target=boot_off_main)
    t.start()
    t.join(timeout=5.0)
    assert errors == [], (
        'all_threads strategy must work off main thread, got: %r' % errors)


def test_signal_strategy_still_requires_main_thread():
    """The legacy guard must still fire for `wall_strategy='signal'` —
    the SIGALRM constraint hasn't changed."""
    errors = []

    def boot_off_main():
        try:
            c = Client()
            c.config(
                service='t',
                period_ms=10,
                duration_ms=100,
                disable_cpu_profiling=True,
                output_path='/tmp/drift-test-signal-off-main.jsonl',
                wall_strategy='signal',
            )
            c.start()
        except RuntimeError as e:
            errors.append(e)

    t = threading.Thread(target=boot_off_main)
    t.start()
    t.join(timeout=5.0)
    assert len(errors) == 1
    assert 'main thread' in str(errors[0])


# ----------------------------------------------------------------- F1b

def test_walk_frame_captures_qualified_name_and_module():
    """Phase F1b: the wall sampler tuple now carries qualified_name
    (CPython 3.11+ only) and module. The first three positions stay
    `(name, file, line)` so downstream tests / consumers keep working."""
    class Holder:
        def method(self):
            return _walk_frame(sys._getframe(0))

    trace = Holder().method()
    leaf = trace[0]
    assert len(leaf) == 5
    name, file, line, qualname, module = leaf
    assert name == 'method'
    # qualified_name is Py 3.11+ only. When present, it must include
    # the class. CPython qualifies nested classes with the enclosing
    # function's `<locals>` segment, e.g.
    # `test_…<locals>.Holder.method` — we check the suffix.
    # When absent (3.7-3.10), we get '' — both are valid.
    if sys.version_info >= (3, 11):
        assert qualname.endswith('Holder.method'), (
            'Py 3.11+ should populate co_qualname with class prefix; '
            'got %r' % qualname)
    else:
        assert qualname == '', (
            'Py < 3.11 has no co_qualname; expected empty string, '
            'got %r' % qualname)
    # `module` should be this test module's __name__. Same on every
    # supported Python version.
    assert module == __name__


def test_walk_frame_qualified_name_empty_for_top_level_function():
    """For a plain (not method) function the qualified name equals
    just the function name on Py 3.11+, and '' on 3.7-3.10."""
    def top_level():
        return _walk_frame(sys._getframe(0))

    trace = top_level()
    qualname = trace[0][3]
    if sys.version_info >= (3, 11):
        # `co_qualname` for a nested function uses '<locals>' — e.g.
        # 'test_walk_frame_..._for_top_level_function.<locals>.top_level'
        assert qualname.endswith('top_level'), qualname
    else:
        assert qualname == ''
