# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Sampler protocol.

This file does NOT introduce a new class hierarchy. The legacy
profilers (`pythonprofiler.WallProfiler`, `cpu_profiler.CPUProfiler`)
already implement the shape this protocol describes — they
predate the protocol. We declare the contract here so future samplers
(see Phase 1: `WallAllThreadsSampler`) have a target to conform to.

Contract — every sampler MUST provide:

  - `period_ns` (property)   — sampling interval in nanoseconds.
  - `profile(duration_ns)`   — run for that many ns, return
                               `dict[tuple[(name, file, line), ...], int]`
                               mapping each unique call stack to its
                               sample count.

Optional methods, used only by strategies that need them:

  - `register_handler()`     — install a process-wide signal handler
                               from the main thread (SIGALRM
                               strategy only). New samplers should
                               leave this absent or no-op.

`Client` introspects each sampler for `register_handler` via
`hasattr` so absence is the cleanest signal of "this sampler doesn't
need a main-thread install step."
"""

# `Protocol` and `runtime_checkable` moved into `typing` in Python
# 3.8. On 3.7 (the test app's runtime — python:3.7-slim) they live
# in `typing_extensions`. The shim keeps the wheel installable on
# both lines without forcing typing_extensions on 3.8+.
from typing import Any, Dict, Tuple
try:
    from typing import Protocol, runtime_checkable
except ImportError:  # Python 3.7
    from typing_extensions import Protocol, runtime_checkable

# A stack frame as it leaves the sampler: (function name, file path,
# line number). Leaf-first. The wire format expands this into a dict
# in `writer.frames_to_dicts` but the in-memory trace stays a tuple of
# tuples — cheap to hash, dedups identical stacks for free.
Frame = Tuple[str, str, int]
Trace = Tuple[Frame, ...]
TraceCounts = Dict[Trace, int]


@runtime_checkable
class Sampler(Protocol):
    """The shape `Client` calls into.

    Both `WallProfiler` and `CPUProfiler` already match this; no
    adapter needed. New samplers (Phase 1+) should conform.
    """

    @property
    def period_ns(self) -> int: ...

    def profile(self, duration_ns: int) -> TraceCounts: ...
