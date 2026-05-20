# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Pluggable event sinks.

A *sink* is where an event goes after the client builds it: a file on
disk (`JsonlFileSink`, wrapping the legacy `JsonlWriter`), a WebSocket
broadcast (`SupabaseRealtimeSink`), or a fan-out of multiple sinks
(`TeeSink`). The `Client` does not know or care which one it holds —
it just calls `sink.emit(event)` / `sink.close()`.

The split exists so that *how* we observe (sampler) and *where* events
go (sink) are independent axes. Adding a new sink is one new class
that implements the `Sink` protocol; no other file changes.
"""

from driftdockerprofiler.sinks.base import Sink, TeeSink
from driftdockerprofiler.sinks.jsonl_file import JsonlFileSink
from driftdockerprofiler.sinks.supabase import SupabaseRealtimeSink

__all__ = ['JsonlFileSink', 'Sink', 'SupabaseRealtimeSink', 'TeeSink']
