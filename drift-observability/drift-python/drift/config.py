"""Config loader.

Schema::

    service: my-service                            # required
    log_path: drift/logs/events.log                # FileSink path (default)
    redact: [password, token, secret]

    # Per-field toggles — all default to true. Set false to omit from events.
    include:
      pod: true
      service: true
      caller: true       # caller's file + line (added as `file`, `line`)
      args_bytes: true   # shallow memory footprint of input args (bytes)
      cpu: true          # 1-minute system load average (cached 1s)
      memory: true       # process RSS bytes at event time (cached 1s)
      rss: true          # process resident set size in bytes, at start+end (cached 1s)

    # Auto-wrap every function/method defined in the listed modules. Wrapped
    # methods record call/return events with `params: {}`. Entries explicitly
    # listed under `methods` win — those keep their `params` filter.
    call_graph:
      modules: [orders, app]

    # Optional: ship the local `log_path` to S3 at end-of-run (atexit) or on
    # SIGINT/SIGTERM. Backends are probed in order: boto3 → requests → urllib
    # (stdlib), so no new dependency is added.
    #
    # `bucket` is the only required field — everything else (region,
    # endpoint, addressing style, SSE, storage class, ACL, credentials)
    # falls through the standard AWS env-var chain at upload time, and
    # the boto3 path additionally honors AWS_PROFILE, ~/.aws/credentials,
    # and IAM instance/task roles. `DRIFT_S3_BUCKET` and `DRIFT_S3_PREFIX`
    # env vars can replace the YAML block entirely so observability can be
    # enabled on a deployment without editing config.
    s3:
      bucket: my-bucket             # required (or DRIFT_S3_BUCKET env)
      prefix: drift                 # optional object-key prefix
      # region: us-east-1           # optional — AWS_REGION / AWS_DEFAULT_REGION
      # endpoint_url: https://...   # optional — for MinIO / R2 / S3-compat
      # profile: prod               # optional — boto3 named profile
      # addressing_style: virtual   # "virtual" (AWS default) or "path"
      # sse: AES256                 # "AES256" or "aws:kms"
      # sse_kms_key_id: arn:aws:... # required when sse=aws:kms
      # storage_class: STANDARD_IA  # STANDARD | STANDARD_IA | GLACIER | ...
      # acl: bucket-owner-full-control
      # content_type: application/x-ndjson

    methods:
      - orders.OrderService.create
      - orders.OrderService.charge
      - { target: orders.OrderService.ship, params: [order_id] }
"""
from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

import yaml

from .sinks import FileSink, Sink

# Matches ${VAR} or ${VAR:-default}. Mirrors compose/k8s conventions.
_ENV_VAR_RE = re.compile(r"\$\{([A-Z_][A-Z0-9_]*)(?::-([^}]*))?\}")

DEFAULT_REDACT = ("password", "token", "authorization", "api_key", "secret", "ssn")
DEFAULT_LOG_PATH = "drift/logs/events.log"
DEFAULT_S3_REGION = "us-east-1"


@dataclass(frozen=True)
class Include:
    """Per-field include flags. All default true; set false to drop the field.

    `caller=True` adds the call-site as `file` (path) + `line` (int) to every
    event. The wrapped method's definition location is not emitted — only the
    call site, which is what consumers actually want when tracing flow.

    `args_bytes=True` adds a shallow memory-footprint estimate of the input
    args+kwargs (`sys.getsizeof` summed over container and top-level elements).
    Shallow on purpose — deep recursion would be unbounded on user payloads.

    `cpu=True` adds the system's 1-minute load average (`load` field) to each
    event. The value is cached for 1 second so high-frequency calls share a
    single syscall. Falls back to 0.0 on platforms without `os.getloadavg`.

    `rss=True` adds the process's resident set size (`rss` field, bytes) to
    every start AND end event. Sampled with the same 1-second TTL cache as
    `load` — Linux reads `/proc/self/statm`, macOS reads
    `resource.getrusage().ru_maxrss`. Falls back to 0 on platforms without
    either (Windows). The start/end pair makes the per-call memory delta
    visible for long-running calls without paying a per-call syscall.
    """
    pod: bool = True
    service: bool = True
    caller: bool = True
    args_bytes: bool = True
    cpu: bool = True
    rss: bool = True


@dataclass(frozen=True)
class CallGraph:
    """Auto-wrap configuration.

    `modules` is a list of import paths. At install time, each module is
    imported and every top-level function or class method defined IN that
    module (`__module__ == module_name`) is wrapped with an empty params
    filter, so the resulting events carry call/return + timing but no
    captured arguments.

    Names starting with `_` are skipped (dunders, private). Methods that
    also appear in `Config.methods` are left to the explicit entry, which
    keeps its `params` configuration.
    """
    modules: tuple[str, ...] = ()


@dataclass
class Method:
    target: str
    params: list[str] | None = None


@dataclass(frozen=True)
class S3Config:
    """End-of-run S3 upload target.

    `bucket` is the only required field — every other value resolves through
    the standard AWS environment-variable chain at upload time (see
    `drift.s3_upload._resolve_settings`):

        region:           AWS_REGION → AWS_DEFAULT_REGION → "us-east-1"
        endpoint_url:     AWS_ENDPOINT_URL_S3 → AWS_ENDPOINT_URL → (default AWS)
        addressing_style: AWS_S3_ADDRESSING_STYLE → "virtual" (AWS) / "path" (custom)
        credentials:      AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN
                          (boto3 path additionally honors AWS_PROFILE,
                           ~/.aws/credentials, IAM instance/task roles)
        sse:              AWS_S3_SSE              ("AES256" | "aws:kms")
        sse_kms_key_id:   AWS_S3_SSE_KMS_KEY_ID
        storage_class:    AWS_S3_STORAGE_CLASS    (STANDARD | STANDARD_IA | …)
        acl:              AWS_S3_ACL              (private | bucket-owner-full-control | …)
        content_type:     AWS_S3_CONTENT_TYPE     (default "application/x-ndjson")

    Any field set explicitly in YAML wins over the env. `bucket` and `prefix`
    can also be set via `DRIFT_S3_BUCKET` / `DRIFT_S3_PREFIX` so observability
    can be toggled on a deployment without editing the config file.

    The final object key is `{prefix}/{service}/{YYYY-MM-DD}/{HHMMSS}-{host}-{pid}.log`.
    """
    bucket: str
    prefix: str = ""
    region: Optional[str] = None
    endpoint_url: Optional[str] = None
    profile: Optional[str] = None
    access_key_id: Optional[str] = None
    secret_access_key: Optional[str] = None
    session_token: Optional[str] = None
    addressing_style: Optional[str] = None   # "virtual" | "path"
    sse: Optional[str] = None                # "AES256" | "aws:kms"
    sse_kms_key_id: Optional[str] = None
    storage_class: Optional[str] = None      # STANDARD | STANDARD_IA | GLACIER | …
    acl: Optional[str] = None                # private | bucket-owner-full-control | …
    content_type: Optional[str] = None       # default "application/x-ndjson"


@dataclass
class Config:
    service: str
    redact: tuple[str, ...] = DEFAULT_REDACT
    methods: list[Method] = field(default_factory=list)
    log_path: str = DEFAULT_LOG_PATH
    include: Include = field(default_factory=Include)
    call_graph: CallGraph = field(default_factory=CallGraph)
    s3: Optional[S3Config] = None

    def build_sink(self) -> Sink:
        return FileSink(self.log_path)


def load(path: str | Path) -> Config:
    text = _expand_env_vars(Path(path).read_text())
    raw = yaml.safe_load(text)
    if not isinstance(raw, dict):
        raise ValueError("drift config must be a YAML mapping at top level")

    service = raw.get("service")
    if not service:
        raise ValueError("drift config: 'service' is required")

    methods = [_parse_method(m, i) for i, m in enumerate(raw.get("methods") or [])]

    return Config(
        service=service,
        redact=tuple(raw.get("redact") or DEFAULT_REDACT),
        methods=methods,
        log_path=raw.get("log_path", DEFAULT_LOG_PATH),
        include=_parse_include(raw.get("include")),
        call_graph=_parse_call_graph(raw.get("call_graph")),
        s3=_parse_s3(raw.get("s3")),
    )


def _parse_include(raw: object) -> Include:
    if raw is None:
        return Include()
    if not isinstance(raw, dict):
        raise ValueError("drift config: 'include' must be a mapping")
    # Unknown keys (including legacy `file`) are ignored to keep the schema
    # forward-compatible.
    return Include(
        pod=bool(raw.get("pod", True)),
        service=bool(raw.get("service", True)),
        caller=bool(raw.get("caller", True)),
        args_bytes=bool(raw.get("args_bytes", True)),
        cpu=bool(raw.get("cpu", True)),
        rss=bool(raw.get("rss", True)),
    )


def _parse_call_graph(raw: object) -> CallGraph:
    if raw is None:
        return CallGraph()
    if not isinstance(raw, dict):
        raise ValueError("drift config: 'call_graph' must be a mapping")
    modules = raw.get("modules") or ()
    if not isinstance(modules, (list, tuple)):
        raise ValueError("drift config: 'call_graph.modules' must be a list")
    return CallGraph(modules=tuple(str(m) for m in modules))


def _parse_s3(raw: object) -> Optional[S3Config]:
    """Parse the `s3:` YAML block, OR build one from env vars when the YAML
    omits it.

    Setting `DRIFT_S3_BUCKET` (and optionally `DRIFT_S3_PREFIX`) is enough to
    turn on uploads in production without re-shipping config. Everything
    else (region, endpoint, credentials) is resolved at upload time through
    the standard AWS env-var chain — see `S3Config` docstring.
    """
    if raw is None:
        bucket = os.environ.get("DRIFT_S3_BUCKET")
        if not bucket:
            return None
        return S3Config(
            bucket=bucket,
            prefix=os.environ.get("DRIFT_S3_PREFIX", ""),
        )
    if not isinstance(raw, dict):
        raise ValueError("drift config: 's3' must be a mapping")
    bucket = raw.get("bucket") or os.environ.get("DRIFT_S3_BUCKET")
    if not bucket:
        raise ValueError(
            "drift config: 's3.bucket' is required when 's3' is set "
            "(or set DRIFT_S3_BUCKET in env)"
        )
    return S3Config(
        bucket=str(bucket),
        prefix=str(raw.get("prefix") or os.environ.get("DRIFT_S3_PREFIX") or ""),
        region=_opt_str(raw.get("region")),
        endpoint_url=_opt_str(raw.get("endpoint_url")),
        profile=_opt_str(raw.get("profile")),
        access_key_id=_opt_str(raw.get("access_key_id")),
        secret_access_key=_opt_str(raw.get("secret_access_key")),
        session_token=_opt_str(raw.get("session_token")),
    )


def _opt_str(v: object) -> Optional[str]:
    if v is None or v == "":
        return None
    return str(v)


def _expand_env_vars(text: str) -> str:
    def replace(match: re.Match[str]) -> str:
        name = match.group(1)
        default = match.group(2) or ""
        return os.environ.get(name, default)
    return _ENV_VAR_RE.sub(replace, text)


def _parse_method(entry: object, idx: int) -> Method:
    if isinstance(entry, str):
        return Method(target=entry)
    if isinstance(entry, dict):
        target = entry.get("target")
        if not target:
            raise ValueError(f"methods[{idx}]: 'target' required")
        params = entry.get("params")
        if params is not None and not isinstance(params, list):
            raise ValueError(f"methods[{idx}].params must be a list")
        return Method(target=target, params=params)
    raise ValueError(f"methods[{idx}]: must be string or mapping, got {type(entry).__name__}")
