# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for `driftdockerprofiler.schemas`.

Validates that:
  - The JSON Schema file ships with the package (file present on disk).
  - The loader API returns a well-formed Draft 2020-12 document.
  - Real events emitted by the agent validate against the schema.
  - Negative cases (missing fields, wrong type) are rejected.

When `jsonschema` is installed, the strict validator runs. Otherwise
the built-in fallback validator runs (less thorough but still useful).
"""

import json
import os

import pytest

import driftdockerprofiler as ddp
from driftdockerprofiler import schemas


# --------------------------------------------------------------- presence

def test_schema_file_ships_with_package():
    p = schemas.schema_path()
    assert os.path.isfile(p), f"schema file not on disk: {p}"
    assert p.endswith(os.path.join("schemas", "event.schema.json"))


def test_schema_loads_as_json():
    raw = open(schemas.schema_path()).read()
    obj = json.loads(raw)
    # Source of truth — Draft 2020-12 schema with $defs + oneOf.
    assert obj["$schema"].startswith("https://json-schema.org/draft/2020-12/")
    assert "oneOf" in obj
    assert "$defs" in obj
    assert {"WallTraceEvent", "CPUTraceEvent",
            "WallProfileEvent", "CPUProfileEvent"} <= set(obj["$defs"])


# --------------------------------------------------------------- public api

def test_event_schema_returns_union():
    """The union covers the four sampler events, the deterministic
    tracer's `function_call`, and the F2 `profile_metadata`
    session-header. Six variants total."""
    s = schemas.event_schema()
    assert s["oneOf"]
    assert len(s["oneOf"]) == 6


def test_event_schema_copy_is_deep():
    """Two calls return INDEPENDENT dicts so mutation doesn't poison cache."""
    a = schemas.event_schema()
    b = schemas.event_schema()
    a["mutated"] = True
    assert "mutated" not in b


def test_event_schema_copy_false_returns_cached_reference():
    """copy=False shares the cache; mutate-at-your-own-risk fast path."""
    a = schemas.event_schema(copy=False)
    b = schemas.event_schema(copy=False)
    assert a is b


def test_schema_for_each_event_type():
    # Discriminator → $defs name lookup table. Mirrors `_TYPE_TO_DEF`
    # in driftdockerprofiler.schemas.
    expected = {
        "wall_trace":       "WallTraceEvent",
        "cpu_trace":        "CPUTraceEvent",
        "wall_profile":     "WallProfileEvent",
        "cpu_profile":      "CPUProfileEvent",
        "function_call":    "FunctionCallEvent",
        "profile_metadata": "ProfileMetadataEvent",
    }
    assert set(expected) == set(schemas.VALID_EVENT_TYPES)
    for t, def_name in expected.items():
        sub = schemas.schema_for(t)
        assert "$ref" in sub
        assert sub["$ref"].endswith(def_name)


def test_schema_for_unknown_raises():
    with pytest.raises(ValueError, match="unknown event type"):
        schemas.schema_for("not_a_real_event")


def test_all_schemas_returns_one_per_type():
    s = schemas.all_schemas()
    assert set(s) == schemas.VALID_EVENT_TYPES


def test_public_api_reexports():
    """Top-level package surfaces the schema API for easy access."""
    for name in ("event_schema", "schema_for", "validate_event",
                 "schema_path", "ValidationError", "VALID_EVENT_TYPES"):
        assert hasattr(ddp, name), name


# --------------------------------------------------------------- validation

def _wall_trace_event():
    return {
        "type": "wall_trace",
        "time": "2026-05-19T12:34:50.012345Z",
        "service": "test-svc",
        "pod": "test-pod",
        "period_ns": 10_000_000,
        "duration_ns": 10_000_000_000,
        "count": 42,
        "cpu": 0.42,
        "memory_bytes": 87_654_321,
        "frames": [
            {"name": "create_order", "file": "/app/app.py", "line": 52},
            {"name": "app",          "file": "/lib/fastapi/routing.py", "line": 273},
        ],
    }


def _cpu_trace_event():
    e = _wall_trace_event()
    e["type"] = "cpu_trace"
    return e


def _wall_profile_event():
    return {
        "type": "wall_profile",
        "profile_type": "wall",
        "time": "2026-05-19T12:34:50.012345Z",
        "time_ns": 1779798890_012345_000,
        "service": "test-svc",
        "pod": "test-pod",
        "period_ns": 10_000_000,
        "duration_ns": 10_000_000_000,
        "cpu": 0.42,
        "memory_bytes": 87_654_321,
        "sample_type": [
            {"type": "sample", "unit": "count"},
            {"type": "wall",   "unit": "nanoseconds"},
        ],
        "samples": [
            {
                "count": 5,
                "value_ns": 50_000_000,
                "frames": [{"name": "f", "file": "/x.py", "line": 1}],
                "labels": {},
            }
        ],
    }


def test_valid_wall_trace_passes():
    schemas.validate_event(_wall_trace_event())


def test_valid_cpu_trace_passes():
    schemas.validate_event(_cpu_trace_event())


def test_valid_wall_profile_passes():
    schemas.validate_event(_wall_profile_event())


def test_missing_required_field_rejected():
    ev = _wall_trace_event()
    del ev["frames"]
    with pytest.raises(schemas.ValidationError):
        schemas.validate_event(ev)


def test_unknown_type_rejected():
    ev = _wall_trace_event()
    ev["type"] = "definitely_not_a_real_type"
    with pytest.raises(schemas.ValidationError):
        schemas.validate_event(ev)


def test_non_dict_event_rejected():
    with pytest.raises(schemas.ValidationError):
        schemas.validate_event("not a dict")


def test_validate_via_top_level_export():
    """driftdockerprofiler.validate_event is the same callable as
    driftdockerprofiler.schemas.validate_event."""
    assert ddp.validate_event is schemas.validate_event


# --------------------------------------------------------------- end-to-end

def test_real_agent_events_validate(tmp_path):
    """Boot a Client, generate some events, validate every line."""
    out = str(tmp_path / "events.jsonl")
    c = ddp.Client()
    c.config(
        service="schema-test",
        output_path=out,
        period_ms=5,
        duration_ms=200,
        disable_cpu_profiling=True,  # wall-only is enough; cpu needs C++ ext
    )
    c.start()
    try:
        # Burn ~0.4s of wall time so SIGALRM fires repeatedly.
        import time
        end = time.monotonic() + 0.4
        n = 0
        while time.monotonic() < end:
            n += 1
        assert n
    finally:
        c.stop()

    events = [json.loads(l) for l in open(out).read().splitlines() if l]
    assert events, "client produced no events"
    for e in events:
        schemas.validate_event(e)
