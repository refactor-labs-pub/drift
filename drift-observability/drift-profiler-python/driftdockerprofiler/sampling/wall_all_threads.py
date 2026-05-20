# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Wall-clock sampler that walks every Python thread.

Why this exists
---------------
The legacy `pythonprofiler.WallProfiler` uses `signal.SIGALRM` +
`signal.setitimer(ITIMER_REAL, ...)`. CPython delivers Python-level
signals **only to the main thread** — see `pythonprofiler.py`
docstring. In web stacks where handlers run on worker threads
(uvicorn sync routes via `loop.run_in_executor`, gunicorn sync
workers, Django WSGI threadpool, anything using a
`concurrent.futures.ThreadPoolExecutor`), the user's code is
**invisible** to the SIGALRM sampler — every captured leaf is whatever
the main thread happens to be doing (typically `epoll_wait` inside
asyncio).

This sampler runs in its own daemon thread and ticks every `period_ms`
by calling `sys._current_frames()`, which returns `{thread_id →
top_frame}` for every Python thread in the interpreter. It is what
dd-trace-py, Sentry's continuous profiler, and Pyroscope all do in
production.

Frame-lifetime safety (dd-trace-py issue #13567)
------------------------------------------------
`sys._current_frames()` returns *borrowed* references. If the owning
thread returns from a function and frees its frame between us getting
the dict and dereferencing the pointer, we get a use-after-free.
dd-trace-py hit this in production with their Cython sampler.

Mitigation: read `co_name`, `co_filename`, `f_lineno` synchronously
inside the GIL-held window — i.e., the sample loop is pure Python
with no `Py_BEGIN_ALLOW_THREADS` block. As long as we don't release
the GIL during the walk (and we don't), no other thread runs, so
no frame can be freed.

Drop-in compatibility with `WallProfiler`
-----------------------------------------
Same external shape:

  - `period_ns` property
  - `profile(duration_ns)` → `{(name, file, line)-tuple-trace: count}`
  - `register_handler()` no-op (signal-strategy artefact; kept so
    `client.py` can call it blindly without knowing the strategy)

What is NOT shared:

  - `REQUIRES_MAIN_THREAD = False` — runs from any thread.
  - No `signal.setitimer` / `signal.signal` calls; pure
    `threading.Thread`. Safe under `gunicorn`/`uvicorn` workers that
    install their own signal handlers.
"""

import collections
import logging
import sys
import threading
import time

from driftdockerprofiler.filters import is_internal_thread_name

# Maximum stack frames to record — matches legacy WallProfiler.
_MAX_STACK_DEPTH = 128
_NANOS_PER_SEC = 1000 * 1000 * 1000

logger = logging.getLogger(__name__)


class WallAllThreadsSampler:
    """Wall-clock sampler covering all Python threads.

    See module docstring for the design rationale. This class is
    deliberately small — the heavy lifting is `sys._current_frames()`.
    """

    # Internal `Client`-orchestration knob: tells `Client.start()`
    # whether to enforce the main-thread caller invariant. The legacy
    # SIGALRM `WallProfiler` requires main-thread because
    # `signal.signal` only works there; this sampler doesn't.
    REQUIRES_MAIN_THREAD = False

    PROFILE_TYPE = 'wall'

    def __init__(self, period_ms):
        if period_ms <= 0:
            raise ValueError('period_ms must be positive, got %r' % (period_ms,))
        self._period_ms = period_ms
        self._period_sec = period_ms / 1000.0
        self._period_ns = period_ms * 1_000_000
        # Trace counts accumulated across the current window. Cleared
        # at the top of every profile() call.
        self._traces = collections.defaultdict(int)
        self._trace_count = 0
        # Lock guards _traces / _trace_count — the sampler thread
        # writes, profile() reads at window close.
        self._lock = threading.Lock()
        # Per-profile-call state. None outside a profile() window.
        self._stop = None
        self._sampler_thread = None
        # Own thread id, set in _sample_loop. We MUST skip ourselves
        # otherwise every tick produces a sample whose leaf is the
        # sampler — pure noise.
        self._sampler_tid = None

    @property
    def period_ns(self):
        return self._period_ns

    def register_handler(self):
        """No-op. Kept for interface parity with `WallProfiler`.

        Legacy `WallProfiler.register_handler` installs a process-wide
        `signal.SIGALRM` handler from the main thread. This sampler
        uses a daemon thread instead and has nothing to install.
        `Client.start()` calls `register_handler()` on whichever wall
        strategy it holds, blindly — so leaving this in place keeps
        the call site uniform.
        """

    def profile(self, duration_ns):
        """Sample for `duration_ns`, return `{trace: count}`.

        Spawns a daemon thread that wakes every `period_ms` and walks
        every other thread's stack. The calling thread blocks on
        `_stop.wait(duration)` so this method has the same blocking
        semantics as `WallProfiler.profile()` (the polling loop in
        `Client` expects this).

        Same return shape as `WallProfiler.profile()`:
        `dict[tuple[(name, file, line), ...], int]`. The `unknown`
        backfill matches `WallProfiler` so consumers don't see
        strategy-dependent count arithmetic.
        """
        with self._lock:
            self._traces = collections.defaultdict(int)
            self._trace_count = 0

        self._stop = threading.Event()
        self._sampler_thread = threading.Thread(
            target=self._sample_loop, daemon=True,
            name='driftdockerprofiler-wall-all-threads',
        )
        self._sampler_thread.start()
        try:
            # Block for the requested window. `stop` may be set from
            # outside (Client.stop()), in which case we exit early —
            # mirrors WallProfiler's behaviour of one full sleep then
            # done.
            self._stop.wait(duration_ns / _NANOS_PER_SEC)
        finally:
            self._stop.set()
            # 2s join slack matches WallProfiler._stop_profiling.
            self._sampler_thread.join(timeout=2.0)
            self._sampler_thread = None

        with self._lock:
            traces = dict(self._traces)
            count = self._trace_count
            self._traces = collections.defaultdict(int)
            self._trace_count = 0

        # Match WallProfiler's "unknown" backfill so the per-window
        # count arithmetic stays consistent across wall strategies.
        # Without this, downstream consumers (the icicle chart's
        # total-time accounting, especially) would see strategy-
        # dependent totals.
        #
        # Frame is 5-tuple `(name, file, line, qualified_name, module)`
        # per Phase F1b — same shape as the real-frame path in
        # `_snapshot_frames`. Emitting a 3-tuple here breaks
        # downstream consumers that unpack positionally (and breaks
        # the test suite's structural assertion on frame shape).
        expected = max(int(duration_ns / self._period_ns), 1)
        if count < expected:
            traces[(('unknown', 'unknown', 0, 'unknown', 'unknown'),)] = (
                expected - count)
        return traces

    def _sample_loop(self):
        """Daemon-thread loop: tick every `period_sec` and accumulate.

        Drift correction: we anchor on `time.monotonic()` and advance
        `next_wake` by a fixed step each iteration, so a slow tick
        doesn't shift the cadence. If we fall behind by more than one
        period, we re-anchor rather than burst-fire to catch up.
        """
        self._sampler_tid = threading.get_ident()
        next_wake = time.monotonic()
        while not self._stop.is_set():
            self._take_sample()
            next_wake += self._period_sec
            sleep_for = next_wake - time.monotonic()
            if sleep_for > 0:
                if self._stop.wait(sleep_for):
                    break
            elif sleep_for < -self._period_sec:
                # More than one period behind — re-anchor.
                next_wake = time.monotonic()

    def _take_sample(self):
        """One tick: walk every thread's stack, accumulate counts.

        The GIL is held throughout (pure-Python, no GIL release in this
        method), so the frame pointers returned by
        `sys._current_frames()` remain valid for the entire walk —
        the dd-trace-py #13567 mitigation.

        Profiler-owned threads (polling loop, sink workers, the
        sampler itself) are skipped here BEFORE walking — that's
        cheaper than walking + post-filtering, and it's the only
        place we can catch the case where a profiler-owned thread is
        blocked in stdlib (`threading.Event.wait`, `queue.Queue.get`,
        `socket.recv`). The leaf-frame `exclude_paths` filter in
        `client.TraceFilter` can't see those because the leaf is
        stdlib, not `/driftdockerprofiler/`. Detection routes through
        the shared `filters.is_internal_thread_name` helper, which
        gates on the `driftdockerprofiler-` thread-name prefix every
        internal `threading.Thread()` call site uses.

        Failures are swallowed: a transient error in one tick must not
        kill the daemon. Matches the upstream googlecloudprofiler
        try/except BaseException pattern in `_collect_and_upload_profile`.
        """
        try:
            frames = sys._current_frames()
        except Exception:  # pragma: no cover - defensive
            return

        # Snapshot thread-id → name once per tick. `threading.enumerate`
        # holds an internal lock briefly; we call it once and read
        # from the dict for the rest of the walk so the per-thread
        # check is O(1) and lock-free.
        thread_names = {t.ident: t.name for t in threading.enumerate()}

        my_tid = self._sampler_tid
        with self._lock:
            for tid, frame in frames.items():
                if tid == my_tid:
                    # Skip ourselves — sampling the sampler is noise.
                    continue
                if is_internal_thread_name(thread_names.get(tid, '')):
                    # Skip every profiler-owned worker thread —
                    # otherwise traces rooted in `_poll_one_profiler`
                    # / sink workers leak `driftdockerprofiler.*`
                    # parent frames into the output.
                    continue
                trace = _walk_frame(frame)
                if not trace:
                    continue
                self._traces[trace] += 1
                self._trace_count += 1


def _walk_frame(frame, max_depth=_MAX_STACK_DEPTH):
    """Copy frame attrs immediately — never retain the frame pointer.

    Returns a leaf-first tuple of 5-tuples
    ``(name, file, line, qualified_name, module)`` — the same shape
    `WallProfiler._record_trace` returns after Phase F1b. Downstream
    code (`_filter_traces`, `frames_to_dicts`, `Builder.populate_profile`)
    handles both 3- and 5-tuple shapes, so the C++ CPU sampler
    (still 3-tuple) flows through unchanged.

    Reading `co_name`, `co_filename`, `co_qualname` is a borrowed
    read of interned strings; `f_lineno` is an int access; `f_globals`
    is the per-frame module dict. All happen synchronously while the
    GIL is held, so the frame can't be freed mid-walk.
    """
    depth = 0
    out = []
    while frame is not None and depth < max_depth:
        # Read all attrs into local Python objects BEFORE advancing —
        # the use-after-free guard. Don't store the frame pointer
        # itself anywhere.
        co = frame.f_code
        name = co.co_name
        file = co.co_filename
        line = frame.f_lineno
        # `co_qualname` is Python 3.11+. getattr keeps 3.7-3.10
        # working; empty string → field absent on the wire.
        qualname = getattr(co, 'co_qualname', '') or ''
        globals_ = frame.f_globals
        # f_globals is documented to always be a dict; defensive
        # `or ''` survives interpreter-teardown edge cases.
        module = globals_.get('__name__', '') if globals_ else ''
        out.append((name, file, line, qualname, module))
        frame = frame.f_back
        depth += 1
    return tuple(out)
