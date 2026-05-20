# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for the `Sink` protocol + `TeeSink` fan-out.

Sinks are the "where do events go" axis. The Phase 0 refactor
introduced the protocol; this file pins down its contract:

  - Anything with `emit(event)` and `close()` is a Sink (duck-typed,
    `runtime_checkable`).
  - `JsonlFileSink` is the legacy `JsonlWriter` and conforms unchanged.
  - `TeeSink` fans out and isolates failures — one bad child cannot
    take down the rest.
  - `Client.config(sink=...)` accepts any Sink and skips building a
    default `JsonlFileSink`.
"""

import json

import pytest

import driftdockerprofiler
from driftdockerprofiler import Client, JsonlFileSink, JsonlWriter, Sink, TeeSink


# ----------------------------------------------------------------- protocol

def test_jsonl_writer_implements_sink_protocol():
    """Legacy `JsonlWriter` must satisfy the new protocol unchanged."""
    w = JsonlWriter('/tmp/drift-test-sink.jsonl')
    try:
        assert isinstance(w, Sink)
    finally:
        w.close()


def test_jsonl_file_sink_is_jsonl_writer_alias():
    """`JsonlFileSink` is just `JsonlWriter` under the sink-layer name —
    same class, so existing code (and tests) that inspect
    `.path` / `.emitted` continue to work."""
    assert JsonlFileSink is JsonlWriter


def test_plain_duck_typed_sink_is_recognized():
    """Anything with `emit(event)` + `close()` qualifies — no subclass needed."""
    class CapturingSink:
        def __init__(self):
            self.events = []
        def emit(self, event): self.events.append(event)
        def close(self): pass

    assert isinstance(CapturingSink(), Sink)


# ----------------------------------------------------------------- TeeSink

def test_tee_fans_out_to_every_child():
    a, b = _Capture(), _Capture()
    tee = TeeSink([a, b])
    tee.emit({'type': 'test', 'n': 1})
    tee.emit({'type': 'test', 'n': 2})
    assert a.events == b.events == [{'type': 'test', 'n': 1},
                                    {'type': 'test', 'n': 2}]


def test_tee_isolates_failure_one_bad_child_doesnt_break_others():
    """A broken sink (Supabase outage, full disk) must not silence the rest."""
    broken = _Broken()
    good = _Capture()
    tee = TeeSink([broken, good])
    tee.emit({'type': 'test', 'n': 1})
    # Good sink still got the event despite broken sink raising.
    assert good.events == [{'type': 'test', 'n': 1}]


def test_tee_close_calls_every_child():
    a, b = _Capture(), _Capture()
    TeeSink([a, b]).close()
    assert a.closed and b.closed


def test_tee_rejects_non_sink_children():
    with pytest.raises(TypeError, match='Sink protocol'):
        TeeSink([object()])


# ----------------------------------------------------------------- client wiring

def test_client_accepts_explicit_sink(tmp_path):
    """`Client.config(sink=...)` skips the default `JsonlFileSink` build."""
    captured = _Capture()
    c = Client()
    c.config(
        service='t',
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
        sink=captured,
    )
    # Internal attribute is `_writer` for back-compat; the injected
    # sink lands there.
    assert c._writer is captured


def test_client_rejects_non_sink_object():
    c = Client()
    with pytest.raises(TypeError, match='Sink protocol'):
        c.config(
            service='t',
            period_ms=10,
            duration_ms=200,
            disable_cpu_profiling=True,
            sink=object(),
        )


def test_client_default_sink_is_jsonl_file_sink(tmp_path):
    """Omitting `sink=` reproduces legacy behaviour — JsonlFileSink built
    from `output_path`."""
    out = tmp_path / 'events.jsonl'
    c = Client()
    c.config(
        service='t',
        output_path=str(out),
        period_ms=10,
        duration_ms=200,
        disable_cpu_profiling=True,
    )
    assert isinstance(c._writer, JsonlFileSink)
    assert c._writer.path == str(out)
    c._writer.close()


# ----------------------------------------------------------------- helpers

class _Capture:
    """Minimal Sink: records every event, never raises."""
    def __init__(self):
        self.events = []
        self.closed = False
    def emit(self, event): self.events.append(event)
    def close(self): self.closed = True


class _Broken:
    """Sink that raises on every emit — for failure-isolation tests."""
    def emit(self, event): raise RuntimeError('boom')
    def close(self): pass
