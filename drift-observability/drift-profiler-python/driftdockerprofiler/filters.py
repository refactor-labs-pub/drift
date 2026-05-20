# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Single source of truth for "what counts as profiler-internal noise".

Clean-architecture layer used by both the SAMPLER (which decides
which threads to walk in the first place) and the CLIENT (which
decides which already-produced traces to drop before they hit the
sink). All exclude / filter / skip decisions in the agent route
through this module so a rule change propagates to every call site at
once.

Layer diagram::

      sampler --calls--> filters.is_internal_thread_name(...)
        |                                ^
        v                                |
      traces                             |
        |                                |
        v                                |
      client.TraceFilter --uses-> filters constants + matches_any(...)
        |
        v
      sink

There are TWO checks because they answer two different questions:

  - `is_internal_thread_name` (thread-name): asked at sample time —
    "is this Python thread one we own?". The wall sampler skips its
    polling sibling thread, its sink-worker thread, and itself. Stops
    profiler-self noise at the source, where the only fact we have is
    the thread's identity (frames inside that thread are mostly
    blocked in stdlib — `threading.Event.wait`, `queue.Queue.get`,
    `socket.recv` — so a leaf-file check can't catch them).

  - `TraceFilter` (file-path): asked AFTER the trace is built —
    "should we drop this trace based on its leaf's file?". Catches:
      * the legacy SIGALRM `WallProfiler` which runs on the main
        thread and can fire mid-`@trace`-decorated user code that
        bottoms out in a profiler helper;
      * user-supplied `exclude_paths` extras (project-specific noise
        like `/sqlalchemy/`);
      * the STRICT preset for "only my code" mode.

Conventions enforced by this module:

  - Every `threading.Thread` created inside this package MUST be named
    with the `INTERNAL_THREAD_PREFIX` prefix. Today:
      - `driftdockerprofiler-<type>-poller`     (client.py)
      - `driftdockerprofiler-wall-all-threads`  (wall_all_threads.py)
      - `driftdockerprofiler-supabase-sink`     (sinks/supabase.py)
    A new internal thread that forgets the prefix leaks frames back
    into the emitted output.

  - Pattern lists are substring-matched against frame `file`
    attributes — cheap, same engine py-spy's `--exclude` and GCP's
    agent-skip filter use.
"""

# Prefix every profiler-internal `threading.Thread` is named with.
# Enforced at the Thread() call site, checked here at sample time.
INTERNAL_THREAD_PREFIX = 'driftdockerprofiler-'


# Exclude paths come in two layers, both substring-matched against
# each leaf frame's `file`:
#
#   1. BUILTIN_EXCLUDE_PATHS — minimal "always filter this" defaults.
#      Only covers pure noise: the profiler observing itself, plus
#      frozen importlib bootstrap. Override by passing
#      `builtin_exclude_paths=(...)` / `()` into `Client.config`.
#
#   2. user-supplied `exclude_paths` — ADDITIVE extras stacked on top
#      of the builtins. The common case is "use the defaults plus my
#      own patterns", e.g. `exclude_paths=('/sqlalchemy/',)`.
#
# A leaf matches if its file contains ANY substring in the combined
# list. The wall-sampler thread-name skip handles the *common* path
# (most profiler-self traces are in our own worker threads); this
# list is the safety net for the rest.
BUILTIN_EXCLUDE_PATHS = (
    # ---- The profiler observing itself ----------------------------- #
    # Every spelling we can encounter at runtime is listed so a path
    # variant (in-tree dev install vs. site-packages, normalized vs.
    # legacy wheel name) can't sneak past the filter.

    '/driftdockerprofiler/',          # the package directory itself
    '/_profiler.cpython',             # CPython-tagged .so filename
    '/_profiler.abi3',                # stable-ABI .so filename
    '_profiler.so',                   # bare .so (no version suffix)
    '/drift_docker_profiler-',        # `*.dist-info/` directory (PEP 503)
    '/drift-docker-profiler-',        # legacy non-normalized dist-info dir

    # ---- CPython internals ----------------------------------------- #
    # Frozen importlib bootstrap — appears as `<frozen
    # importlib._bootstrap>` / `<frozen importlib._bootstrap_external>`
    # / `<frozen runpy>` in tracebacks. Always noise; no actionable
    # user signal.
    '<frozen ',
    # `<built-in>` shows up for some C-implemented stdlib calls (e.g.
    # `<built-in method builtins.exec>`). Same story as <frozen >.
    '<built-in',
)


# Strictly-only-user-code preset. Users who want "show me ONLY my
# code" pass this (or augment it) via `exclude_paths=`. Kept out of
# the defaults because dropping stdlib + site-packages turns the
# icicle chart empty in apps that haven't decorated their hot methods
# with `@trace` — the sampler events all leaf in framework code.
#
# Usage::
#
#     driftdockerprofiler.start(
#         service='svc',
#         exclude_paths=driftdockerprofiler.STRICT_USER_CODE_EXCLUDE_PATHS,
#     )
STRICT_USER_CODE_EXCLUDE_PATHS = (
    '/lib/python3.',     # Python stdlib (catches /usr/lib, /usr/local/lib, venvs)
    '/site-packages/',   # third-party packages (fastapi, uvicorn, etc.)
)


# Combined "this is a system / runtime / profiler-self frame" set.
# Drives the Frame.is_system join-key label emitted in the JSONL wire
# format — i.e. *"would the agent-skip filter with the strict preset
# enabled drop this frame?"*. Single source of truth, never out of
# sync with the two lists above.
SYSTEM_PATH_PREFIXES = BUILTIN_EXCLUDE_PATHS + STRICT_USER_CODE_EXCLUDE_PATHS


# --------------------------------------------------------- pure helpers


def is_internal_thread_name(name):
  """True iff `name` is the name of a profiler-owned worker thread.

  Used by the wall sampler to skip its own polling / sink-worker
  threads BEFORE sampling them. The convention is enforced at every
  `threading.Thread(name=...)` call site inside this package.
  """
  return bool(name) and name.startswith(INTERNAL_THREAD_PREFIX)


def matches_any(file, patterns):
  """True iff `file` contains any substring in `patterns`.

  The one substring-match engine the module shares. Empty file /
  empty patterns / empty individual pattern strings all short-circuit
  cleanly (an empty file can't be in any pattern; an empty pattern
  list can't match anything; an empty pattern string would otherwise
  match everything).
  """
  if not file or not patterns:
    return False
  for pat in patterns:
    if pat and pat in file:
      return True
  return False


def is_system_frame(file):
  """True iff `file` looks like a system / stdlib / site-packages /
  profiler-self frame under the agent-skip rules.

  Pure function: takes a frame's file path and returns a bool. Used
  to label the wire-format `Frame.is_system` field at emit time —
  same predicate driving `SYSTEM_PATH_PREFIXES`. Called per frame,
  so it has to be cheap; the rule list is small and the match is
  a Python `in`-loop, well under a microsecond.
  """
  return matches_any(file, SYSTEM_PATH_PREFIXES)


# --------------------------------------------------------- TraceFilter


def _normalize_patterns(patterns):
  """Coerce any iterable of pattern values to a tuple of str.

  Defensive against:
    * `None` → `()`
    * a list (caller could mutate it after passing it in) → tuple copy
    * non-str values (a user accidentally passes a Path) → str()
  """
  if not patterns:
    return ()
  return tuple(str(p) for p in patterns)


class TraceFilter:
  """Drops traces whose leaf frame matches any configured pattern.

  Single-responsibility helper used by `Client._write_window`. The
  pattern list is OWNED here so the Client never has to think about
  it — config-time it constructs a `TraceFilter`, write-time it just
  calls `filter(traces)`.

  Filtering on the LEAF (frames[0]) is deliberate: it's "what code is
  executing now". Deeper frames may legitimately route through
  filtered paths on the way to user code; dropping those would lose
  signal.

  Profiler-owned-thread traces are NOT caught here — those are
  skipped earlier, at sample time, via `is_internal_thread_name`
  inside the wall sampler. This filter is the second-line drop for
  cases that path doesn't cover (legacy SIGALRM `WallProfiler`,
  user-supplied `exclude_paths`, the STRICT preset).
  """

  __slots__ = ('_patterns',)

  def __init__(self, builtin_patterns=BUILTIN_EXCLUDE_PATHS, extra_patterns=()):
    """Concat the two layers once at construction.

    Both args accept any iterable; we normalize to tuple-of-str so an
    accidental list mutation by the caller can't surprise us later.
    """
    self._patterns = _normalize_patterns(builtin_patterns) + _normalize_patterns(extra_patterns)

  @property
  def patterns(self):
    """The combined builtin + extra pattern tuple, post-normalization."""
    return self._patterns

  def should_exclude(self, leaf_file):
    """True iff a trace with this leaf-file should be dropped."""
    return matches_any(leaf_file, self._patterns)

  def filter(self, traces):
    """Return a new `{trace: count}` dict with matching leaves dropped.

    Pass-through (return input as-is) when no patterns are configured
    — avoids the dict copy for the disabled-filter case.
    """
    if not self._patterns:
      return traces
    out = {}
    for trace, count in traces.items():
      if not trace:
        continue
      leaf = trace[0]
      # trace[0] is a (name, file, line, ...) tuple.
      leaf_file = leaf[1] if len(leaf) > 1 else ''
      if self.should_exclude(leaf_file):
        continue
      out[trace] = count
    return out
