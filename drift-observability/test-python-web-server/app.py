"""FastAPI test app — exercises `driftdockerprofiler` end-to-end.

Unlike the old `drift-python` trace agent, this profiler is a stack
sampler (SIGALRM wall + SIGPROF CPU). It does NOT wrap functions and
does NOT need to see them defined before install — so we start it at
the top of the module rather than the bottom. The sampler runs in
background threads and emits one JSONL event per unique call stack per
profile window.

All knobs are env vars so the same image runs cleanly under docker
compose, Tilt, and a bare `uvicorn` invocation:

  DRIFT_SERVICE       service label stamped on every event
                      (default: 'test-python-web-server')
  DRIFT_EVENTS_PATH   JSONL output file shared with observability-server
                      (default: /trace/events.log — same as TRACE_PATH on
                      the Go side)
  DRIFT_PERIOD_MS     sampling interval                 (default: 10)
  DRIFT_DURATION_MS   profile-window length             (default: 10000)
  DRIFT_EMIT_MODE     'per_trace' | 'bundle'            (default: per_trace)
"""
from __future__ import annotations

import os
import uuid
# typing.List / typing.Dict instead of the 3.9+ `list[...]` /
# `dict[...]` builtin generics — pydantic v1 evals annotation strings
# at runtime, and `list[dict]` raises `'type' object is not
# subscriptable` on python:3.7-slim.
from typing import Any, Dict, List

# Start the profiler BEFORE FastAPI / handlers exist. Stack sampling
# captures whatever's on the stack at signal time — it doesn't need to
# walk the module to discover routes the way the old trace agent did.
import driftdockerprofiler

driftdockerprofiler.start(
    service=os.environ.get("DRIFT_SERVICE", "test-python-web-server"),
    output_path=os.environ.get("DRIFT_EVENTS_PATH", "/trace/events.log"),
    period_ms=int(os.environ.get("DRIFT_PERIOD_MS", "10")),
    duration_ms=int(os.environ.get("DRIFT_DURATION_MS", "10000")),
    emit_mode=os.environ.get("DRIFT_EMIT_MODE", "per_trace"),
    # exclude_paths is NOT passed — the profiler-internal frames
    # (`/driftdockerprofiler/`, `/_profiler.cpython`, `<frozen ...>`)
    # are already in `BUILTIN_EXCLUDE_PATHS`. Pass project-specific
    # patterns here only.
)

from fastapi import FastAPI, HTTPException  # noqa: E402 — after profiler.start()
from pydantic import BaseModel  # noqa: E402

from orders import OrderService  # noqa: E402

app = FastAPI(
    title="driftdockerprofiler test-python-web-server",
    description=(
        "Sample FastAPI service profiled by **driftdockerprofiler**. The "
        "wall + CPU stack samplers run continuously and emit one JSONL "
        "event per unique call stack per profile window to "
        "`/trace/events.log`. The observability-server tails that file "
        "and streams events to "
        "[the live viewer](http://localhost:8080/live)."
    ),
    version="0.2.0",
)
service = OrderService()


class CreateBody(BaseModel):
    customer_id: str
    customer_tier: str = "standard"
    items: List[Dict[str, Any]] = []


class ChargeBody(BaseModel):
    amount_value: float
    amount_currency: str = "USD"
    password: str  # carried verbatim — sampler does not capture args


@app.get("/healthz")
def healthz() -> dict:
    return {"status": "ok"}


@app.post("/orders")
def create_order(body: CreateBody) -> dict:
    order_id = "o-" + uuid.uuid4().hex[:8]
    result = service.create(
        order_id=order_id,
        customer={"id": body.customer_id, "tier": body.customer_tier},
        items=body.items,
    )
    # `{**result, "order_id": ...}` instead of `result | {...}` — the
    # latter is Python 3.9+ and this app runs on python:3.7-slim.
    return {**result, "order_id": order_id}


@app.post("/orders/{order_id}/charge")
async def charge_order(order_id: str, body: ChargeBody) -> dict:
    try:
        return await service.charge(
            order_id=order_id,
            amount={"value": body.amount_value, "currency": body.amount_currency},
            password=body.password,
        )
    except RuntimeError as e:
        raise HTTPException(status_code=402, detail=str(e))


@app.post("/orders/{order_id}/ship")
def ship_order(order_id: str) -> dict:
    return service.ship(order_id=order_id)


# DEBUG: Phase-3 sampler verification. Returns whatever
# `sys._current_frames()` sees right now — the same thing the
# WallAllThreadsSampler walks every tick. Use during a parallel
# `POST /orders` to see whether the worker thread is visible.
@app.get("/debug/threads")
def debug_threads() -> dict:
    import sys, threading
    snapshot = []
    for tid, frame in sys._current_frames().items():
        thread = next((t for t in threading.enumerate()
                       if t.ident == tid), None)
        name = thread.name if thread else "<unknown>"
        leaf = {
            "name": frame.f_code.co_name,
            "file": frame.f_code.co_filename,
            "line": frame.f_lineno,
        }
        snapshot.append({"tid": tid, "thread_name": name, "leaf": leaf})
    return {"threads": snapshot}
