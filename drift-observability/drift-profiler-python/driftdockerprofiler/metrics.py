# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Lightweight process / system metrics.

Captures the three numbers we stamp on every emitted event:

  - **cpu**:               1-minute system load average
                           (`os.getloadavg()[0]`). Single syscall,
                           ~µs latency. We don't compute a "%CPU"
                           because the meaningful denominator (number
                           of cores) is something the consumer
                           already knows. Load gives an absolute
                           "system busy" signal that aggregates
                           cleanly across hosts.
  - **memory_bytes**:      Current Resident Set Size (RSS) in bytes.
                           Linux: `/proc/self/statm`. macOS / BSD:
                           `resource.getrusage(RUSAGE_SELF).ru_maxrss`.
  - **memory_peak_bytes**: Peak RSS observed since process start, in
                           bytes. `ru_maxrss` is monotonically
                           non-decreasing — useful for spotting
                           transient spikes the current-RSS probe
                           may have missed.

All three probes are intentionally cheap: they're called once per
profile window in `Client._write_window`, *not* per emitted sample.
That keeps the agent's overhead bounded as the number of unique
stacks per window grows.
"""

from __future__ import annotations

import os
import sys
from typing import Tuple


# Cached page size — `os.sysconf` is a syscall on most platforms and
# the value never changes during a process's lifetime.
try:
  _PAGE_SIZE = os.sysconf('SC_PAGE_SIZE')
except (OSError, AttributeError, ValueError):
  # Default to 4 KiB — correct on every modern x86_64 / arm64 Linux.
  _PAGE_SIZE = 4096


def cpu_load() -> float:
  """Return the 1-minute load average, or 0.0 if unavailable.

  `os.getloadavg` is POSIX (Linux + macOS); Windows raises
  AttributeError. We swallow OS errors so the agent never crashes
  because of a metric probe — losing one number is fine.
  """
  try:
    return float(os.getloadavg()[0])
  except (OSError, AttributeError):
    return 0.0


def memory_rss_bytes() -> int:
  """Return the calling process's resident set size, in bytes.

  Path 1 (Linux): `/proc/self/statm` — second column is RSS in pages,
  multiplied by `SC_PAGE_SIZE`. One open + one short read, no parse
  beyond `.split()[1]`. This is the same source `top` and `htop` use.

  Path 2 (macOS / other POSIX): `resource.getrusage().ru_maxrss`.
  Beware: on Linux ru_maxrss is in *kilobytes*, on macOS it's in
  *bytes*. We only hit this path on non-Linux so the bytes
  interpretation is correct here.

  Returns 0 on Windows or when both probes fail.
  """
  if sys.platform.startswith('linux'):
    try:
      with open('/proc/self/statm') as f:
        # Format: "size resident shared text lib data dt" (all in pages)
        return int(f.read().split()[1]) * _PAGE_SIZE
    except (OSError, ValueError, IndexError):
      pass  # fall through to rusage

  try:
    import resource
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # macOS: bytes already. Other BSDs match macOS here.
    return int(rss)
  except (ImportError, OSError):
    return 0


def memory_peak_bytes() -> int:
  """Return the calling process's PEAK resident set size since start.

  Source: `resource.getrusage(RUSAGE_SELF).ru_maxrss`. Same unit
  convention as `memory_rss_bytes`:
    Linux: ru_maxrss is in kilobytes — multiply by 1024.
    macOS / BSD: ru_maxrss is in bytes already.

  ru_maxrss is monotonic non-decreasing for the lifetime of the
  process; it answers "what's the largest this process has ever
  been" and never shrinks even if memory is freed.

  Returns 0 if `resource` is unavailable (Windows) or rusage fails.
  """
  try:
    import resource
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if sys.platform.startswith('linux'):
      return int(rss) * 1024   # kilobytes → bytes
    return int(rss)             # macOS / BSD: bytes already
  except (ImportError, OSError):
    return 0


def snapshot() -> Tuple[float, int, int]:
  """Return `(cpu_load, rss_bytes, peak_rss_bytes)` in one call.

  Called by the client at window close; the same tuple is stamped
  on every event in that window. Three probes back-to-back is cheap
  enough that we don't bother caching across windows.

  Coerces `peak >= rss`. On Linux, `/proc/self/statm` (used for
  current RSS) and `ru_maxrss` (used for peak) are updated at
  different instants by the kernel — under sustained allocation the
  current-RSS read can briefly exceed the stale peak read. The
  user-facing invariant ("peak is monotonic ≥ current") must hold
  in every emitted event, so we max() at the boundary rather than
  ship the kernel's racy view downstream.
  """
  cpu = cpu_load()
  rss = memory_rss_bytes()
  peak = memory_peak_bytes()
  return cpu, rss, max(peak, rss)
