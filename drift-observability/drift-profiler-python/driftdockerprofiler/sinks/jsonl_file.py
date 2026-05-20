# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""JSONL-file sink — thin alias over the existing `JsonlWriter`.

`JsonlWriter` already provides `emit(event)` and `close()` with thread
safety and atexit-flush semantics. This module just re-exports it
under the new `JsonlFileSink` name so call sites in the new sink-aware
code can say `from driftdockerprofiler.sinks import JsonlFileSink`
without reaching into `writer.py`.

The legacy name `JsonlWriter` is kept exported from
`driftdockerprofiler.writer` for back-compat — existing tests and
external users that imported it directly continue to work unchanged.
"""

from driftdockerprofiler.writer import JsonlWriter as JsonlFileSink

__all__ = ['JsonlFileSink']
