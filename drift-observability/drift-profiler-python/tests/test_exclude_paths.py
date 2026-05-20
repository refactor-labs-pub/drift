# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for the two-layer exclude filter in `Client`.

The filter drops a trace iff its LEAF frame's `file` contains any of
the configured substrings. The configured list is
`builtin_exclude_paths + exclude_paths` — the builtins are the
"baked-in" defaults (profiler self + Python stdlib + frozen +
site-packages), and `exclude_paths` is additive user-supplied extras.
"""

import sys

import pytest

from driftdockerprofiler.client import (
    BUILTIN_EXCLUDE_PATHS,
    STRICT_USER_CODE_EXCLUDE_PATHS,
    Client,
    _is_system_frame,
)

pytestmark = pytest.mark.skipif(
    not (sys.platform.startswith("linux") or sys.platform.startswith("darwin")),
    reason="client uses POSIX signals; skipped on non-POSIX",
)


def _configured(**overrides):
    """Build + configure a Client without starting it. Tests poke at the
    private _filter_traces / _should_exclude helpers directly so they
    don't need a live SIGALRM loop."""
    c = Client()
    cfg = dict(
        service="t",
        output_path="/tmp/drift-test-exclude-paths.jsonl",
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
    )
    cfg.update(overrides)
    c.config(**cfg)
    return c


# ----------------------------------------------------------------- BUILTIN defaults

def test_builtins_cover_profiler_self():
    assert any("driftdockerprofiler" in p for p in BUILTIN_EXCLUDE_PATHS)
    assert any("_profiler.cpython" in p for p in BUILTIN_EXCLUDE_PATHS)


def test_builtins_cover_frozen_importlib():
    """Frozen bootstrap modules appear as `<frozen importlib._bootstrap>`."""
    assert any("<frozen " in p for p in BUILTIN_EXCLUDE_PATHS)


def test_builtins_DO_NOT_cover_stdlib_by_default():
    """Stdlib leaves are kept by default — asyncio / threading runners
    are useful context. Users wanting to drop them add
    STRICT_USER_CODE_EXCLUDE_PATHS to `exclude_paths`."""
    assert not any("/lib/python3." in p for p in BUILTIN_EXCLUDE_PATHS)


def test_builtins_DO_NOT_cover_site_packages_by_default():
    """Third-party leaves (fastapi, uvicorn) are kept by default for
    the same reason as stdlib — they're meaningful framework context."""
    assert not any("/site-packages/" in p for p in BUILTIN_EXCLUDE_PATHS)


def test_strict_preset_covers_stdlib_and_site_packages():
    """The "only my code" preset adds stdlib + site-packages on top."""
    assert any("/lib/python3." in p for p in STRICT_USER_CODE_EXCLUDE_PATHS)
    assert any("/site-packages/" in p for p in STRICT_USER_CODE_EXCLUDE_PATHS)


# ----------------------------------------------------------------- _should_exclude

def test_should_exclude_matches_profiler_self():
    c = _configured()
    assert c._should_exclude(
        "/usr/lib/python3.11/site-packages/driftdockerprofiler/cpu_profiler.py")
    assert c._should_exclude("/opt/driftdockerprofiler/_profiler.cpython-311.so")


def test_should_exclude_matches_frozen():
    c = _configured()
    assert c._should_exclude("<frozen importlib._bootstrap>")
    assert c._should_exclude("<frozen importlib._bootstrap_external>")


def test_should_exclude_default_keeps_stdlib():
    """asyncio / threading appear as leaves under default config — they
    pass through. Users opt INTO dropping them via the strict preset."""
    c = _configured()
    assert not c._should_exclude("/usr/lib/python3.11/asyncio/runners.py")
    assert not c._should_exclude("/usr/local/lib/python3.7/threading.py")


def test_should_exclude_default_keeps_site_packages():
    c = _configured()
    assert not c._should_exclude("/usr/local/lib/python3.7/site-packages/uvicorn/server.py")
    assert not c._should_exclude("/venv/lib/python3.11/site-packages/fastapi/routing.py")


def test_should_exclude_with_strict_preset_drops_stdlib_and_site_packages():
    """Opting INTO the strict preset via exclude_paths gets aggressive filtering."""
    c = _configured(exclude_paths=STRICT_USER_CODE_EXCLUDE_PATHS)
    assert c._should_exclude("/usr/lib/python3.11/asyncio/runners.py")
    assert c._should_exclude("/usr/local/lib/python3.7/site-packages/uvicorn/server.py")
    # Still keeps user code.
    assert not c._should_exclude("/app/orders.py")


def test_should_exclude_does_not_match_user_code():
    """`/app/` and similar app-root patterns must pass through."""
    c = _configured()
    assert not c._should_exclude("/app/orders.py")
    assert not c._should_exclude("/app/app.py")
    assert not c._should_exclude("/srv/myproject/handlers.py")


# ----------------------------------------------------------------- _filter_traces

def test_filter_drops_profiler_self_leaf():
    c = _configured()
    profiler_self = (
        ("profile", "/usr/local/lib/python3.7/site-packages/driftdockerprofiler/cpu_profiler.py", 42),
        ("_poll_profiler_service", "/usr/local/lib/python3.7/site-packages/driftdockerprofiler/client.py", 350),
    )
    user_code = (
        ("create", "/app/orders.py", 14),
        ("create_order", "/app/app.py", 83),
    )
    traces = {profiler_self: 5, user_code: 3}
    filtered = c._filter_traces(traces)
    assert profiler_self not in filtered
    assert user_code in filtered
    assert filtered[user_code] == 3


def test_filter_default_keeps_stdlib_and_site_packages():
    """Default config keeps asyncio / fastapi leaves — useful context.
    Frozen bootstrap is the only "framework" thing the default drops."""
    c = _configured()
    asyncio_leaf = (("run", "/usr/local/lib/python3.7/asyncio/runners.py", 43),)
    fastapi_leaf = (("__call__", "/usr/local/lib/python3.7/site-packages/fastapi/routing.py", 273),)
    frozen_leaf = (("_call_with_frames_removed", "<frozen importlib._bootstrap>", 219),)
    user_trace = (("create", "/app/orders.py", 14),)
    traces = {asyncio_leaf: 1, fastapi_leaf: 2, frozen_leaf: 3, user_trace: 99}
    filtered = c._filter_traces(traces)
    # Frozen dropped; asyncio + fastapi + user code survive.
    assert asyncio_leaf in filtered
    assert fastapi_leaf in filtered
    assert frozen_leaf not in filtered
    assert user_trace in filtered


def test_filter_with_strict_preset_drops_framework_leaves():
    """When the user opts into the strict preset via exclude_paths,
    stdlib + site-packages leaves get dropped."""
    c = _configured(exclude_paths=STRICT_USER_CODE_EXCLUDE_PATHS)
    framework_leaves = [
        ("run", "/usr/local/lib/python3.7/asyncio/runners.py", 43),
        ("__call__", "/usr/local/lib/python3.7/site-packages/fastapi/routing.py", 273),
    ]
    traces = {(leaf,): i + 1 for i, leaf in enumerate(framework_leaves)}
    user_trace = (("create", "/app/orders.py", 14),)
    traces[user_trace] = 99
    filtered = c._filter_traces(traces)
    assert len(filtered) == 1
    assert user_trace in filtered


def test_filter_keeps_everything_when_builtins_disabled_and_no_extras():
    """`builtin_exclude_paths=()` + `exclude_paths=()` → no filtering."""
    c = _configured(builtin_exclude_paths=(), exclude_paths=())
    profiler_self = (
        ("profile", "/usr/local/lib/python3.7/site-packages/driftdockerprofiler/cpu_profiler.py", 42),
    )
    traces = {profiler_self: 5}
    filtered = c._filter_traces(traces)
    assert profiler_self in filtered


def test_filter_only_inspects_leaf_frame():
    """User-code leaf with profiler frames DEEPER in the stack is kept."""
    c = _configured()
    mixed = (
        ("create", "/app/orders.py", 14),  # leaf — user code
        ("some_helper", "/usr/lib/python3.11/site-packages/driftdockerprofiler/x.py", 1),
    )
    traces = {mixed: 1}
    filtered = c._filter_traces(traces)
    assert mixed in filtered


# ----------------------------------------------------------------- two-knob composition

def test_user_extras_stack_on_top_of_builtins():
    """`exclude_paths` is ADDITIVE — both builtins and user extras apply."""
    c = _configured(exclude_paths=("/myorm/",))
    # Default builtins still in force (frozen):
    frozen_leaf = (("ex", "<frozen importlib._bootstrap>", 219),)
    # Stdlib + site-packages now PASS THROUGH by default:
    asyncio_leaf = (("run", "/usr/local/lib/python3.7/asyncio/runners.py", 43),)
    site_pkg_leaf = (("x", "/usr/local/lib/python3.7/site-packages/foo.py", 1),)
    # User extra in force:
    orm_leaf = (("query", "/app/myorm/engine.py", 99),)
    user_leaf = (("create", "/app/orders.py", 14),)

    traces = {frozen_leaf: 1, asyncio_leaf: 1, site_pkg_leaf: 1,
              orm_leaf: 1, user_leaf: 1}
    filtered = c._filter_traces(traces)
    assert frozen_leaf not in filtered       # dropped by builtins (frozen)
    assert orm_leaf not in filtered          # dropped by user extras
    assert asyncio_leaf in filtered          # stdlib survives by default
    assert site_pkg_leaf in filtered         # site-packages survives by default
    assert user_leaf in filtered             # user code always survives
    assert user_leaf in filtered             # passes both filters


def test_user_can_override_builtins_to_keep_stdlib():
    """Custom `builtin_exclude_paths=()` disables the default filter so
    stdlib leaves come back through; user extras still apply."""
    c = _configured(builtin_exclude_paths=(), exclude_paths=("/driftdockerprofiler/",))
    asyncio_leaf = (("run", "/usr/local/lib/python3.7/asyncio/runners.py", 43),)
    profiler_self = (
        ("profile", "/usr/local/lib/python3.7/site-packages/driftdockerprofiler/cpu_profiler.py", 42),
    )
    traces = {asyncio_leaf: 1, profiler_self: 1}
    filtered = c._filter_traces(traces)
    assert asyncio_leaf in filtered          # builtins disabled, asyncio survives
    assert profiler_self not in filtered     # user-extra still hides profiler self


# ----------------------------------------------------------------- F1a: is_system label

def test_is_system_frame_matches_profiler_self():
    """The Google-practice rule for `Frame.is_system` is the union of
    BUILTIN_EXCLUDE_PATHS + STRICT_USER_CODE_EXCLUDE_PATHS. Profiler
    internals (`/driftdockerprofiler/`, the compiled `.so`, frozen
    importlib) all classify as system."""
    assert _is_system_frame(
        "/usr/local/lib/python3.11/site-packages/driftdockerprofiler/cpu_profiler.py")
    assert _is_system_frame("/opt/driftdockerprofiler/_profiler.cpython-311.so")
    assert _is_system_frame("<frozen importlib._bootstrap>")


def test_is_system_frame_matches_stdlib_and_site_packages():
    """STRICT_USER_CODE preset (stdlib + site-packages) also flags
    `is_system=True`. This is what makes the label useful: the viewer
    can dim every stdlib/3p frame regardless of where in the stack it
    sits, without re-running the filter."""
    assert _is_system_frame("/usr/lib/python3.11/asyncio/runners.py")
    assert _is_system_frame(
        "/usr/local/lib/python3.7/site-packages/uvicorn/server.py")
    assert _is_system_frame(
        "/venv/lib/python3.11/site-packages/fastapi/routing.py")


def test_is_system_frame_returns_false_for_user_code():
    """`/app/` and similar app-root paths must be `is_system=False`
    — that's exactly the half of the binary tree the viewer wants
    to highlight."""
    assert not _is_system_frame("/app/orders.py")
    assert not _is_system_frame("/app/app.py")
    assert not _is_system_frame("/srv/myproject/handlers.py")


def test_is_system_frame_handles_edge_cases():
    """Empty / missing file paths shouldn't crash. Falsy file → False
    (defensive default; a missing file path can't be classified, so
    we'd rather under-label than crash)."""
    assert not _is_system_frame("")
    assert not _is_system_frame(None)  # tracer paths sometimes lack file
