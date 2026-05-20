# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Tests for `SupabaseRealtimeSink`.

We deliberately don't hit live Supabase — instead we inject a
`FakeWebSocket` that records every message. The tests pin down:

  - the Phoenix Channels envelope shape (join, broadcast, heartbeat)
  - `emit()` never blocks and never raises
  - drop-on-full when the queue is saturated
  - failure isolation (broken WebSocket factory doesn't kill the app)
  - env-var auto-wiring through `from_env()`
  - `Sink` protocol conformance
"""

import json
import os
import queue
import threading
import time

import pytest

from driftdockerprofiler.sinks import Sink, SupabaseRealtimeSink
from driftdockerprofiler.sinks.supabase import (
    _build_wss_url,
    _DEFAULT_CHANNEL,
    _VSN,
    from_env,
)


# ----------------------------------------------------------------- helpers

class FakeWebSocket:
    """Replaces a real `websocket.WebSocket` for tests. Captures every
    `send()` and lets the worker `recv()` block (we don't care about
    server replies in fire-and-forget broadcasting)."""

    def __init__(self):
        self.sent = []          # list of JSON strings sent
        self.closed = False
        self._timeout = None
        self._lock = threading.Lock()

    def send(self, data):
        with self._lock:
            self.sent.append(data)

    def recv(self):
        # Block briefly then return empty — the sink doesn't process
        # server replies in this test path.
        time.sleep(0.05)
        return ''

    def close(self):
        self.closed = True

    def settimeout(self, timeout):
        self._timeout = timeout

    # Test-side helper.
    def parsed(self):
        with self._lock:
            return [json.loads(s) for s in self.sent]


def _make_sink(**overrides):
    """Build a sink wired to a FakeWebSocket — never touches network."""
    fake_ws = FakeWebSocket()

    def factory(url, timeout):
        return fake_ws

    sink = SupabaseRealtimeSink(
        supabase_url='https://test.supabase.co',
        api_key='test-jwt',
        websocket_factory=factory,
        **overrides,
    )
    return sink, fake_ws


def _wait_for(predicate, timeout=2.0, step=0.01):
    """Poll until `predicate()` is truthy or `timeout` elapses."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return False


# ----------------------------------------------------------------- url construction

def test_build_wss_url_from_https():
    url = _build_wss_url('https://abc123.supabase.co', 'JWT_HERE')
    assert url == (
        'wss://abc123.supabase.co/realtime/v1/websocket'
        '?apikey=JWT_HERE&vsn=' + _VSN
    )


def test_build_wss_url_accepts_wss_input():
    url = _build_wss_url('wss://abc123.supabase.co', 'JWT')
    assert url.startswith('wss://abc123.supabase.co/realtime/v1/websocket')


def test_build_wss_url_rejects_garbage():
    with pytest.raises(ValueError, match='supabase_url'):
        _build_wss_url('not-a-url-at-all', 'JWT')


# ----------------------------------------------------------------- construction

def test_construction_starts_worker_thread():
    sink, _ws = _make_sink()
    try:
        assert any(t.name == 'driftdockerprofiler-supabase-sink'
                   for t in threading.enumerate())
    finally:
        sink.close()


def test_construction_requires_url_and_key():
    with pytest.raises(ValueError, match='supabase_url'):
        SupabaseRealtimeSink(supabase_url='', api_key='k')
    with pytest.raises(ValueError, match='api_key'):
        SupabaseRealtimeSink(supabase_url='https://x.supabase.co', api_key='')


# ----------------------------------------------------------------- Sink protocol

def test_implements_sink_protocol():
    sink, _ws = _make_sink()
    try:
        assert isinstance(sink, Sink)
    finally:
        sink.close()


# ----------------------------------------------------------------- envelope shape

def test_join_envelope_matches_phoenix_protocol():
    sink, ws = _make_sink(channel='my-channel')
    try:
        _wait_for(lambda: len(ws.sent) >= 1)
    finally:
        sink.close()

    join = ws.parsed()[0]
    assert join['topic'] == 'realtime:my-channel'
    assert join['event'] == 'phx_join'
    assert 'ref' in join and 'join_ref' in join
    assert join['ref'] == join['join_ref']
    payload = join['payload']
    assert payload['access_token'] == 'test-jwt'
    cfg = payload['config']
    assert cfg['broadcast'] == {'ack': False, 'self': False}
    assert cfg['private'] is False
    assert cfg['postgres_changes'] == []


def test_broadcast_envelope_matches_phoenix_protocol():
    sink, ws = _make_sink(channel='c', event_name='profiler-event')
    try:
        sink.emit({'type': 'wall_trace', 'count': 42})
        _wait_for(lambda: any(
            json.loads(s).get('event') == 'broadcast' for s in ws.sent))
    finally:
        sink.close()

    broadcast = next(m for m in ws.parsed() if m['event'] == 'broadcast')
    assert broadcast['topic'] == 'realtime:c'
    payload = broadcast['payload']
    assert payload['type'] == 'broadcast'
    assert payload['event'] == 'profiler-event'
    assert payload['payload'] == {'type': 'wall_trace', 'count': 42}
    # broadcasts always carry the join_ref so the server routes
    # them to the right channel process.
    assert 'join_ref' in broadcast


def test_default_channel_when_omitted():
    sink, ws = _make_sink()
    try:
        _wait_for(lambda: len(ws.sent) >= 1)
    finally:
        sink.close()
    assert ws.parsed()[0]['topic'] == 'realtime:' + _DEFAULT_CHANNEL


# ----------------------------------------------------------------- emit semantics

def test_emit_never_blocks_or_raises():
    sink, _ws = _make_sink()
    try:
        # The worker may or may not have sent the join yet — that's
        # fine. emit() must be safe regardless.
        for i in range(100):
            sink.emit({'i': i})
    finally:
        sink.close()


def test_emit_drops_when_queue_full():
    """emit() must NEVER block the caller — drop-on-full is the
    contract that keeps profiler emit() off the request hot path."""
    sink, _ws = _make_sink(max_queue=2)
    # Don't let the worker drain by stopping it before any emit. We
    # do this by setting the stop event directly.
    sink._stop.set()
    sink._worker.join(timeout=2.0)
    # Now the queue can't drain. Fill it past capacity.
    sink.emit({'n': 1})
    sink.emit({'n': 2})
    sink.emit({'n': 3})    # third must be dropped
    sink.emit({'n': 4})    # fourth too
    assert sink.dropped >= 2


def test_emit_handles_non_serializable_via_default_str():
    """Profile events may contain `time` as an int; non-JSON things
    shouldn't crash the sink — `default=str` covers them."""
    sink, ws = _make_sink()
    try:
        sink.emit({'time': 1_234_567_890,
                   'obj': object()})    # unrepresentable
        _wait_for(lambda: any(
            json.loads(s).get('event') == 'broadcast'
            for s in ws.sent))
    finally:
        sink.close()
    broadcasts = [m for m in ws.parsed() if m['event'] == 'broadcast']
    assert broadcasts, 'broadcast never sent'


# ----------------------------------------------------------------- failure isolation

def test_broken_websocket_factory_does_not_crash_emit():
    """If the WS lib fails to connect, emit() MUST stay quiet. The
    worker reconnects in the background."""
    def broken_factory(url, timeout):
        raise OSError('connection refused')

    sink = SupabaseRealtimeSink(
        supabase_url='https://x.supabase.co',
        api_key='k',
        websocket_factory=broken_factory,
        # We test that emit() doesn't crash, not that the worker
        # eventually succeeds.
    )
    try:
        # Caller should not see exceptions.
        for i in range(50):
            sink.emit({'i': i})
    finally:
        sink.close()


def test_worker_reconnects_after_failure(monkeypatch):
    """First connection raises, second succeeds — the worker MUST
    keep trying. We patch `_BACKOFF_STEPS` to zero so the test runs
    fast."""
    import driftdockerprofiler.sinks.supabase as sb
    monkeypatch.setattr(sb, '_BACKOFF_STEPS', (0.0,))

    call_count = [0]
    good_ws = FakeWebSocket()

    def flaky_factory(url, timeout):
        call_count[0] += 1
        if call_count[0] == 1:
            raise OSError('first call fails')
        return good_ws

    sink = SupabaseRealtimeSink(
        supabase_url='https://x.supabase.co',
        api_key='k',
        websocket_factory=flaky_factory,
    )
    try:
        # Wait until the second factory call has fed the join.
        assert _wait_for(lambda: len(good_ws.sent) >= 1, timeout=3.0), (
            'worker did not reconnect after the first failure')
    finally:
        sink.close()
    assert call_count[0] >= 2


# ----------------------------------------------------------------- close

def test_close_is_idempotent():
    sink, ws = _make_sink()
    sink.close()
    sink.close()
    sink.close()
    assert ws.closed


# ----------------------------------------------------------------- from_env

def test_from_env_returns_none_without_required_vars(monkeypatch):
    monkeypatch.delenv('SUPABASE_URL', raising=False)
    monkeypatch.delenv('SUPABASE_REALTIME_API_KEY', raising=False)
    assert from_env() is None


def test_from_env_returns_none_with_only_one_var(monkeypatch):
    monkeypatch.setenv('SUPABASE_URL', 'https://x.supabase.co')
    monkeypatch.delenv('SUPABASE_REALTIME_API_KEY', raising=False)
    assert from_env() is None


def test_from_env_constructs_when_both_present(monkeypatch):
    monkeypatch.setenv('SUPABASE_URL', 'https://x.supabase.co')
    monkeypatch.setenv('SUPABASE_REALTIME_API_KEY', 'jwt')
    fake_ws = FakeWebSocket()

    def factory(url, timeout):
        return fake_ws

    sink = from_env(websocket_factory=factory)
    assert sink is not None
    try:
        assert isinstance(sink, SupabaseRealtimeSink)
        # Default channel honored.
        assert ('realtime:' + _DEFAULT_CHANNEL) in sink._topic
    finally:
        sink.close()


def test_from_env_honors_channel_override(monkeypatch):
    monkeypatch.setenv('SUPABASE_URL', 'https://x.supabase.co')
    monkeypatch.setenv('SUPABASE_REALTIME_API_KEY', 'jwt')
    monkeypatch.setenv('SUPABASE_REALTIME_CHANNEL', 'custom-channel')
    fake_ws = FakeWebSocket()

    def factory(url, timeout):
        return fake_ws

    sink = from_env(websocket_factory=factory)
    try:
        assert sink._topic == 'realtime:custom-channel'
    finally:
        sink.close()


def test_kwargs_override_env(monkeypatch):
    monkeypatch.setenv('SUPABASE_URL', 'https://wrong.supabase.co')
    monkeypatch.setenv('SUPABASE_REALTIME_API_KEY', 'wrong-jwt')
    fake_ws = FakeWebSocket()

    def factory(url, timeout):
        return fake_ws

    sink = from_env(
        supabase_url='https://right.supabase.co',
        api_key='right-jwt',
        websocket_factory=factory,
    )
    try:
        assert 'right.supabase.co' in sink.url
        assert 'right-jwt' in sink.url
    finally:
        sink.close()
