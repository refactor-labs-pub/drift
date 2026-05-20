# syntax=docker/dockerfile:1.7
#
# Build context must be the repo root so both
# `drift-profiler-python/dist/` (the prebuilt wheel) and this directory
# are visible. See dev/Tiltfile.
#
# The driftdockerprofiler wheel is produced by
#   `make export-driftdockerprofiler`  (or the lighter `make wheel-driftdockerprofiler`)
# inside a `python:3.7` container, mirrored to
# `drift-profiler-python/dist/driftdockerprofiler-*.whl`. The wheel
# embeds a compiled C++ extension (the SIGPROF-based CPU sampler), so
# it is platform/python-version-specific — pick a Python here that
# matches the wheel's tag (we ship cp37 wheels, so the runtime image
# must be `python:3.7*`).

FROM python:3.7-slim
WORKDIR /app

COPY test-python-web-server/requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

# Prebuilt driftdockerprofiler wheel — must match the runtime Python
# ABI tag (cp37). The build job in Makefile / Tilt produces this file.
# Newer setuptools normalizes the dist name to `drift_docker_profiler`
# (underscores per PEP 503), so the glob accepts both spellings.
COPY drift-profiler-python/dist/drift*docker*profiler-*.whl /tmp/
RUN pip install --no-cache-dir /tmp/drift*docker*profiler-*.whl && \
    rm /tmp/drift*docker*profiler-*.whl && \
    python -c "import driftdockerprofiler; \
assert driftdockerprofiler.cpu_profiling_available(), \
    'native C++ extension did not load — wheel is broken'"

COPY test-python-web-server/app.py test-python-web-server/orders.py /app/

# JSONL events go here. The observability-server's tailer reads the
# same path (TRACE_PATH defaults to /trace/events.log). Mount a shared
# volume at /trace to wire them together.
ENV DRIFT_EVENTS_PATH=/trace/events.log

EXPOSE 8000
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8000"]
