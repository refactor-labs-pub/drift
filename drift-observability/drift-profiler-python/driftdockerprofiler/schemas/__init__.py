# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""JSON Schema (Draft 2020-12) for every event driftdockerprofiler emits.

The on-disk schema lives at `schemas/event.schema.json` next to this
module and ships inside the wheel via `package_data` in setup.py.
This module is the loader / public Python API.

Usage::

    import driftdockerprofiler

    # Full union schema (oneOf over all four event types).
    schema = driftdockerprofiler.event_schema()

    # Just one variant (for tools that want a single object schema).
    schema = driftdockerprofiler.schema_for("cpu_trace")

    # Validate an event you read off the wire. `jsonschema` is an
    # optional dependency — if it's installed, `validate()` raises
    # `jsonschema.ValidationError` on mismatch; if not, it falls
    # back to a tiny built-in check (presence + type of required
    # top-level fields).
    driftdockerprofiler.validate_event(json.loads(line))

The schema files are LOADED LAZILY and cached so repeated calls are
cheap. Cached dicts are deep-copied on return when `copy=True` (the
default) so callers can mutate the result without poisoning the cache.
"""

from __future__ import annotations

import copy as _copy_mod
import json
import os
from typing import Any, Dict, List, Optional

# --------------------------------------------------------------- file paths

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCHEMA_FILE = os.path.join(_HERE, "event.schema.json")

# Map each `type` discriminator value to the corresponding $defs key.
_TYPE_TO_DEF = {
    "wall_trace":       "WallTraceEvent",
    "cpu_trace":        "CPUTraceEvent",
    "function_call":    "FunctionCallEvent",
    "wall_profile":     "WallProfileEvent",
    "cpu_profile":      "CPUProfileEvent",
    "profile_metadata": "ProfileMetadataEvent",
}

VALID_EVENT_TYPES = frozenset(_TYPE_TO_DEF)

# --------------------------------------------------------------- cache

_cached_schema: Optional[Dict[str, Any]] = None


def _load_uncached() -> Dict[str, Any]:
  """Read event.schema.json from disk. Single source of truth."""
  with open(_SCHEMA_FILE, "rb") as f:
    return json.load(f)


def schema_path() -> str:
  """Absolute path to event.schema.json inside the installed wheel."""
  return _SCHEMA_FILE


def event_schema(copy: bool = True) -> Dict[str, Any]:
  """Return the full union schema (oneOf over all four event types).

  Args:
    copy: When True (default) return a deep copy so callers can mutate
      it freely. Set False if you're feeding it to a validator and
      never mutating — saves ~50 µs per call.
  """
  global _cached_schema
  if _cached_schema is None:
    _cached_schema = _load_uncached()
  return _copy_mod.deepcopy(_cached_schema) if copy else _cached_schema


def schema_for(event_type: str, copy: bool = True) -> Dict[str, Any]:
  """Return the schema for one event-type discriminator value.

  Args:
    event_type: One of `'wall_trace'`, `'cpu_trace'`, `'wall_profile'`,
      `'cpu_profile'`.

  Raises:
    ValueError: when `event_type` is not a recognised discriminator.
  """
  if event_type not in _TYPE_TO_DEF:
    raise ValueError(
        f"unknown event type {event_type!r}; "
        f"expected one of {sorted(_TYPE_TO_DEF)}"
    )
  full = event_schema(copy=copy)
  def_name = _TYPE_TO_DEF[event_type]
  # Build a self-contained sub-schema: keep $defs (the variant uses
  # allOf with refs back into them) but replace the top-level union
  # with a single $ref to the chosen variant. This way a consumer
  # can feed *just* this dict to jsonschema and it resolves cleanly.
  sub = {
      "$schema": full["$schema"],
      "$id": full["$id"] + "#/$defs/" + def_name,
      "title": full["$defs"][def_name].get("title", def_name),
      "description": full["$defs"][def_name].get("description", ""),
      "$defs": full["$defs"],
      "$ref": "#/$defs/" + def_name,
  }
  return sub


def all_schemas() -> Dict[str, Dict[str, Any]]:
  """Return `{discriminator: sub_schema}` for every event type. Handy
  for tools that want to publish per-type endpoints (e.g. dump one
  file per variant into a docs site)."""
  return {t: schema_for(t) for t in _TYPE_TO_DEF}


# --------------------------------------------------------------- validate

class ValidationError(ValueError):
  """Raised by `validate_event` when the event doesn't conform.

  When `jsonschema` is installed, the underlying
  `jsonschema.ValidationError` is wrapped so callers get one error
  type to catch regardless of whether the strict validator was
  available.
  """


# Required top-level keys for the lightweight built-in validator
# (used when `jsonschema` is not installed). Per event family — the
# sampler events share EventBase, the deterministic-tracer event has
# its own minimal shape.
_SAMPLER_BASE_REQUIRED = ("type", "time", "service", "pod",
                          "period_ns", "duration_ns", "cpu", "memory_bytes")
_PER_TRACE_REQUIRED = ("count", "frames")
_BUNDLE_REQUIRED = ("profile_type", "time_ns", "sample_type", "samples")
_FUNCTION_CALL_REQUIRED = ("type", "time", "service", "pod",
                           "qualname", "duration_ns", "status")
# profile_metadata is the per-run header event the Client emits at
# startup. It does NOT share EventBase (no period_ns / duration_ns /
# cpu / memory_bytes) — those only make sense for sampler events.
# Matches the `required` list in event.schema.json#ProfileMetadataEvent.
_PROFILE_METADATA_REQUIRED = ("type", "time", "service", "pod",
                              "mode", "schema_version", "service_id",
                              "generator")


def _builtin_validate(event: Any) -> None:
  """Minimal validator used when `jsonschema` isn't available.

  Checks presence + outer type of required fields. Does NOT validate
  every detail — for that, install `jsonschema`. Catches the common
  schema-drift mistakes (missing field, wrong top-level type).
  """
  if not isinstance(event, dict):
    raise ValidationError(f"event must be a dict, got {type(event).__name__}")
  t = event.get("type")
  if t not in VALID_EVENT_TYPES:
    raise ValidationError(
        f"event.type={t!r} not in {sorted(VALID_EVENT_TYPES)}")
  if t == "function_call":
    for k in _FUNCTION_CALL_REQUIRED:
      if k not in event:
        raise ValidationError(
            f"function_call event missing required field: {k!r}")
    return
  if t == "profile_metadata":
    # Per-run header — distinct shape from sampler events (no
    # period_ns / duration_ns / cpu / memory_bytes).
    for k in _PROFILE_METADATA_REQUIRED:
      if k not in event:
        raise ValidationError(
            f"profile_metadata event missing required field: {k!r}")
    return
  # Sampler events share EventBase.
  for k in _SAMPLER_BASE_REQUIRED:
    if k not in event:
      raise ValidationError(f"event missing required field: {k!r}")
  required = _PER_TRACE_REQUIRED if t.endswith("_trace") else _BUNDLE_REQUIRED
  for k in required:
    if k not in event:
      raise ValidationError(
          f"{t} event missing required field: {k!r}")


def validate_event(event: Any) -> None:
  """Validate one event against the schema.

  Uses `jsonschema` (Draft 2020-12) when installed for a thorough
  check; otherwise falls back to a small built-in validator that
  catches the common drift mistakes. Either way, raises
  `driftdockerprofiler.schemas.ValidationError` on mismatch.

  Idempotent / read-only — never mutates the input.
  """
  try:
    import jsonschema  # type: ignore
    from jsonschema import Draft202012Validator
  except ImportError:
    _builtin_validate(event)
    return

  schema = event_schema(copy=False)
  validator = Draft202012Validator(schema)
  errors: List[Any] = sorted(validator.iter_errors(event), key=lambda e: e.path)
  if errors:
    # Raise our own type so callers don't need to also import jsonschema.
    msg = "; ".join(f"{list(e.path) or '<root>'}: {e.message}" for e in errors[:5])
    raise ValidationError(msg)


__all__ = [
    "VALID_EVENT_TYPES",
    "ValidationError",
    "all_schemas",
    "event_schema",
    "schema_for",
    "schema_path",
    "validate_event",
]
