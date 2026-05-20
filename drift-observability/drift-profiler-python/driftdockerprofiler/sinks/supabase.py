# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Supabase Realtime sink — stream events over WSS instead of (or
alongside) a local JSONL file.

Wire protocol
-------------
Supabase Realtime is Phoenix Channels v1.0.0. Every message is a JSON
object::

    {"topic": "...", "event": "...", "payload": {...},
     "ref": "<n>", "join_ref": "<n>"}

Connection URL::

    wss://<host>/realtime/v1/websocket?apikey=<JWT>&vsn=1.0.0

`<host>` is the hostname from the Supabase project URL (e.g.
`abc123.supabase.co`). The `apikey` is a JWT — anon key for public
channels, service-role key for private channels.

The sink joins ONE channel (`realtime:<channel>`) at startup, sends
every event as a `broadcast`, and replies to nothing (we don't
subscribe). One heartbeat every 25 s on the `phoenix` topic keeps the
socket alive — server times out around 60 s of silence.

Design notes
------------
1. **Sync WebSocket lib (`websocket-client`).** The profiler's sampler
   threads cannot drive an asyncio loop; they call `sink.emit()`
   synchronously. A sync library lets us put one blocking
   `send`/`recv` per iteration in a dedicated worker thread without
   contaminating the rest of the codebase with `await`.

2. **Bounded `queue.Queue` between caller and worker.** `emit()` is
   non-blocking — `put_nowait()` drops on full. A network blip, a
   server timeout, or an expired token MUST NOT propagate into the
   user's request handler. The producer pays one constant-time hash +
   enqueue; everything else happens off the hot path.

3. **No backpressure to the caller, ever.** The trade-off is that
   under sustained network failure, events are dropped (with a
   counter). This is the same contract `JsonlWriter` provides
   (`emit()` swallows OSError) — sinks isolate failure from the app.

4. **Reconnect with exponential backoff.** `realtime-js` uses
   `[1, 2, 5, 10]` second steps capped at 10 s; we mirror that.

5. **Optional dependency.** `websocket-client` is NOT pulled into the
   profiler's main wheel — it's imported lazily at construction
   time so users who never touch Supabase keep zero new deps.
"""

import itertools
import json
import logging
import os
import queue
import threading
import time
from typing import Any, Dict, Optional
from urllib.parse import urlparse

logger = logging.getLogger(__name__)

# Phoenix Channels protocol version. v1.0.0 is the named-field
# envelope; v2.0.0 is positional arrays. Both `realtime-js` and
# `realtime-py` default to v1.0.0 — easier to debug on the wire.
_VSN = '1.0.0'

# Server times out around 60 s of silence. realtime-js sends a
# heartbeat every 30 s; we go a touch earlier for safety.
_HEARTBEAT_SEC = 25.0

# Backoff schedule for reconnect attempts. Caps at 10 s — matches
# realtime-js. Each failed connect bumps us up one step; success
# resets to 0.
_BACKOFF_STEPS = (1.0, 2.0, 5.0, 10.0)

# Default channel name when neither kwarg nor env var supplies one.
_DEFAULT_CHANNEL = 'drift-profiler-events'

# Env vars the auto-wiring path reads.
_ENV_URL = 'SUPABASE_URL'
_ENV_KEY = 'SUPABASE_REALTIME_API_KEY'
_ENV_CHANNEL = 'SUPABASE_REALTIME_CHANNEL'


def _build_wss_url(supabase_url: str, api_key: str) -> str:
    """Convert `https://<host>` (the standard `SUPABASE_URL` shape)
    into the Realtime WSS URL with `apikey` and `vsn` query params.

    Tolerates inputs that already use wss:// or include a path.
    """
    parsed = urlparse(supabase_url)
    if parsed.scheme not in ('http', 'https', 'ws', 'wss'):
        raise ValueError(
            'supabase_url must start with http(s):// or ws(s)://, got %r'
            % (supabase_url,))
    # Force WSS — Realtime always runs on TLS.
    host = parsed.netloc or parsed.path  # support bare-host strings too
    return ('wss://%s/realtime/v1/websocket?apikey=%s&vsn=%s'
            % (host, api_key, _VSN))


class SupabaseRealtimeSink:
    """Phoenix Channels broadcast sink. Same interface as `JsonlWriter`.

    Args:
      supabase_url: Standard Supabase project URL,
        e.g. ``https://abc123.supabase.co``. Also accepts a bare host
        or a ``wss://`` URL.
      api_key: JWT used for both socket auth (URL param) and channel
        authorization (`access_token` in the join payload). Anon key
        for public channels, service-role key for private ones.
      channel: Name of the realtime channel. Joined as
        ``realtime:<channel>``. Channels are ad-hoc — no DB row
        required. Defaults to ``drift-profiler-events``.
      event_name: Inner ``payload.event`` field on every broadcast.
        Lets a subscriber filter by event name. Defaults to
        ``profiler-event``.
      max_queue: Max events buffered between `emit()` and the worker
        thread. Once full, new events are dropped (counted via
        `dropped` property). 10_000 is enough to ride out a ~2 min
        network blip at 100 events/s.
      websocket_factory: Override for tests — returns an object with
        `send(str)`, `recv()`, `close()` methods. Production code
        leaves this `None` and we use `websocket-client`'s
        `create_connection`.
    """

    def __init__(self,
                 supabase_url: str,
                 api_key: str,
                 channel: str = _DEFAULT_CHANNEL,
                 event_name: str = 'profiler-event',
                 max_queue: int = 10_000,
                 connect_timeout: float = 10.0,
                 websocket_factory=None):
        if not supabase_url:
            raise ValueError('supabase_url is required')
        if not api_key:
            raise ValueError('api_key is required')

        self._wss_url = _build_wss_url(supabase_url, api_key)
        self._api_key = api_key
        self._topic = 'realtime:%s' % (channel,)
        self._event_name = event_name
        self._connect_timeout = connect_timeout
        self._websocket_factory = (
            websocket_factory or _default_websocket_factory)

        self._queue: queue.Queue = queue.Queue(maxsize=max_queue)
        self._stop = threading.Event()
        self._ref_counter = itertools.count(1)
        self._dropped = 0
        self._emitted = 0
        # The worker thread is what owns the WebSocket. Calling code
        # never touches it directly. Daemon=True so a forgotten
        # close() doesn't pin the interpreter.
        self._worker = threading.Thread(
            target=self._run,
            name='driftdockerprofiler-supabase-sink',
            daemon=True,
        )
        self._worker.start()

    # ----------------------------------------------------------- public

    @property
    def dropped(self) -> int:
        """Count of events `emit()` had to drop because the queue
        was full. Stays 0 unless the network is broken / lagging."""
        return self._dropped

    @property
    def emitted(self) -> int:
        """Count of events successfully written to the WebSocket."""
        return self._emitted

    @property
    def url(self) -> str:
        """For tests / logging. The full wss:// URL we connect to —
        do NOT log this in production, it contains the API key."""
        return self._wss_url

    @property
    def safe_url(self) -> str:
        """Same as `url` but with the ``apikey`` query param redacted.
        Safe to print to logs / stdout."""
        parsed = urlparse(self._wss_url)
        pairs = []
        for pair in (parsed.query or '').split('&'):
            if pair.startswith('apikey='):
                pairs.append('apikey=<redacted>')
            else:
                pairs.append(pair)
        return '%s://%s%s?%s' % (parsed.scheme, parsed.netloc,
                                 parsed.path, '&'.join(pairs))

    def emit(self, event: Dict[str, Any]) -> None:
        """Hand the event to the worker thread. Never blocks, never
        raises. If the queue is full, drops the event and bumps
        `dropped`.
        """
        try:
            self._queue.put_nowait(event)
        except queue.Full:
            self._dropped += 1
        except Exception:  # pragma: no cover - defensive
            # Belt-and-suspenders: the contract is "emit never raises
            # into the caller", and we mean it.
            self._dropped += 1

    def flush(self, timeout: float = 10.0) -> bool:
        """Block until every queued event has been processed by the
        worker (sent over WSS, or dropped due to a send error), or
        ``timeout`` elapses.

        Returns True if the queue drained in time, False otherwise.

        The contract is "events I `emit()`'d before this call are
        either confirmed sent or accounted for in `dropped`/send
        errors by the time this returns". Use before `close()` if you
        care about not losing in-flight events on shutdown.
        """
        deadline = time.monotonic() + timeout
        # Queue.unfinished_tasks decrements on task_done() (which the
        # worker calls in a `finally` block after each get). Polling
        # avoids the no-timeout limitation of Queue.join().
        while self._queue.unfinished_tasks > 0:
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.05)
        return True

    def close(self, timeout: float = 5.0) -> None:
        """Drain pending events, then signal the worker to shut down.
        Idempotent.

        `timeout` bounds BOTH the drain wait and the worker join — so
        worst case `close()` blocks ~2x `timeout`.
        """
        if self._stop.is_set():
            return
        # Drain first so in-flight events aren't dropped on the floor
        # when we yank the worker.
        self.flush(timeout=timeout)
        self._stop.set()
        if self._worker.is_alive():
            self._worker.join(timeout=timeout)

    # ----------------------------------------------------------- worker thread

    def _next_ref(self) -> str:
        return str(next(self._ref_counter))

    def _run(self) -> None:
        """Outer loop: connect → join → drain. On any exception,
        sleep with exponential backoff and reconnect."""
        backoff_idx = 0
        while not self._stop.is_set():
            try:
                self._connect_and_drain()
                # Clean exit (stop requested) — leave the loop.
                return
            except Exception as e:  # pylint: disable=broad-except
                logger.warning(
                    'SupabaseRealtimeSink: %s; reconnecting after %.1fs',
                    e, _BACKOFF_STEPS[backoff_idx])
                if self._stop.wait(_BACKOFF_STEPS[backoff_idx]):
                    return
                backoff_idx = min(backoff_idx + 1,
                                  len(_BACKOFF_STEPS) - 1)

    def _connect_and_drain(self) -> None:
        """One connect-and-serve cycle. Returns cleanly when stop is
        set; raises on any I/O error so the outer loop can back off."""
        ws = self._websocket_factory(
            self._wss_url, timeout=self._connect_timeout)
        try:
            join_ref = self._next_ref()
            self._send_join(ws, join_ref)
            # Spec says wait for phx_reply status=ok before considering
            # the channel joined. Skipping per-reply parsing — if the
            # server rejects, our next send will raise and trigger
            # reconnect. Adequate for fire-and-forget broadcast.
            last_heartbeat = time.monotonic()
            # Short recv timeout so the worker can check _stop frequently.
            try:
                ws.settimeout(0.25)
            except Exception:  # pragma: no cover - test doubles may omit
                pass
            while not self._stop.is_set():
                # Heartbeat first — keeps the socket alive even when
                # the event queue is empty.
                now = time.monotonic()
                if now - last_heartbeat >= _HEARTBEAT_SEC:
                    self._send_heartbeat(ws)
                    last_heartbeat = now
                try:
                    event = self._queue.get(timeout=0.25)
                except queue.Empty:
                    continue
                # task_done() in `finally` so a send failure still
                # decrements unfinished_tasks — otherwise `flush()`
                # would hang waiting on a counter the worker never
                # clears. Matches the existing "send errors drop the
                # event" contract.
                try:
                    self._send_broadcast(ws, join_ref, event)
                    self._emitted += 1
                finally:
                    self._queue.task_done()
        finally:
            try:
                ws.close()
            except Exception:  # pylint: disable=broad-except
                pass

    def _send_join(self, ws, join_ref: str) -> None:
        envelope = {
            'topic': self._topic,
            'event': 'phx_join',
            'ref': join_ref,
            'join_ref': join_ref,
            'payload': {
                'config': {
                    'broadcast': {'ack': False, 'self': False},
                    'presence': {'key': ''},
                    'postgres_changes': [],
                    'private': False,
                },
                'access_token': self._api_key,
            },
        }
        ws.send(json.dumps(envelope))

    def _send_heartbeat(self, ws) -> None:
        envelope = {
            'topic': 'phoenix',
            'event': 'heartbeat',
            'payload': {},
            'ref': self._next_ref(),
        }
        ws.send(json.dumps(envelope))

    def _send_broadcast(self, ws, join_ref: str,
                        event: Dict[str, Any]) -> None:
        envelope = {
            'topic': self._topic,
            'event': 'broadcast',
            'join_ref': join_ref,
            'ref': self._next_ref(),
            'payload': {
                'type': 'broadcast',
                'event': self._event_name,
                'payload': event,
            },
        }
        ws.send(json.dumps(envelope, default=str))


# ----------------------------------------------------------------- factory + auto-wiring

def _default_websocket_factory(url: str, timeout: float):
    """Import `websocket-client` lazily so users who don't use
    Supabase don't pay the dependency.

    Returns an object that quacks like a WebSocket: `send(str)`,
    `recv() -> str`, `close()`, `settimeout(float)`.

    On macOS the system Python often has no usable CA root bundle,
    so unprefixed `create_connection(wss://...)` raises
    ``CERTIFICATE_VERIFY_FAILED``. We point `sslopt['ca_certs']` at
    ``certifi.where()`` when certifi is importable — works on every
    platform, no per-host setup. Without certifi we fall back to
    whatever CAs the interpreter already trusts.
    """
    try:
        from websocket import create_connection  # type: ignore
    except ImportError as e:
        raise RuntimeError(
            'SupabaseRealtimeSink requires the `websocket-client` package. '
            'Install it with: pip install websocket-client'
        ) from e
    sslopt = None
    try:
        import certifi  # type: ignore
        sslopt = {'ca_certs': certifi.where()}
    except ImportError:
        # No certifi — let Python use its default trust store.
        # On macOS this is the failure mode that motivates `certifi`;
        # install it (`pip install certifi`) if you hit
        # CERTIFICATE_VERIFY_FAILED.
        pass
    if sslopt is not None:
        return create_connection(url, timeout=timeout, sslopt=sslopt)
    return create_connection(url, timeout=timeout)


def from_env(
        supabase_url: Optional[str] = None,
        api_key: Optional[str] = None,
        channel: Optional[str] = None,
        **kwargs,
) -> Optional['SupabaseRealtimeSink']:
    """Construct a `SupabaseRealtimeSink` from kwargs + env vars.

    Returns the sink if BOTH `supabase_url` AND `api_key` are
    resolvable (either via kwarg or env var); returns ``None``
    otherwise. Lets the caller do ``sink = from_env() or
    JsonlFileSink(...)``.

    Env vars consulted (kwargs win):
      - ``SUPABASE_URL``                  → supabase_url
      - ``SUPABASE_REALTIME_API_KEY``     → api_key
      - ``SUPABASE_REALTIME_CHANNEL``     → channel (default
                                            ``drift-profiler-events``)
    """
    url = supabase_url or os.environ.get(_ENV_URL)
    key = api_key or os.environ.get(_ENV_KEY)
    if not (url and key):
        return None
    return SupabaseRealtimeSink(
        supabase_url=url,
        api_key=key,
        channel=(channel
                 or os.environ.get(_ENV_CHANNEL)
                 or _DEFAULT_CHANNEL),
        **kwargs,
    )
