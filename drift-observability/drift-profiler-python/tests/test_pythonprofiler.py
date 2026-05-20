# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""End-to-end tests for `driftdockerprofiler.pythonprofiler.WallProfiler`.

These tests exercise the real SIGALRM path. They must run on the main
thread (signal.signal restriction) and produce at least a handful of
samples while we burn CPU.
"""

import signal
import sys
import threading

import pytest

from driftdockerprofiler.pythonprofiler import WallProfiler

pytestmark = pytest.mark.skipif(
    not (sys.platform.startswith('linux') or sys.platform.startswith('darwin')),
    reason='wall profiler uses POSIX signals; skipped on non-POSIX',
)


def _busy_burn(duration_s):
    """Burn wall time without doing syscalls."""
    import time
    end = time.monotonic() + duration_s
    n = 0
    while time.monotonic() < end:
        n += 1
    return n


def test_profile_returns_dict_of_traces():
    p = WallProfiler(period_ms=5)
    p.register_handler()
    try:
        traces = p.profile(duration_ns=200_000_000)  # 0.2 s
    finally:
        # Disarm the timer so signal traffic doesn't leak between tests.
        signal.setitimer(signal.ITIMER_REAL, 0)

    assert isinstance(traces, dict)
    # ~40 samples at 5 ms; aggregated into far fewer unique stacks.
    assert sum(traces.values()) >= 5, traces

    # Every key is a tuple of frame 5-tuples post-F1b
    # `(name, file, line, qualified_name, module)`. Positions 0-2 are
    # unchanged from the legacy 3-tuple shape, so any consumer that
    # only needed `(name, file, line)` keeps working via positional
    # access without code changes.
    for trace, count in traces.items():
        assert isinstance(trace, tuple) and trace
        assert isinstance(count, int) and count >= 1
        for frame in trace:
            assert isinstance(frame, tuple) and len(frame) == 5
            name, fname, lineno, qualname, module = frame
            assert isinstance(name, str)
            assert isinstance(fname, str)
            assert isinstance(lineno, int)
            assert isinstance(qualname, str)
            assert isinstance(module, str)


def test_profile_clears_state_between_runs():
    """A second profile() call must NOT accumulate samples from the first."""
    p = WallProfiler(period_ms=5)
    p.register_handler()
    try:
        _ = p.profile(duration_ns=100_000_000)
        traces2 = p.profile(duration_ns=100_000_000)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    # Reset means trace_count starts at 0 in the new window. We can't
    # verify the EXACT counts (timing-dependent), but we CAN verify
    # the new dict is small relative to "two windows' worth".
    total = sum(traces2.values())
    # 100 ms / 5 ms = 20 samples per window. Two windows would be 40.
    # The fresh window must be near 20, not 40.
    assert total <= 30, f'state leaked between profile windows: {total}'


def test_unknown_trace_filled_in_when_handler_missed_ticks():
    """When `trace_count` < expected ticks, the missing slots get attributed
    to a synthetic ('unknown', 'unknown', 0) trace. This test forces a
    zero-tick window by NEVER starting the timer (we call profile with a
    duration but the handler can't fire because we patch out the start).
    """
    p = WallProfiler(period_ms=10)
    # Don't register — handler not installed → no ticks → all unknown.
    # We invoke the private state directly to bypass the actual sampling.
    p._reset()
    p._trace_count = 0
    traces = p._collect_and_clear_traces(duration_ns=100_000_000)  # 100 ms

    # 100 ms / 10 ms period = 10 expected ticks, all unknown.
    assert traces == {(('unknown', 'unknown', 0),): 10}


def test_period_ns_property():
    p = WallProfiler(period_ms=7)
    assert p.period_ns == 7_000_000


def test_profile_type_constant():
    assert WallProfiler.PROFILE_TYPE == 'wall'
