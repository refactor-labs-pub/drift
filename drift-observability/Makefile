# =============================================================================
# drift-observability — repo-level orchestration
#
# === The handful you'll actually use ===
#   make help                            # this menu (auto-generated)
#   make up                              # boot the local cluster via Tilt + minikube
#   make test-driftdockerprofiler        # host-side pytest for driftdockerprofiler
#   make export-driftdockerprofiler      # Dockerized wheel build → dist/driftdockerprofiler/<v>/
#                                        #   produces a wheel WITH the C++ ext baked in
#   make test-export-driftdockerprofiler # install the exported wheel in a clean
#                                        #   container and assert the C++ ext loads end-to-end
#   make smoke-driftdockerprofiler       # build the smoke-test docker image and run it
#
# Cluster targets delegate to dev/Makefile. The driftdockerprofiler agent
# (wall + CPU stack sampler, ships a compiled .so) lives in
# drift-profiler-python/ and exports a pip-installable wheel into
# dist/driftdockerprofiler/<version>/.
#
# Help is auto-generated: add `## doc string` after any target ':' and it
# shows up in `make help`. `### Section name` lines start a new help section.
# =============================================================================

# --- Config -------------------------------------------------------------------

# driftdockerprofiler — the wall + CPU stack-sampling agent.
# Build image MUST be a full `python:VER` image (not -slim) — the
# wheel embeds a compiled C++ extension and slim images skip gcc/g++.
# Test image is -slim because the wheel ships the .so prebuilt.
DRIFTDOCKER_DIR        := drift-profiler-python
DRIFTDOCKER_OUT        := dist/driftdockerprofiler
DRIFTDOCKER_BUILD_IMG  ?= python:3.7
DRIFTDOCKER_TEST_IMG   ?= python:3.7-slim

BLUE   := \033[1;34m
GREEN  := \033[1;32m
YELLOW := \033[1;33m
CYAN   := \033[1;36m
RED    := \033[1;31m
DIM    := \033[2m
BOLD   := \033[1m
RESET  := \033[0m

.DEFAULT_GOAL := help
.PHONY: help \
        install up down stop reset-data nuke \
        wheel-driftdockerprofiler test-driftdockerprofiler test-cpp-driftdockerprofiler \
        export-driftdockerprofiler export-driftdockerprofiler-clean \
        test-export-driftdockerprofiler smoke-driftdockerprofiler \
        export-driftdockerprofiler-readme \
        images images-app images-obs \
        e2e-driftdockerprofiler \
        live-supabase-driftdockerprofiler

# `live-supabase-driftdockerprofiler` reads SUPABASE_URL,
# SUPABASE_REALTIME_API_KEY, SUPABASE_REALTIME_CHANNEL from
# `drift-observability/.env` (gitignored — see `.env.example`).
# The Python script does the loading; the Makefile target is just
# a runner.

### Tier 1 (the ones you'll use)

help: ## Show this help (auto-generated from inline doc strings)
	@printf "\n$(BLUE)drift-observability — make targets$(RESET)\n"
	@awk 'BEGIN { FS = ":.*## " } \
		/^### / { printf "\n  $(GREEN)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_.-]+:.*## / { printf "    $(BLUE)%-28s$(RESET) %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@printf "\n  $(GREEN)Sub-project Makefiles$(RESET)\n"
	@printf "    $(BLUE)%-28s$(RESET) %s\n" "make -C dev help"  "cluster ops (minikube + tilt + helm)"
	@printf "\n"

### Cluster ops (delegate to dev/Makefile)

install: ## brew install minikube, kubectl, tilt, helm, k9s
	@$(MAKE) --no-print-directory -C dev install

up: ## Start minikube and run `tilt up`
	@$(MAKE) --no-print-directory -C dev up

down: ## `tilt down` (cluster stays up)
	@$(MAKE) --no-print-directory -C dev down

stop: ## Stop minikube (keeps state)
	@$(MAKE) --no-print-directory -C dev stop

reset-data: ## Reset persistent data volumes
	@$(MAKE) --no-print-directory -C dev reset-data

nuke: ## tilt down + minikube delete
	@$(MAKE) --no-print-directory -C dev nuke

### driftdockerprofiler — local dev loop (host pytest)

test-driftdockerprofiler: ## Run driftdockerprofiler Python unit tests on the host (uses your system python3)
	@printf "$(BLUE)▶$(RESET) pytest driftdockerprofiler (host)\n"
	@cd $(DRIFTDOCKER_DIR) && PYTHONPATH=. python3 -m pytest tests/ --ignore=tests/cpp -q

# C++ unit tests for the native sampler internals (clock, stacktraces).
# Run inside the build image (python:3.11) so gcc/g++ + Python.h are
# both available; the test binary executes during `docker build`, so
# failures abort the build.
test-cpp-driftdockerprofiler: ## Compile + run the C++ unit tests inside Docker (verifies the .cc files we ship)
	@printf "$(BLUE)▶$(RESET) building + running C++ tests inside $(DRIFTDOCKER_BUILD_IMG)\n"
	@cd $(DRIFTDOCKER_DIR) && docker build --target cpp-test --progress=plain -t driftdockerprofiler-cpp-test . 2>&1 | grep -E "^#11|Running |PASS|FAIL|Cases:|Assertion" | sed 's/^#11 [0-9.]* //' ; \
	  if docker image inspect driftdockerprofiler-cpp-test >/dev/null 2>&1; then \
	    printf "$(GREEN)✓$(RESET) C++ tests passed\n" ; \
	  else \
	    printf "$(RED)✗$(RESET) C++ tests FAILED — see output above\n" ; \
	    exit 1 ; \
	  fi

### driftdockerprofiler — production export (Dockerized, ships C++ extension)
#
# `export-driftdockerprofiler` is the one-shot production build:
#   - Builds the wheel inside $(DRIFTDOCKER_BUILD_IMG) (gcc/g++ present),
#     so the .so for the SIGPROF-based CPU sampler gets baked in.
#   - Drops everything into dist/driftdockerprofiler/<version>/ — sits
#     right next to the repo root and contains:
#       driftdockerprofiler-<v>-cpXX-cpXX-linux_<arch>.whl   # pip target
#       README.md                                            # copied verbatim
#       INSTALL.md                                           # generated pip + poetry recipes
#       SHA256SUMS                                           # checksum manifest
#   - Pretty-renders README.md after the build so you can eyeball the docs.
#
# `test-export-driftdockerprofiler` then exercises the wheel in a clean
# `python:3.11-slim` container — pip-installs it, asserts the C++
# extension is loadable (`cpu_profiling_available() is True`), starts
# the agent, burns CPU, stops, and validates the on-the-wire event
# schema (cpu + memory_bytes included). Hermetic; touches nothing on
# the host. The C++ extension is the most fragile thing in this
# project, so this is the bar for "the wheel actually works".
#
# Idempotent — re-runs wipe the version directory before rebuilding.

wheel-driftdockerprofiler: ## Quick local wheel build (still via Docker — C++ ext needs gcc/g++) → drift-profiler-python/dist/
	@printf "$(BLUE)▶$(RESET) building driftdockerprofiler wheel via Docker ($(DRIFTDOCKER_BUILD_IMG))\n"
	# mkdir BEFORE rm so the directory never disappears mid-build. Tilt
	# can fire the test-python-web-server image build in parallel with
	# this target; if `dist/` is briefly gone, Docker's COPY fails with
	# `lstat /drift-profiler-python/dist: no such file or directory`.
	# rm -f only the wheels keeps the directory itself stable.
	@cd $(DRIFTDOCKER_DIR) && mkdir -p dist && rm -f dist/*.whl && \
	  docker build --output type=local,dest=./dist --target wheel-export . ; \
	  printf "$(GREEN)✓$(RESET) artifacts:\n" ; \
	  ls -1 dist | sed 's/^/    /'

export-driftdockerprofiler: ## Dockerized build → dist/driftdockerprofiler/<v>/ (.whl + README + INSTALL.md + SHA256SUMS)
	@VERSION=$$(awk -F"'" '/^__version__/ {print $$2; exit}' $(DRIFTDOCKER_DIR)/driftdockerprofiler/__version__.py); \
	if [ -z "$$VERSION" ]; then \
	  printf "$(RED)✗$(RESET) couldn't read version from $(DRIFTDOCKER_DIR)/driftdockerprofiler/__version__.py\n"; exit 1; \
	fi; \
	OUT="$(DRIFTDOCKER_OUT)/$$VERSION"; \
	printf "\n$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "$(BLUE)driftdockerprofiler / export$(RESET) — building v$$VERSION via Docker\n"; \
	printf "$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "\n$(CYAN)[1/5]$(RESET) pre-flight: docker available?\n"; \
	docker version >/dev/null 2>&1 || { printf "$(RED)✗$(RESET) Docker daemon not reachable\n"; exit 1; }; \
	printf "  $(GREEN)✓$(RESET) docker ok\n"; \
	printf "\n$(CYAN)[2/5]$(RESET) preparing output dir $(CYAN)$$OUT$(RESET)\n"; \
	rm -rf "$$OUT"; mkdir -p "$$OUT"; \
	printf "  $(GREEN)✓$(RESET) clean\n"; \
	printf "\n$(CYAN)[3/5]$(RESET) building wheel + C++ extension inside $(CYAN)$(DRIFTDOCKER_BUILD_IMG)$(RESET)\n"; \
	( cd $(DRIFTDOCKER_DIR) && \
	  docker build --build-arg PYTHON_VERSION=3.11 \
	    --output type=local,dest="$$OLDPWD/$$OUT" \
	    --target wheel-export . ) \
	  || { printf "$(RED)✗$(RESET) docker build failed\n"; exit 1; }; \
	printf "  $(GREEN)✓$(RESET) wheel built (C++ extension baked in)\n"; \
	printf "\n$(CYAN)[4/5]$(RESET) copying README + writing INSTALL.md + mirroring to $(CYAN)$(DRIFTDOCKER_DIR)/dist/$(RESET)\n"; \
	cp $(DRIFTDOCKER_DIR)/README.md "$$OUT/README.md"; \
	[ -f $(DRIFTDOCKER_DIR)/LICENSE ] && cp $(DRIFTDOCKER_DIR)/LICENSE "$$OUT/LICENSE" || true; \
	WHEEL=$$(ls "$$OUT"/*.whl 2>/dev/null | head -1 | xargs -n1 basename); \
	rm -rf $(DRIFTDOCKER_DIR)/dist; mkdir -p $(DRIFTDOCKER_DIR)/dist; \
	cp "$$OUT/$$WHEEL" $(DRIFTDOCKER_DIR)/dist/; \
	printf "  $(GREEN)✓$(RESET) wheel mirrored to $(CYAN)$(DRIFTDOCKER_DIR)/dist/$$WHEEL$(RESET)\n"; \
	{ \
	  printf '# Installing driftdockerprofiler v%s\n\n' "$$VERSION"; \
	  printf 'This directory ships a built wheel (`%s`) including the\n' "$$WHEEL"; \
	  printf 'compiled `driftdockerprofiler._profiler` C++ extension (SIGPROF CPU sampler).\n\n'; \
	  printf '## pip\n\n```bash\npip install ./%s\n```\n\n' "$$WHEEL"; \
	  printf '## poetry\n\n```bash\npoetry add ./%s\n```\n\n' "$$WHEEL"; \
	  printf '## Verify\n\n```bash\nshasum -a 256 -c SHA256SUMS\npython -c "import driftdockerprofiler; print(driftdockerprofiler.cpu_profiling_available())"\n```\n'; \
	} > "$$OUT/INSTALL.md"; \
	printf "  $(GREEN)✓$(RESET) README.md + INSTALL.md in place\n"; \
	printf "\n$(CYAN)[5/5]$(RESET) computing SHA256SUMS\n"; \
	( cd "$$OUT" && find . -maxdepth 1 -type f ! -name SHA256SUMS \
	    -exec shasum -a 256 {} + | sed 's|  \./|  |' > SHA256SUMS ); \
	printf "  $(GREEN)✓$(RESET) checksum manifest written\n"; \
	ABS_OUT=$$(cd "$$OUT" && pwd); \
	ABS_WHEEL="$$ABS_OUT/$$WHEEL"; \
	printf "\n$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "$(GREEN)✓ driftdockerprofiler v$$VERSION exported (built inside $(DRIFTDOCKER_BUILD_IMG))$(RESET)\n"; \
	printf "$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "  $(BOLD)Location$(RESET)  $(CYAN)$$ABS_OUT$(RESET)\n"; \
	printf "  $(BOLD)Contents$(RESET)\n"; \
	ls -1 "$$OUT" | sed 's/^/      /'; \
	printf "\n  $(BOLD)How to use it$(RESET)\n"; \
	printf "    $(YELLOW)# pip$(RESET)\n"; \
	printf "    $(CYAN)pip install $$ABS_WHEEL$(RESET)\n\n"; \
	printf "    $(YELLOW)# poetry$(RESET)\n"; \
	printf "    $(CYAN)poetry add $$ABS_WHEEL$(RESET)\n\n"; \
	printf "    $(YELLOW)# smoke-test in a clean container (verifies C++ ext loads)$(RESET)\n"; \
	printf "    $(CYAN)make test-export-driftdockerprofiler$(RESET)\n"; \
	$(MAKE) --no-print-directory export-driftdockerprofiler-readme README_PATH="$$OUT/README.md" VERSION="$$VERSION"

export-driftdockerprofiler-clean: ## Wipe dist/driftdockerprofiler/ so the next export starts fresh
	@printf "$(BLUE)▶$(RESET) rm -rf $(DRIFTDOCKER_OUT)\n"
	@rm -rf $(DRIFTDOCKER_OUT)
	@printf "$(GREEN)✓$(RESET) cleaned\n"

# `test-export-driftdockerprofiler` — installs the exported wheel in a
# fresh container (no source mounted), asserts the C++ extension loads,
# runs the agent end-to-end, and validates the on-the-wire schema
# (cpu + memory_bytes fields present). This is the gate for "the wheel
# actually works in production" — anything that breaks here is shipping
# broken bits.
test-export-driftdockerprofiler: ## Install the exported wheel in a fresh container + assert C++ ext + schema (end-to-end)
	@VERSION=$$(awk -F"'" '/^__version__/ {print $$2; exit}' $(DRIFTDOCKER_DIR)/driftdockerprofiler/__version__.py); \
	OUT="$(DRIFTDOCKER_OUT)/$$VERSION"; \
	WHEEL=$$(ls "$$OUT"/*.whl 2>/dev/null | head -1); \
	if [ -z "$$WHEEL" ]; then \
	  printf "$(YELLOW)!$(RESET) no wheel in $$OUT — running export-driftdockerprofiler first\n"; \
	  $(MAKE) --no-print-directory export-driftdockerprofiler; \
	  WHEEL=$$(ls "$$OUT"/*.whl 2>/dev/null | head -1); \
	fi; \
	WHEEL_NAME=$$(basename "$$WHEEL"); \
	SMOKE="$$OUT/.smoke_test.py"; \
	{ \
	  echo "import json, time, timeit, driftdockerprofiler"; \
	  echo "assert driftdockerprofiler.cpu_profiling_available(), \"native C++ ext missing\""; \
	  echo "driftdockerprofiler.start(service=\"wheel-smoke\", period_ms=10, duration_ms=300)"; \
	  echo "n = 0; deadline = timeit.default_timer() + 0.9"; \
	  echo "while timeit.default_timer() < deadline:"; \
	  echo "    for _ in range(10000): n += 1"; \
	  echo "driftdockerprofiler.stop()"; \
	  echo "evs = [json.loads(l) for l in open(\"/tmp/drift/events.jsonl\")]"; \
	  echo "assert evs, \"no events emitted\""; \
	  echo "REQUIRED = (\"type\",\"time\",\"service\",\"period_ns\",\"duration_ns\",\"count\",\"cpu\",\"memory_bytes\",\"frames\")"; \
	  echo "for e in evs:"; \
	  echo "    for k in REQUIRED:"; \
	  echo "        assert k in e, \"missing key %r in event %r\" % (k, e)"; \
	  echo "wall = sum(1 for e in evs if e[\"type\"] == \"wall_trace\")"; \
	  echo "cpu  = sum(1 for e in evs if e[\"type\"] == \"cpu_trace\")"; \
	  echo "print(\"OK total=\", len(evs), \" wall=\", wall, \" cpu=\", cpu)"; \
	  echo "print(\"   cpu_load=\", evs[0][\"cpu\"], \" memory_bytes=\", evs[0][\"memory_bytes\"])"; \
	  echo "print(\"   leaf frame:\", evs[0][\"frames\"][0])"; \
	} > "$$SMOKE"; \
	printf "\n$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "$(BLUE)test-export-driftdockerprofiler$(RESET) — smoke-test $$WHEEL_NAME in $(DRIFTDOCKER_TEST_IMG)\n"; \
	printf "$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	docker run --rm \
	  -v "$$PWD/$$OUT":/wheels:ro \
	  -e PIP_DISABLE_PIP_VERSION_CHECK=1 \
	  $(DRIFTDOCKER_TEST_IMG) \
	  sh -c "pip install --quiet --no-cache-dir /wheels/$$WHEEL_NAME && python /wheels/.smoke_test.py"; \
	STATUS=$$?; \
	rm -f "$$SMOKE"; \
	if [ $$STATUS -ne 0 ]; then \
	  printf "$(RED)✗$(RESET) smoke test failed — wheel is broken\n"; exit $$STATUS; \
	fi; \
	printf "\n$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "$(GREEN)✓ wheel installs, C++ extension loads, schema verified$(RESET)\n"; \
	printf "$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"

# `smoke-driftdockerprofiler` — Dockerfile-driven smoke test that lives
# INSIDE the project's Dockerfile (the multi-stage `smoke-test` target).
# Use this when you want to verify a fresh `python:3.11-slim` install
# WITHOUT first running the export pipeline.
smoke-driftdockerprofiler: ## Build the project's smoke-test docker image and run it (verifies C++ ext loads)
	@printf "$(BLUE)▶$(RESET) building smoke-test image (target=smoke-test in $(DRIFTDOCKER_DIR)/Dockerfile)\n"
	@cd $(DRIFTDOCKER_DIR) && docker build --target smoke-test -t driftdockerprofiler-smoke . ; \
	  printf "$(GREEN)✓$(RESET) image built\n"
	@printf "$(BLUE)▶$(RESET) running end-to-end smoke test inside the container\n"
	@printf "  (start agent → burn CPU → stop → read /tmp/drift/events.jsonl)\n\n"
	@docker run --rm driftdockerprofiler-smoke

# Pretty-print a README.md to stdout. Detects glow → mdcat → bat → awk fallback.
# Called by export-driftdockerprofiler with README_PATH=... VERSION=... but
# also runnable standalone: `make export-driftdockerprofiler-readme README_PATH=path/to/README.md`.
export-driftdockerprofiler-readme: ## Pretty-render a README.md (auto-detects glow/mdcat/bat, falls back to awk)
	@README_PATH=$${README_PATH:-$(DRIFTDOCKER_DIR)/README.md}; \
	VERSION=$${VERSION:-$$(awk -F"'" '/^__version__/ {print $$2; exit}' $(DRIFTDOCKER_DIR)/driftdockerprofiler/__version__.py)}; \
	if [ ! -f "$$README_PATH" ]; then \
	  printf "$(RED)✗$(RESET) README not found at $$README_PATH\n"; exit 1; \
	fi; \
	printf "\n$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"; \
	printf "$(BLUE)README$(RESET) — driftdockerprofiler v$$VERSION  $(DIM)($$README_PATH)$(RESET)\n"; \
	printf "$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n\n"; \
	if command -v glow >/dev/null 2>&1; then \
	  glow -s dark "$$README_PATH"; \
	elif command -v mdcat >/dev/null 2>&1; then \
	  mdcat "$$README_PATH"; \
	elif command -v bat >/dev/null 2>&1; then \
	  bat --style=plain --language=md --paging=never "$$README_PATH"; \
	else \
	  awk -v B="$$(printf '\033[1;34m')" -v G="$$(printf '\033[1;32m')" \
	      -v Y="$$(printf '\033[1;33m')" -v C="$$(printf '\033[1;36m')" \
	      -v D="$$(printf '\033[2m')" -v BD="$$(printf '\033[1m')" \
	      -v R="$$(printf '\033[0m')" \
	      'BEGIN { in_code = 0 } \
	       /^```/ { in_code = !in_code; printf "%s%s%s\n", D, $$0, R; next } \
	       in_code { printf "%s%s%s\n", C, $$0, R; next } \
	       /^### /  { printf "%s%s%s\n", Y, $$0, R; next } \
	       /^## /   { printf "%s%s%s\n", G, $$0, R; next } \
	       /^# /    { printf "%s%s%s%s\n", BD, B, $$0, R; next } \
	       { print }' "$$README_PATH"; \
	fi; \
	printf "\n$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n\n"

### Docker images (what Tilt also builds)

images: images-app images-obs ## Build both app + observability images

# images-app depends on the prebuilt driftdockerprofiler wheel — the
# app Dockerfile only COPYs the wheel; it doesn't compile it. Letting
# `make images-app` rebuild the wheel on demand means a fresh checkout
# can just run `make images` without remembering the build order.
images-app: wheel-driftdockerprofiler ## Build test-python-web-server:dev (depends on driftdockerprofiler wheel)
	@printf "$(BLUE)▶$(RESET) docker build test-python-web-server:dev\n"
	@docker build -f test-python-web-server/Dockerfile -t test-python-web-server:dev .
	@printf "$(GREEN)✓$(RESET) test-python-web-server:dev built\n"

images-obs: ## Build observability-server:dev
	@printf "$(BLUE)▶$(RESET) docker build observability-server:dev\n"
	@docker build -t observability-server:dev observability-server
	@printf "$(GREEN)✓$(RESET) observability-server:dev built\n"

### End-to-end (driftdockerprofiler → observability-server in Docker)

# `e2e-driftdockerprofiler` is the integration goalpost: it boots the
# test-python-web-server (profiled by driftdockerprofiler) and the
# observability-server in a docker compose stack wired through a
# shared volume, pumps HTTP traffic, then asserts:
#   - the JSONL events file at /trace/events.log fills up
#   - every event carries the new schema (cpu + memory_bytes + frames)
#   - BOTH `wall_trace` AND `cpu_trace` events appear (the C++ extension
#     fires under load — the whole point of the wheel)
#   - the SSE stream at /live_logs emits `data:` frames
# Tears the stack down on exit. Takes ~30-45 s end-to-end.

.PHONY: e2e-driftdockerprofiler
e2e-driftdockerprofiler: wheel-driftdockerprofiler ## End-to-end smoke test: app + observability-server in docker, assert schema + C++ ext + SSE
	@bash test-python-web-server/e2e/smoke_test.sh

### Supabase Realtime — live (NOT a unit test, hits the network)

# Opens a real WSS to your Supabase project, joins channel `drift-test`,
# emits 10 synthetic profiler events, and (in a second WSS) subscribes
# to the same channel so you SEE the round-trip on stdout. Requires
# `websocket-client` — installs it into the user site on demand.
live-supabase-driftdockerprofiler: ## Live round-trip to channel drift-test on Supabase (reads drift-observability/.env)
	@python3 -c "import websocket, certifi" 2>/dev/null || \
	  python3 -m pip install --quiet --user websocket-client certifi
	@python3 $(DRIFTDOCKER_DIR)/scripts/live_supabase_realtime.py
