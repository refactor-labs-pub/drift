"""End-of-run S3 handler for the local JSONL log file.

Triggered from `drift.uninstall()` (which also fires from `atexit` and from
the SIGINT/SIGTERM handlers registered in `instrument.install`). Best-effort:
every failure mode is caught and logged — the host process must never crash
because the remote sink is unhappy.

Configuration resolution (lowest → highest priority):

    1. Hardcoded default       — region "us-east-1", no endpoint override
    2. AWS standard env vars   — AWS_REGION, AWS_DEFAULT_REGION,
                                  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY,
                                  AWS_SESSION_TOKEN, AWS_PROFILE,
                                  AWS_ENDPOINT_URL_S3, AWS_ENDPOINT_URL
    3. Drift-specific env vars — DRIFT_S3_BUCKET, DRIFT_S3_PREFIX
                                  (allow enabling s3 without YAML changes)
    4. YAML `s3:` block         — explicit values always win

This mirrors what boto3 itself does. Whenever boto3 is installed we let
`boto3.Session` do the resolution natively (env vars, shared credentials
file, IAM instance/task roles, SSO, …). Only the manual SigV4 fallback
paths (requests / urllib) read env vars directly.

HTTP-client priority (no added dependencies):

    1. boto3   — preferred when the host project already uses the AWS SDK.
                 Uses Session + botocore.Config + TransferConfig — the
                 official multipart-aware upload path.
    2. requests — second choice if it's available in the parent environment.
    3. urllib  — pure-stdlib fallback. We hand-sign with AWS Signature V4.

`ImportError` from a backend is silent and moves on to the next; any other
error short-circuits with a warning (we don't want to double-upload the
same bytes with two different libraries).
"""
from __future__ import annotations

import hashlib
import hmac
import logging
import os
import socket
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional, Tuple
from urllib.parse import quote

from .config import S3Config

logger = logging.getLogger("drift.s3_upload")

_DEFAULT_REGION = "us-east-1"
_AWS_HOST_FMT = "{bucket}.s3.{region}.amazonaws.com"
_HTTP_TIMEOUT_S = 60.0

# Standard AWS env vars, listed once so the env-resolution code below stays
# readable. botocore's published precedence is the source of truth.
_ENV_REGION = ("AWS_REGION", "AWS_DEFAULT_REGION")
_ENV_ENDPOINT = ("AWS_ENDPOINT_URL_S3", "AWS_ENDPOINT_URL")


@dataclass(frozen=True)
class _Settings:
    """Effective S3 settings after merging YAML + env. All fields resolved."""
    bucket: str
    prefix: str
    region: str
    endpoint_url: Optional[str]
    profile: Optional[str]
    access_key_id: Optional[str]
    secret_access_key: Optional[str]
    session_token: Optional[str]


def _first_env(*names: str) -> Optional[str]:
    for n in names:
        v = os.environ.get(n)
        if v:
            return v
    return None


def _resolve_settings(cfg: S3Config) -> _Settings:
    """Merge an `S3Config` (YAML) with the AWS env-var chain. YAML wins."""
    return _Settings(
        bucket=cfg.bucket,
        prefix=cfg.prefix,
        region=cfg.region or _first_env(*_ENV_REGION) or _DEFAULT_REGION,
        endpoint_url=cfg.endpoint_url or _first_env(*_ENV_ENDPOINT),
        profile=cfg.profile or os.environ.get("AWS_PROFILE"),
        access_key_id=cfg.access_key_id or os.environ.get("AWS_ACCESS_KEY_ID"),
        secret_access_key=cfg.secret_access_key or os.environ.get("AWS_SECRET_ACCESS_KEY"),
        session_token=cfg.session_token or os.environ.get("AWS_SESSION_TOKEN"),
    )


# ----------------------------------------------------------------- public API

def upload(local_path: str | Path, s3_config: S3Config, service: str = "drift") -> bool:
    """Upload `local_path` to s3://{bucket}/{key}. Never raises.

    Returns True on success, False on any failure (missing file, no creds,
    network error, non-2xx response).
    """
    p = Path(local_path)
    if not p.exists():
        logger.info("drift s3 upload skipped: %s does not exist", p)
        return False
    if p.stat().st_size == 0:
        logger.info("drift s3 upload skipped: %s is empty", p)
        return False

    settings = _resolve_settings(s3_config)
    key = _build_key(settings.prefix, service)

    for backend, fn in (
        ("boto3", _upload_boto3),
        ("requests", _upload_requests),
        ("urllib", _upload_urllib),
    ):
        try:
            ok = fn(p, key, settings)
        except ImportError:
            # Backend not installed — try the next one.
            continue
        except Exception as e:
            logger.warning("drift s3 upload via %s failed: %s", backend, e)
            return False
        if ok:
            logger.info(
                "drift s3 upload via %s succeeded: s3://%s/%s",
                backend, settings.bucket, key,
            )
        return ok
    return False  # unreachable: urllib is stdlib


def _build_key(prefix: str, service: str) -> str:
    """Compose `{prefix}/{service}/{YYYY-MM-DD}/{HHMMSS}-{host}-{pid}.log`.

    Including host+pid guarantees uniqueness across pods sharing a prefix.
    """
    now = datetime.now(timezone.utc)
    date = now.strftime("%Y-%m-%d")
    ts = now.strftime("%H%M%S")
    host = (os.environ.get("HOSTNAME") or socket.gethostname() or "host").replace("/", "-")
    pid = os.getpid()
    parts = [
        seg for seg in (prefix.strip("/"), service, date, f"{ts}-{host}-{pid}.log")
        if seg
    ]
    return "/".join(parts)


# ----------------------------------------------------------------- backends

def _upload_boto3(path: Path, key: str, s: _Settings) -> bool:
    """Official path: `boto3.Session` resolves credentials through the full
    AWS chain (env → shared file → AWS_PROFILE → IAM role → SSO), and
    `client.upload_file` uses the multipart-aware transfer manager.

    Only the fields the caller set explicitly are forwarded to the Session;
    everything else is left for boto3 to discover natively.
    """
    import boto3  # ImportError → fall through to requests
    from boto3.s3.transfer import TransferConfig
    from botocore.config import Config as BotoConfig

    session_kwargs: dict = {"region_name": s.region}
    if s.profile:
        session_kwargs["profile_name"] = s.profile
    # Only override creds when the caller actually passed them; otherwise
    # let boto3's default credential chain run (instance profile, SSO, etc.).
    if s.access_key_id and s.secret_access_key:
        session_kwargs["aws_access_key_id"] = s.access_key_id
        session_kwargs["aws_secret_access_key"] = s.secret_access_key
        if s.session_token:
            session_kwargs["aws_session_token"] = s.session_token

    session = boto3.Session(**session_kwargs)

    boto_config = BotoConfig(
        retries={"max_attempts": 3, "mode": "standard"},
        connect_timeout=10,
        read_timeout=_HTTP_TIMEOUT_S,
    )
    client_kwargs: dict = {"config": boto_config}
    if s.endpoint_url:
        client_kwargs["endpoint_url"] = s.endpoint_url

    client = session.client("s3", **client_kwargs)
    # 8 MiB threshold matches botocore's default but is stated explicitly so
    # the behavior is obvious to anyone reading this.
    transfer_config = TransferConfig(multipart_threshold=8 * 1024 * 1024)
    client.upload_file(str(path), s.bucket, key, Config=transfer_config)
    return True


def _upload_requests(path: Path, key: str, s: _Settings) -> bool:
    import requests  # ImportError → fall through to urllib

    body = path.read_bytes()
    url, headers = _sigv4(method="PUT", key=key, body=body, s=s)
    resp = requests.put(url, data=body, headers=headers, timeout=_HTTP_TIMEOUT_S)
    if 200 <= resp.status_code < 300:
        return True
    logger.warning(
        "drift s3 upload via requests rejected: HTTP %s — %s",
        resp.status_code, resp.text[:200],
    )
    return False


def _upload_urllib(path: Path, key: str, s: _Settings) -> bool:
    import urllib.error
    import urllib.request

    body = path.read_bytes()
    url, headers = _sigv4(method="PUT", key=key, body=body, s=s)
    req = urllib.request.Request(url, data=body, method="PUT", headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=_HTTP_TIMEOUT_S) as resp:
            return 200 <= resp.status < 300
    except urllib.error.HTTPError as e:
        try:
            payload = e.read().decode("utf-8", "replace")[:200]
        except Exception:
            payload = ""
        logger.warning("drift s3 upload via urllib rejected: HTTP %s — %s", e.code, payload)
        return False


# --------------------------------------------------------- AWS Signature V4
#
# Implemented locally so we don't pull in botocore for the requests/urllib
# fallback paths. Reference:
#   https://docs.aws.amazon.com/IAM/latest/UserGuide/create-signed-request.html


def _sigv4(
    *,
    method: str,
    key: str,
    body: bytes,
    s: _Settings,
) -> Tuple[str, dict]:
    if not s.access_key_id or not s.secret_access_key:
        raise RuntimeError(
            "drift s3 upload requires AWS credentials "
            "(set s3.access_key_id/secret_access_key in YAML or "
            "AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY in env)"
        )

    # For S3-compatible endpoints (MinIO, Cloudflare R2, …) keep the
    # configured host; otherwise use AWS virtual-hosted style.
    if s.endpoint_url:
        from urllib.parse import urlparse
        parsed = urlparse(s.endpoint_url)
        scheme = parsed.scheme or "https"
        endpoint_host = parsed.netloc
        # Path-style for arbitrary endpoints: /{bucket}/{key}
        canonical_uri = "/" + s.bucket + "/" + _encode_segments(key.lstrip("/"))
        host = endpoint_host
        url = f"{scheme}://{host}{canonical_uri}"
    else:
        host = _AWS_HOST_FMT.format(bucket=s.bucket, region=s.region)
        canonical_uri = "/" + _encode_segments(key.lstrip("/"))
        url = f"https://{host}{canonical_uri}"

    now = datetime.now(timezone.utc)
    amz_date = now.strftime("%Y%m%dT%H%M%SZ")
    date_stamp = now.strftime("%Y%m%d")

    payload_hash = hashlib.sha256(body).hexdigest()

    sign_headers = {
        "host": host,
        "x-amz-content-sha256": payload_hash,
        "x-amz-date": amz_date,
    }
    if s.session_token:
        sign_headers["x-amz-security-token"] = s.session_token

    sorted_names = sorted(sign_headers)
    canonical_headers = "".join(f"{n}:{sign_headers[n]}\n" for n in sorted_names)
    signed_headers = ";".join(sorted_names)
    canonical_request = "\n".join([
        method,
        canonical_uri,
        "",  # no query string
        canonical_headers,
        signed_headers,
        payload_hash,
    ])

    credential_scope = f"{date_stamp}/{s.region}/s3/aws4_request"
    string_to_sign = "\n".join([
        "AWS4-HMAC-SHA256",
        amz_date,
        credential_scope,
        hashlib.sha256(canonical_request.encode("utf-8")).hexdigest(),
    ])

    signing_key = _derive_signing_key(s.secret_access_key, date_stamp, s.region, "s3")
    signature = hmac.new(
        signing_key, string_to_sign.encode("utf-8"), hashlib.sha256,
    ).hexdigest()

    authorization = (
        f"AWS4-HMAC-SHA256 Credential={s.access_key_id}/{credential_scope}, "
        f"SignedHeaders={signed_headers}, Signature={signature}"
    )

    # Wire headers: omit `host` (HTTP libs add it from the URL), keep the
    # rest in the exact form we signed.
    out_headers = {
        "x-amz-content-sha256": payload_hash,
        "x-amz-date": amz_date,
        "Authorization": authorization,
    }
    if s.session_token:
        out_headers["x-amz-security-token"] = s.session_token
    return url, out_headers


def _encode_segments(key: str) -> str:
    """URL-encode each path segment per AWS canonical-URI rules.

    AWS requires per-segment percent-encoding of every char except the
    unreserved set; the `/` between segments must NOT be encoded.
    """
    return "/".join(quote(seg, safe="") for seg in key.split("/"))


def _derive_signing_key(
    secret: str, date_stamp: str, region: str, service: str,
) -> bytes:
    def _hmac(k: bytes, msg: str) -> bytes:
        return hmac.new(k, msg.encode("utf-8"), hashlib.sha256).digest()

    k_date = _hmac(("AWS4" + secret).encode("utf-8"), date_stamp)
    k_region = _hmac(k_date, region)
    k_service = _hmac(k_region, service)
    return _hmac(k_service, "aws4_request")


__all__ = ["upload", "S3Config"]
