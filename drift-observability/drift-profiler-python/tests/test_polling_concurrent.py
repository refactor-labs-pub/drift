# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Phase-3 tests — one polling thread PER profile type.

Before Phase 3, `Client` ran a single polling thread that round-robined
through profile types (WALL → CPU → WALL → CPU at duration_ms each).
With both samplers configured, WALL got ~50 % wall-clock coverage; a
request landing in a CPU window was invisible to wall sampling. Phase
3 spawns one continuous polling thread per type, so WALL + CPU run
concurrently and wall coverage is 100 %.

These tests pin down the new architectural contract:
  - `Client.start()` spawns one thread per profile type.
  - Each thread is named after the type (introspectable in tracebacks
    and `threading.enumerate()`).
  - `Client.stop()` joins every thread cleanly.
  - The legacy `_polling_thread` attribute still exists (back-compat
    with any external introspector); the new authoritative list is
    `_polling_threads`.
  - The legacy round-robin entry point `_poll_profiler_service` is
    still importable / callable (back-compat) but no longer used by
    `start()`.
"""

import sys
import threading
import time

import pytest

from driftdockerprofiler import Client

pytestmark = pytest.mark.skipif(
    not (sys.platform.startswith("linux") or sys.platform.startswith("darwin")),
    reason="POSIX-only (signals + setitimer).",
)


def _make_client(**overrides):
    """Build a configured-but-not-started Client. Wall-only by default
    so the test works without the C++ CPU extension."""
    c = Client()
    cfg = dict(
        service="t",
        output_path="/tmp/drift-test-phase3.jsonl",
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
    )
    cfg.update(overrides)
    c.config(**cfg)
    return c


def test_start_spawns_one_polling_thread_per_profile_type():
    """With WALL configured, one polling thread is alive after start()."""
    c = _make_client()
    try:
        c.start()
        # Phase-3 invariant: one thread per profile type.
        assert len(c._polling_threads) == len(c._profilers)
        assert all(t.is_alive() for t in c._polling_threads)
    finally:
        c.stop()


def test_polling_thread_is_named_after_profile_type():
    """Threads must be greppable in stack dumps / py-spy output."""
    c = _make_client()
    try:
        c.start()
        names = [t.name for t in c._polling_threads]
        assert any("wall" in n.lower() for n in names), \
            f"expected a WALL polling thread, got names={names}"
    finally:
        c.stop()


def test_stop_joins_every_polling_thread():
    """No zombie threads after stop()."""
    c = _make_client()
    c.start()
    threads = list(c._polling_threads)
    c.stop()
    # Give the OS a beat to mark each thread as dead.
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline and any(t.is_alive() for t in threads):
        time.sleep(0.01)
    for t in threads:
        assert not t.is_alive(), f"polling thread {t.name} still alive after stop()"
    assert c._polling_threads == []


def test_legacy_polling_thread_attribute_still_set():
    """Back-compat: code that introspects `self._polling_thread`
    (subclasses, pre-Phase-3 tests) must still find a live thread."""
    c = _make_client()
    try:
        c.start()
        assert c._polling_thread is not None
        assert c._polling_thread.is_alive()
        # And it must be one of the per-type threads — not a phantom.
        assert c._polling_thread in c._polling_threads
    finally:
        c.stop()


def test_legacy_poll_profiler_service_method_still_callable():
    """Back-compat: subclasses overriding `_poll_profiler_service`
    must still find the method to call into."""
    c = _make_client()
    assert hasattr(c, "_poll_profiler_service")
    assert callable(c._poll_profiler_service)


def test_per_type_threads_have_independent_backoff():
    """Phase-3 design: a WALL failure must not slow CPU (and vice
    versa). Each polling thread owns its own `Backoff` — the shared
    `Client._backoff` (kept for back-compat with `_poll_profiler_service`)
    must be untouched by the new per-type loops.

    Proof: monkey-patch the shared `_backoff.reset` to record calls,
    run a window or two, and assert it was never invoked.
    """
    c = _make_client()
    calls = []
    original_reset = c._backoff.reset
    c._backoff.reset = lambda: calls.append(1) or original_reset()
    try:
        c.start()
        time.sleep(0.5)  # let at least two windows complete
        assert calls == [], (
            "Shared Client._backoff.reset() was called %d times — the "
            "per-type polling threads must use their OWN local backoff, "
            "not the shared one." % len(calls))
    finally:
        c.stop()
