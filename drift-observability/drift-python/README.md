# drift

Minimal method-tracing for Python services.

## Install (editable)

```bash
pip install -e ".[dev]"
```

## Build wheel

```bash
python -m build       # produces dist/drift-0.1.0-py3-none-any.whl
```

## Use

```python
import drift
drift.install("/etc/drift/config.yaml")
```

That's the whole API. After this line, every method listed in the config
is wrapped; each call emits two JSONL events:

```json
{"call":"<uuid>","qualname":"X.foo","params":{"a":1},"time":"...","args_bytes":128,"load":0.42}
{"call":"<uuid>","qualname":"X.foo","time":"...","status":"ok","duration_ms":1.2,"load":0.42}
```

Start vs end is distinguished by presence of `params` (start) vs
`duration_ms` (end) — there is no `phase` field. `args_bytes` is a
shallow `sys.getsizeof` over the args + kwargs container and its
top-level elements; `load` is the 1-minute system load average (cached
1s, shared across all wrapped methods).

## Config

```yaml
service: my-service        # required — stamped on every event
log_path: drift/logs/events.log
redact: [password, token, secret]

# Per-field toggles — all default true. Set false to drop a field from events.
include:
  pod: true
  service: true
  caller: true        # call-site file + line (added as `file`, `line`)
  args_bytes: true    # shallow sys.getsizeof of args/kwargs (on start events)
  cpu: true           # 1-min system load average as `load` (on every event,
                      # cached 1s — one syscall per second, shared globally)

# Auto-wrap every top-level function and class method defined in the listed
# modules. Auto-wrapped calls emit events with `params: {}` — only entries
# under `methods` below capture their arguments. Names starting with `_` are
# skipped. This is wrap-time discovery (NOT sys.setprofile), so there's zero
# per-call cost beyond the methods we actually decided to record.
call_graph:
  modules: [orders, app]

methods:
  - orders.OrderService.create              # capture all kwargs
  - orders.OrderService.charge
  - target: orders.OrderService.ship        # capture only listed params
    params: [order_id]

# Optional: ship the local log file to S3 at end-of-run (atexit) or on
# SIGINT/SIGTERM. Backends are tried in order: boto3 → requests → urllib
# (stdlib), so no new dependency is required.
#
# Everything except `bucket` resolves through the standard AWS env-var chain
# at upload time, so deployments don't need to duplicate credentials in YAML:
#     region:       AWS_REGION → AWS_DEFAULT_REGION
#     endpoint_url: AWS_ENDPOINT_URL_S3 → AWS_ENDPOINT_URL
#     credentials:  AWS_ACCESS_KEY_ID/SECRET/SESSION_TOKEN, AWS_PROFILE,
#                   ~/.aws/credentials, IAM role  (boto3 path: full chain)
#
# `DRIFT_S3_BUCKET` / `DRIFT_S3_PREFIX` can replace the YAML block entirely
# — set them in the deployment env to turn uploads on without re-shipping.
s3:
  bucket: my-bucket
  prefix: drift                 # optional object-key prefix
  # region:        us-east-1    # optional — defaults to env or us-east-1
  # endpoint_url:  https://...  # optional — for MinIO / R2 / S3-compatible
  # profile:       prod         # optional — boto3 named profile
```

Set `DRIFT_DISABLED=1` to install as a no-op.

## Test

```bash
pytest -q
```
