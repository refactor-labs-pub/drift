# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Sink protocol + fan-out helper.

Why a protocol instead of an ABC: `JsonlWriter` already exists in
`writer.py` and is widely referenced by tests under the name
`JsonlWriter`. Forcing it to subclass an ABC would require either a
rename or a multiple-inheritance hack. A `typing.Protocol` with
`runtime_checkable` lets the existing class participate without any
modification — pure duck typing with type-checker support.

Every sink MUST be safe to call from any thread. Implementations are
responsible for their own locking; the `Client` calls `emit()` from the
polling thread, but `@trace`-decorated functions may call it from any
thread.

Sinks MUST swallow transient errors. A sink failure (full disk,
dropped WebSocket, expired token) must never propagate into user code.
The reference behaviour is `JsonlWriter` — see writer.py.
"""

import logging
# `Protocol`/`runtime_checkable` shim — see sampling/base.py for why.
from typing import Any, Dict, Iterable
try:
    from typing import Protocol, runtime_checkable
except ImportError:  # Python 3.7
    from typing_extensions import Protocol, runtime_checkable

logger = logging.getLogger(__name__)


@runtime_checkable
class Sink(Protocol):
    """Where events end up. Any object with these two methods qualifies."""

    def emit(self, event: Dict[str, Any]) -> None:
        """Send one event. Must not raise into the caller.

        `event` is a plain dict ready for JSON serialization (no
        Python-specific types). `time` may be either an int
        (nanoseconds since epoch) or an ISO-8601 string; the sink may
        rewrite ints to ISO if its wire format requires.
        """
        ...

    def close(self) -> None:
        """Release resources. Idempotent."""
        ...


class TeeSink:
    """Fan-out: forward every event to every child sink.

    A failure in one child does not stop the others — each child's
    `emit()` is wrapped in a try/except so a broken Supabase connection
    can't take down JSONL logging (or vice versa). The exception is
    logged at WARNING and dropped.

    Order of `sinks` is the order children see each event. Useful when
    one sink is "primary" (must succeed if it can) and another is
    "secondary" / best-effort.
    """

    def __init__(self, sinks: Iterable[Sink]):
        self._sinks = tuple(sinks)
        for s in self._sinks:
            if not isinstance(s, Sink):
                raise TypeError(
                    "TeeSink child %r does not implement the Sink protocol "
                    "(needs `emit(event)` and `close()`)" % (s,))

    @property
    def sinks(self):
        return self._sinks

    def emit(self, event: Dict[str, Any]) -> None:
        for sink in self._sinks:
            try:
                sink.emit(event)
            except Exception:  # pylint: disable=broad-except
                # Match JsonlWriter's "never let I/O break the caller"
                # contract. One sink's failure cannot starve the rest.
                logger.warning(
                    'TeeSink child %r failed on emit; continuing',
                    type(sink).__name__, exc_info=True)

    def close(self) -> None:
        for sink in self._sinks:
            try:
                sink.close()
            except Exception:  # pylint: disable=broad-except
                logger.warning(
                    'TeeSink child %r failed on close; continuing',
                    type(sink).__name__, exc_info=True)
