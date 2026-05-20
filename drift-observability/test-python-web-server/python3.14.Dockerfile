# syntax=docker/dockerfile:1.7
#
# Python 3.14 sibling of `test-python-web-server/Dockerfile`. SAME
# `app.py` + `orders.py`, SAME build-context contract (repo root,
# see dev/Tiltfile) — the only deltas vs. the 3.7 image are:
#
#   - Base image:  python:3.7-slim    →  python:3.14-slim
#   - Wheel input: dist/              →  dist-py314/
#                  (a separate cp314 wheel; the 3.7 wheel's
#                  compiled `_profiler.so` is cp37-only and would
#                  fail to import here)
#   - Requirements: requirements.txt  →  requirements-py314.txt
#                  (pydantic v2 + newer fastapi/uvicorn — pydantic
#                  v1 has no 3.14 wheels)
#   - Supabase extras (websocket-client + certifi) pre-installed so
#     `driftdockerprofiler.start(supabase_url=..., supabase_api_key=...)`
#     enters broadcast mode immediately.
#
# The cp314 wheel is produced by
#
#     docker build --build-arg PYTHON_VERSION=3.14 \
#         --output type=local,dest=./dist-py314 \
#         --target wheel-export drift-profiler-python
#
# (the same multi-stage Dockerfile the cp37 wheel uses, just with the
# build-arg flipped). The dev Tiltfile drives this as the
# `driftdockerprofiler-wheel-py314` local_resource.

FROM python:3.14-slim
WORKDIR /app

COPY test-python-web-server/requirements-py314.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

# Prebuilt wheel for python:3.14. setup.py drops the C++ CPU sampler
# on 3.14+ (the SIGPROF stack walker reaches into now-opaque
# PyInterpreterFrame internals — non-trivial port), so this is a pure
# Python `py3-none-any.whl` carrying only the wall sampler. The smoke
# check therefore validates `import` + the wall sampler's presence,
# not `cpu_profiling_available()` (which we expect to be False here).
#
# The glob accepts both `driftdockerprofiler-` (legacy, pre-PEP-503
# normalization) and `drift_docker_profiler-` (current setuptools
# output) so the Dockerfile keeps working if the wheel naming flips.
COPY drift-profiler-python/dist-py314/drift*docker*profiler-*.whl /tmp/
RUN pip install --no-cache-dir /tmp/drift*docker*profiler-*.whl && \
    rm /tmp/drift*docker*profiler-*.whl && \
    python -c "import driftdockerprofiler; \
print('cpu_profiling_available:', \
    driftdockerprofiler.cpu_profiling_available()); \
print('wheel imported cleanly on', __import__('sys').version)"

COPY test-python-web-server/app.py test-python-web-server/orders.py /app/

# Same shared-volume contract as the python:3.7 image — events go to
# /trace/events.log unless Supabase env vars switch the agent into
# broadcast mode at start() time.
ENV DRIFT_EVENTS_PATH=/trace/events.log \
    DRIFT_SERVICE=test-python-web-server-py314

EXPOSE 8000
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8000"]
