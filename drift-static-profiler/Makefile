# =============================================================================
# drift-static-profiler — language-agnostic static call-tree analyzer
#
# === The 3 commands you'll actually use ===
#   make            # ASCII analyzer output across every fixture (no npm needed)
#   make test       # run the integration test suite
#   make viewer     # launch the React viewer at :5180 (run `make refresh` first
#                   # if you've changed the profiler or fixture sources)
#
# Everything else (per-language demos/tests, fixture refresh, schema, cleanup)
# is below; see `make help` for the full list.
#
# Help is auto-generated: add `## doc string` after any target ':' and it shows
# up in `make help`. `### Section name` lines start a new help section.
# =============================================================================

# --- Config -------------------------------------------------------------------

BIN          := ./target/debug/drift-static-profiler
RELEASE_BIN  := ./target/release/drift-static-profiler
VIEWER_DIR   := viewer
VIEWER_PORT  := 5180
FIXTURE_OUT  := $(VIEWER_DIR)/public/fixtures
# User scans (one JSON per scanned folder, named after the folder's basename)
# land here. The viewer auto-discovers them via scans/index.json. The whole
# scans/ directory is gitignored — each contributor regenerates locally.
SCANS_OUT    := $(FIXTURE_OUT)/scans

# Fixture catalog — one row per fixture. Add a row → automatic targets.
# Format: FIXTURE_<key> := <fixture_dir>  <entry_symbol>
FIXTURE_python := tests/fixtures/python-fastapi     create_order
FIXTURE_java   := tests/fixtures/java-spring        createOrder
FIXTURE_ts     := tests/fixtures/typescript-nestjs  create
FIXTURE_js     := tests/fixtures/javascript-express notifyDownstream

FIXTURE_KEYS   := python java ts js

UNAME_S := $(shell uname -s)

# Make cargo + cargo-installed binaries visible to recipes even on the same
# `make` run that just installed them (rustup writes to ~/.cargo/bin, which
# isn't on PATH until a new shell sources ~/.cargo/env).
export PATH := $(HOME)/.cargo/bin:$(PATH)

BLUE   := \033[1;34m
GREEN  := \033[1;32m
YELLOW := \033[1;33m
CYAN   := \033[1;36m
RESET  := \033[0m

.DEFAULT_GOAL := demo
.PHONY: demo test viewer \
        setup install install-rust install-viewer check-rust \
        build release \
        test-verbose test-python test-java test-ts list-tests \
        demo-python demo-java demo-ts demo-js \
        tags-python tags-java tags-ts tags-js \
        refresh refresh-python refresh-java refresh-ts refresh-js refresh-go refresh-rust refresh-scala refresh-kotlin refresh-insights-demo refresh-docker-app viewer-data \
        viewer-bun viewer-stop viewer-install viewer-build \
        scan scan-roots scan-no-tests scan-roots-no-tests scans-index \
        diff-demo schema-validate \
        refresh-catalogs extend-catalogs verify-catalogs list-uncategorized \
        sync-otel-registry refresh-from-upstream \
        watch clean distclean help

CATALOG_DIR    := src/research_classefiers+categories
CATALOG_SCRIPT := $(CATALOG_DIR)/build_otel_registry.py
CATALOG_FILES  := $(CATALOG_DIR)/python.json \
                  $(CATALOG_DIR)/javascript.json \
                  $(CATALOG_DIR)/java.json \
                  $(CATALOG_DIR)/go.json \
                  $(CATALOG_DIR)/rust.json \
                  $(CATALOG_DIR)/scala.json \
                  $(CATALOG_DIR)/receiver_patterns.json \
                  $(CATALOG_DIR)/unambiguous_methods.json

### Tier 1 (the 3 you'll use)

demo: install build demo-python demo-java demo-ts demo-js ## ① ASCII analyzer output for every fixture

test: check-rust ## ② Run the integration test suite
	@printf "$(BLUE)▶$(RESET) cargo test --test integration\n"
	@cargo test --test integration

viewer: install-viewer viewer-stop ## ③ Start the React viewer at :5180 (uses existing fixtures; `make refresh` to regenerate)
	@printf "$(BLUE)▶$(RESET) Vite dev server (npm) → http://localhost:$(VIEWER_PORT)/\n"
	@cd $(VIEWER_DIR) && npm run dev -- --port $(VIEWER_PORT)

### Setup

setup: install-rust install-viewer ## One-time machine setup (rust + viewer npm deps)
	@printf "$(GREEN)✓$(RESET) drift-static-profiler is ready — try: make\n"

install: check-rust install-viewer ## Ensure deps are present (called by demo/viewer)

install-rust: ## Install rustup (writes to ~/.cargo, ~/.rustup, ~/.zshenv)
	@if command -v cargo >/dev/null 2>&1; then \
		printf "$(GREEN)✓$(RESET) rust already installed: $$(cargo --version)\n"; \
	else \
		printf "$(BLUE)▶$(RESET) installing rustup (default profile, stable toolchain)\n"; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default; \
		printf "$(YELLOW)!$(RESET) open a new terminal or: source \$$HOME/.cargo/env\n"; \
	fi

check-rust: ## Fail fast if cargo isn't on PATH
	@command -v cargo >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) Rust not installed. Run: make install-rust\n"; exit 1; }

install-viewer: $(VIEWER_DIR)/node_modules/.package-lock.json ## npm install in viewer/ if needed

$(VIEWER_DIR)/node_modules/.package-lock.json: $(VIEWER_DIR)/package.json $(VIEWER_DIR)/package-lock.json
	@printf "$(BLUE)▶$(RESET) installing JS deps in $(VIEWER_DIR)/\n"
	@cd $(VIEWER_DIR) && npm install
	@touch $@

### Build

build: check-rust ## cargo build (debug)
	@cargo build

release: check-rust ## cargo build --release
	@cargo build --release

$(BIN): build

### Test variants

test-verbose: ## All integration tests with --nocapture (prints tree per test)
	@cargo test --test integration -- --nocapture --test-threads=1

test-python: ## Only python_* tests
	@cargo test --test integration python_ -- --nocapture --test-threads=1

test-java: ## Only java_* tests
	@cargo test --test integration java_ -- --nocapture --test-threads=1

test-ts: ## Only typescript_* tests
	@cargo test --test integration typescript_ -- --nocapture --test-threads=1

list-tests: ## Enumerate test names
	@cargo test --test integration -- --list

### ASCII demos

demo-python: $(BIN) ## Python FastAPI fixture
	@printf "\n$(CYAN)=== PYTHON / FastAPI — POST /orders ===$(RESET)\n"
	@$(BIN) analyze $(word 1,$(FIXTURE_python)) --entry $(word 2,$(FIXTURE_python)) --no-accessors

demo-java: $(BIN) ## Java Spring Boot fixture
	@printf "\n$(CYAN)=== JAVA / Spring Boot — POST /orders ===$(RESET)\n"
	@$(BIN) analyze $(word 1,$(FIXTURE_java)) --entry $(word 2,$(FIXTURE_java)) --no-accessors

demo-ts: $(BIN) ## TypeScript NestJS fixture
	@printf "\n$(CYAN)=== TYPESCRIPT / NestJS — POST /orders ===$(RESET)\n"
	@$(BIN) analyze $(word 1,$(FIXTURE_ts)) --entry $(word 2,$(FIXTURE_ts)) --no-accessors

demo-js: $(BIN) ## JavaScript Express + Mongoose fixture
	@printf "\n$(CYAN)=== JAVASCRIPT / Express + Mongoose ===$(RESET)\n"
	@$(BIN) analyze $(word 1,$(FIXTURE_js)) --entry $(word 2,$(FIXTURE_js)) --no-accessors

### Raw tag dumps

tags-python: $(BIN) ## Raw tree-sitter tags for the Python fixture
	@$(BIN) tags $(word 1,$(FIXTURE_python))

tags-java: $(BIN) ## Raw tree-sitter tags for the Java fixture
	@$(BIN) tags $(word 1,$(FIXTURE_java))

tags-ts: $(BIN) ## Raw tree-sitter tags for the TypeScript fixture
	@$(BIN) tags $(word 1,$(FIXTURE_ts))

tags-js: $(BIN) ## Raw tree-sitter tags for the JavaScript fixture
	@$(BIN) tags $(word 1,$(FIXTURE_js))

### Refresh viewer fixtures

refresh: refresh-python refresh-java refresh-ts refresh-js refresh-go refresh-rust refresh-scala refresh-kotlin refresh-insights-demo refresh-docker-app ## Rerun ALL fixtures through the profiler
	@printf "$(GREEN)✓$(RESET) all viewer fixtures regenerated from the profiler\n"

refresh-docker-app: $(BIN) ## Regenerate the docker-app fixture (Dockerfile + docker-compose)
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/docker-app \
	  --name docker-app \
	  --out-dir $(FIXTURE_OUT) \
	  --include-private \
	  --min-reach 1 > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/docker-app.json\n"

refresh-insights-demo: $(BIN) ## Regenerate the synthetic insights-demo fixture (showcases findings)
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/insights-demo \
	  --name insights-demo \
	  --out-dir $(FIXTURE_OUT) \
	  --no-accessors > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/insights-demo.json\n"

# go-gin / rust-axum / scala-play use auto-root-discovery (no single
# --entry name fits all of them). Same pattern as refresh-insights-demo.
refresh-go: $(BIN) ## Regenerate the Go fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/go-gin \
	  --name go-gin \
	  --out-dir $(FIXTURE_OUT) \
	  --no-accessors > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/go-gin.json\n"

refresh-rust: $(BIN) ## Regenerate the Rust fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/rust-axum \
	  --name rust-axum \
	  --out-dir $(FIXTURE_OUT) \
	  --no-accessors > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/rust-axum.json\n"

refresh-scala: $(BIN) ## Regenerate the Scala fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/scala-play \
	  --name scala-play \
	  --out-dir $(FIXTURE_OUT) \
	  --no-accessors > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/scala-play.json\n"

refresh-kotlin: $(BIN) ## Regenerate the Kotlin fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze-root tests/fixtures/kotlin-ktor \
	  --name kotlin-ktor \
	  --out-dir $(FIXTURE_OUT) \
	  --no-accessors > /dev/null
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/kotlin-ktor.json\n"

refresh-python: $(BIN) ## Rerun just the Python fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze $(word 1,$(FIXTURE_python)) --entry $(word 2,$(FIXTURE_python)) --no-accessors --json \
	  > $(FIXTURE_OUT)/python-fastapi.json
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/python-fastapi.json\n"

refresh-java: $(BIN) ## Rerun just the Java fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze $(word 1,$(FIXTURE_java)) --entry $(word 2,$(FIXTURE_java)) --no-accessors --json \
	  > $(FIXTURE_OUT)/java-spring.json
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/java-spring.json\n"

refresh-ts: $(BIN) ## Rerun just the TypeScript fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze $(word 1,$(FIXTURE_ts)) --entry $(word 2,$(FIXTURE_ts)) --no-accessors --json \
	  > $(FIXTURE_OUT)/typescript-nestjs.json
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/typescript-nestjs.json\n"

refresh-js: $(BIN) ## Rerun just the JavaScript fixture
	@mkdir -p $(FIXTURE_OUT)
	@$(BIN) analyze $(word 1,$(FIXTURE_js)) --entry $(word 2,$(FIXTURE_js)) --no-accessors --json \
	  > $(FIXTURE_OUT)/javascript-express.json
	@printf "  $(BLUE)↻$(RESET) $(FIXTURE_OUT)/javascript-express.json\n"

viewer-data: refresh ## back-compat alias for `refresh`

### React viewer

viewer-install: install-viewer ## back-compat alias for `install-viewer`

viewer-bun: install-viewer viewer-stop ## Start dev server with bun on http://localhost:5180/ (uses existing fixtures)
	@command -v bun >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) bun not installed — see https://bun.sh/install\n"; exit 1; }
	@printf "$(BLUE)▶$(RESET) Vite dev server (bun) → http://localhost:$(VIEWER_PORT)/\n"
	@cd $(VIEWER_DIR) && bun run dev -- --port $(VIEWER_PORT)

viewer-stop: ## Kill whatever is listening on port 5180
	@PID=$$(lsof -ti :$(VIEWER_PORT) 2>/dev/null); \
	  if [ -n "$$PID" ]; then \
	    printf "$(BLUE)▶$(RESET) killing PID $$PID on port $(VIEWER_PORT)\n"; \
	    kill -TERM $$PID 2>/dev/null || true; \
	    sleep 0.5; \
	    kill -KILL $$PID 2>/dev/null || true; \
	  else \
	    printf "$(GREEN)✓$(RESET) nothing on port $(VIEWER_PORT)\n"; \
	  fi

viewer-build: install-viewer refresh ## Production build → viewer/dist/
	@cd $(VIEWER_DIR) && npm run build

### Scan any folder → viewer

# Point the profiler at any folder on disk and have the result show up
# in the viewer as its own entry on the landing page. Each scan writes
# `viewer/public/fixtures/scans/<name>.json`, where <name> defaults to
# the BASENAME of SCAN_PATH (so `/Users/me/ktor` → `scans/ktor.json`).
# Override with NAME=… to pick a different label/filename.
#
#   make scan /Users/me/ktor                  # → scans/ktor.json
#   make scan SCAN_PATH=../other-repo         # → scans/other-repo.json
#   make scan SCAN_PATH=./some/dir NAME=myapp # → scans/myapp.json
#
# After writing the JSON, the recipe regenerates scans/index.json — a
# small manifest the viewer fetches at boot to render one card per
# user-run scan. The whole scans/ directory is gitignored.
# Allow a positional path: `make scan /abs/path`, `make scan-roots /abs/path`,
# `make scan-no-tests /abs/path`, etc. Treat any extra goals after scan/
# scan-roots/scan-no-tests/scan-roots-no-tests as args: first one → SCAN_PATH,
# and stub them out as no-op targets so make doesn't try to build them.
ifneq ($(filter scan scan-roots scan-no-tests scan-roots-no-tests,$(MAKECMDGOALS)),)
  SCAN_POSITIONAL := $(filter-out scan scan-roots scan-no-tests scan-roots-no-tests,$(MAKECMDGOALS))
  ifneq ($(SCAN_POSITIONAL),)
    SCAN_PATH ?= $(firstword $(SCAN_POSITIONAL))
    $(eval $(SCAN_POSITIONAL):;@:)
  endif
endif

SCAN_PATH  ?=
ENTRY      ?=
# Derive NAME from SCAN_PATH's basename (last directory component, with
# any trailing slash stripped). `?=` keeps explicit NAME=… overrides.
# Examples: /Users/me/ktor → ktor ; ./code/myproj/ → myproj.
NAME       ?= $(notdir $(patsubst %/,%,$(SCAN_PATH)))

# `scan` is the "everything" target: it walks the project, auto-
# discovers every in-degree=0 symbol as a root, and disables every root
# filter (tests, private symbols, accessors, reach<2). Use this when
# you want the full picture; use `scan-roots` for the curated default
# discovery; use `scan-no-tests` for the same "everything" view but
# with test/spec/mock files dropped at the walker stage.
#
#   make scan /abs/path
#   make scan SCAN_PATH=/abs/path NAME=myproj
scan: $(BIN) ## Scan any folder with ALL root filters off (everything as roots). Curated alternative: `scan-roots`. Strict: `scan-no-tests`.
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan /abs/path [NAME=fixture_name]\n"; \
	  printf "          or: make scan SCAN_PATH=/abs/path [NAME=fixture_name]\n"; \
	  printf "   tip: this is the unfiltered scan — every in-degree=0 symbol is a root.\n"; \
	  printf "        use `make scan-roots` for the curated default (skips tests/private/accessors).\n"; \
	  exit 1; \
	fi
	@mkdir -p $(SCANS_OUT)
	@printf "$(BLUE)▶$(RESET) scanning $(SCAN_PATH) (no filters) → $(SCANS_OUT)/$(NAME).json\n"
	@$(BIN) analyze-root "$(SCAN_PATH)" \
	  --name "$(NAME)" \
	  --out-dir "$(SCANS_OUT)" \
	  --min-reach 1 \
	  --max-roots 5000 \
	  --include-tests \
	  --include-private \
	  --include-accessors \
	  --print
	@$(MAKE) --no-print-directory scans-index

# Root-level profile: auto-discover every plausible entry point in a folder
# (symbols with no in-graph caller, ranked by transitive reach) and profile
# each one. The viewer's "Roots" tab renders a sortable table; click any row
# to drill into that root's flame graph + call tree.
#
# This is the static-analysis analog of Chrome DevTools' Top-Down view,
# pprof's `top -cum`, or Speedscope's Sandwich view.
#
#   make scan-roots SCAN_PATH=/Users/me/code/myproj
#   make scan-roots SCAN_PATH=./some/dir ROOTS_NAME=myproj-roots MIN_REACH=4
#
MIN_REACH  ?= 2
MAX_ROOTS  ?= 200
# Same derivation as NAME: basename of SCAN_PATH. Override with
# ROOTS_NAME=… if you want a different filename in scans/.
ROOTS_NAME ?= $(notdir $(patsubst %/,%,$(SCAN_PATH)))

scan-roots: $(BIN) ## Auto-discover root entry points and write a multi-root profile JSON
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan-roots /abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  printf "          or: make scan-roots SCAN_PATH=/abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  exit 1; \
	fi
	@mkdir -p $(SCANS_OUT)
	@printf "$(BLUE)▶$(RESET) discovering roots in $(SCAN_PATH) → $(SCANS_OUT)/$(ROOTS_NAME).json\n"
	@$(BIN) analyze-root "$(SCAN_PATH)" \
	  --name "$(ROOTS_NAME)" \
	  --out-dir "$(SCANS_OUT)" \
	  --min-reach $(MIN_REACH) \
	  --max-roots $(MAX_ROOTS) \
	  --no-accessors \
	  --print
	@$(MAKE) --no-print-directory scans-index

# `scan-no-tests` is `scan` minus tests: same "everything as roots"
# semantics (every in-degree=0 symbol qualifies, all filters off), but
# test/spec/mock files are dropped at the WALKER stage so they don't
# enter the graph at all — not as roots, not as callees, not as
# dead_code. The walker's filter matches path segments (`tests/`,
# `__tests__/`, `spec/`, `__mocks__/`, `testdata/`) and filename
# conventions (`*.test.{ts,js}`, `*.spec.*`, `test_*.py`, `*_test.go`,
# `*Test.java`, `*Spec.scala`, `*.mock.*`).
#
#   make scan-no-tests /abs/path
#   make scan-no-tests SCAN_PATH=/abs/path NAME=myproj-prod
scan-no-tests: $(BIN) ## Like `scan` (everything as roots) but with test/spec/mock files dropped at the walker stage.
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan-no-tests /abs/path [NAME=fixture_name]\n"; \
	  printf "          or: make scan-no-tests SCAN_PATH=/abs/path [NAME=fixture_name]\n"; \
	  printf "   tip: same loose discovery as `make scan`, but test/spec/mock files are walked OUT.\n"; \
	  exit 1; \
	fi
	@mkdir -p $(SCANS_OUT)
	@printf "$(BLUE)▶$(RESET) scanning $(SCAN_PATH) (no filters, no tests) → $(SCANS_OUT)/$(NAME).json\n"
	@$(BIN) analyze-root "$(SCAN_PATH)" \
	  --name "$(NAME)" \
	  --out-dir "$(SCANS_OUT)" \
	  --min-reach 1 \
	  --max-roots 5000 \
	  --include-private \
	  --include-accessors \
	  --no-tests \
	  --print
	@$(MAKE) --no-print-directory scans-index

scan-roots-no-tests: $(BIN) ## `make scan-roots` + walker-level filter for test/spec/mock files
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan-roots-no-tests /abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  printf "          or: make scan-roots-no-tests SCAN_PATH=/abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  exit 1; \
	fi
	@mkdir -p $(SCANS_OUT)
	@printf "$(BLUE)▶$(RESET) discovering roots in $(SCAN_PATH) (no tests) → $(SCANS_OUT)/$(ROOTS_NAME).json\n"
	@$(BIN) analyze-root "$(SCAN_PATH)" \
	  --name "$(ROOTS_NAME)" \
	  --out-dir "$(SCANS_OUT)" \
	  --min-reach $(MIN_REACH) \
	  --max-roots $(MAX_ROOTS) \
	  --no-accessors \
	  --no-tests \
	  --print
	@$(MAKE) --no-print-directory scans-index

# Rebuild scans/index.json from the current contents of scans/. The viewer
# fetches this file at boot to discover user-run scans; browsers can't
# list directories so we materialize an index. Idempotent.
#
# Backed by the `regen-scans-index` subcommand in the profiler binary.
# Reads only the first 8 KB of each scan to pull `generator.source_root`,
# so per-file cost is flat regardless of scan size — flat enough that
# even a 30 GB scan in the directory adds milliseconds, not minutes.
scans-index: $(BIN) ## (internal) Rebuild viewer/public/fixtures/scans/index.json
	@$(BIN) regen-scans-index $(SCANS_OUT)

### Diff & schema

diff-demo: $(BIN) ## Generate a regression scenario and show diff output
	@printf "$(BLUE)▶$(RESET) generating baseline from python fixture\n"
	@mkdir -p /tmp/drift-diff-demo
	@$(BIN) analyze tests/fixtures/python-fastapi --entry create_order --no-accessors --json \
	  > /tmp/drift-diff-demo/baseline.json
	@cp -r tests/fixtures/python-fastapi /tmp/drift-diff-demo/regressed 2>/dev/null || \
	  (rm -rf /tmp/drift-diff-demo/regressed && cp -r tests/fixtures/python-fastapi /tmp/drift-diff-demo/regressed)
	@printf 'from .models import Order\n\nclass OrderRepository:\n    def __init__(self, session):\n        self.session = session\n\n    def save(self, order):\n        for sub in [order]:\n            self.session.add(sub)\n            self.session.commit()\n        return order\n\n    def find_by_id(self, oid):\n        return self.session.query(Order).filter(Order.id == oid).first()\n' \
	  > /tmp/drift-diff-demo/regressed/app/repositories.py
	@$(BIN) analyze /tmp/drift-diff-demo/regressed --entry create_order --no-accessors --json \
	  > /tmp/drift-diff-demo/current.json
	@printf "\n$(BLUE)▶$(RESET) diff output (expect: NEW SMELL n_plus_one_risk)\n"
	@$(BIN) diff /tmp/drift-diff-demo/baseline.json /tmp/drift-diff-demo/current.json --no-fail

schema-validate: refresh ## Validate viewer/public/fixtures/*.json against schema/profile.schema.json
	@python3 -c "import jsonschema" 2>/dev/null || { \
		printf "$(YELLOW)!$(RESET) installing jsonschema...\n"; pip3 install --quiet jsonschema; }
	@python3 -c "import json, glob, sys; \
from jsonschema import Draft202012Validator; \
schema = json.load(open('schema/profile.schema.json')); \
v = Draft202012Validator(schema); \
files = sorted(glob.glob('$(FIXTURE_OUT)/*.json')); \
ok = True; \
[(\
  (errors := sorted(v.iter_errors(json.load(open(f))), key=lambda e: list(e.path))) and (\
    print(f'  ✗ {f}: {len(errors)} error(s)'), [print(f'      • {\"/\".join(str(p) for p in e.path) or \"(root)\"}: {e.message}') for e in errors[:5]], (ok := False) \
  ) or print(f'  ✓ {f}')) for f in files]; \
sys.exit(0 if ok else 1)"

### Classification catalogs
#
# The classifier in src/categories.rs is data-driven: it loads JSONs from
# src/research_classefiers+categories/ at compile time (`include_str!`).
# Those JSONs are extended in place by `build_otel_registry.py`, which
# scans the 488 YAML files in src/categories-opentelemetry-classefiers/
# (the OpenTelemetry registry snapshot) and merges new module → category
# entries, deduping by module name. Existing entries always win on
# conflict — the script is strictly additive.
#
# Where to add a new data source:
#
#   1. More YAMLs (same `instrumentation-<lang>-<lib>.yml` naming):
#        make extend-catalogs SRC=/abs/path/to/your-yaml-dir
#      The script merges those into the per-lang JSONs alongside the
#      default OTel registry.
#
#   2. Direct module → category pairs (no YAML wrapping):
#        edit src/research_classefiers+categories/module_overrides.json
#      Rebuild the crate to pick them up. This file is for cross-language
#      hand-curated entries; per-language JSONs are for OTel-derived data.
#
#   3. Receiver patterns / unambiguous methods:
#        edit receiver_patterns.json / unambiguous_methods.json directly,
#      or extend the CURATED_METHODS dict in build_otel_registry.py and
#      re-run `make refresh-catalogs`.

refresh-catalogs: ## Merge default OTel registry YAMLs into per-lang catalogs (idempotent)
	@printf "$(BLUE)▶$(RESET) merging OTel registry → $(CATALOG_DIR)/*.json\n"
	@cd $(CATALOG_DIR) && python3 build_otel_registry.py

# Usage: make extend-catalogs SRC=/abs/path/to/yaml-dir
extend-catalogs: ## Also merge from SRC=<dir of instrumentation-*.yml files>
	@if [ -z "$(SRC)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make extend-catalogs SRC=/abs/path/to/yaml-dir\n"; \
	  printf "          (the dir should contain instrumentation-<lang>-<lib>.yml files)\n"; \
	  exit 1; fi
	@if [ ! -d "$(SRC)" ]; then \
	  printf "$(YELLOW)!$(RESET) not a directory: $(SRC)\n"; exit 1; fi
	@printf "$(BLUE)▶$(RESET) merging default OTel + $(SRC) → $(CATALOG_DIR)/*.json\n"
	@cd $(CATALOG_DIR) && python3 build_otel_registry.py --source "$(SRC)"

verify-catalogs: ## Run script twice; second pass must add 0 (idempotency proof)
	@printf "$(BLUE)▶$(RESET) idempotency check — running script twice\n"
	@cd $(CATALOG_DIR) && python3 build_otel_registry.py > /tmp/drift-catalog-run1.log 2>&1
	@cd $(CATALOG_DIR) && python3 build_otel_registry.py > /tmp/drift-catalog-run2.log 2>&1
	@if grep -q "GRAND_TOTAL_ADDED=0" /tmp/drift-catalog-run2.log; then \
	  printf "$(GREEN)✓$(RESET) idempotent — second run added 0\n"; \
	  tail -8 /tmp/drift-catalog-run2.log; \
	else \
	  printf "$(YELLOW)!$(RESET) NOT idempotent — second run added entries:\n"; \
	  grep "GRAND_TOTAL_ADDED\|added=" /tmp/drift-catalog-run2.log; \
	  exit 1; fi

OTEL_REGISTRY_DIR := src/categories-opentelemetry-classefiers
SYNC_SCRIPT       := $(CATALOG_DIR)/sync_otel_registry.py

# Optional knobs (override on the command line):
#   make sync-otel-registry PRUNE=1     # also delete local files not upstream
#   make sync-otel-registry DRY=1       # show changes without writing
#   make sync-otel-registry BRANCH=v1.2 # sync from a non-main branch
#   make sync-otel-registry LIMIT=20    # only fetch first 20 (testing)
PRUNE  ?=
DRY    ?=
BRANCH ?= main
LIMIT  ?=

sync-otel-registry: ## Re-download OTel registry YAMLs from upstream (PRUNE=1 DRY=1 BRANCH=... LIMIT=N)
	@printf "$(BLUE)▶$(RESET) syncing OTel registry → $(OTEL_REGISTRY_DIR)\n"
	@python3 $(SYNC_SCRIPT) \
	  $(if $(PRUNE),--prune,) \
	  $(if $(DRY),--dry-run,) \
	  $(if $(BRANCH),--branch $(BRANCH),) \
	  $(if $(LIMIT),--limit $(LIMIT),)

refresh-from-upstream: sync-otel-registry refresh-catalogs ## sync OTel registry + re-merge into catalogs
	@printf "$(GREEN)✓$(RESET) catalogs refreshed from upstream OTel registry\n"

list-uncategorized: ## Show every OTel library the script couldn't categorize (dry-run)
	@cd $(CATALOG_DIR) && OUT=$$(python3 build_otel_registry.py --list-uncategorized --dry-run); \
	  if printf "%s" "$$OUT" | grep -q "Uncategorized libraries"; then \
	    printf "%s\n" "$$OUT" | sed -n '/Uncategorized libraries/,$$p'; \
	  else \
	    UNCAT=$$(printf "%s" "$$OUT" | sed -n 's/.*uncategorized: *//p' | head -1); \
	    printf "$(GREEN)✓$(RESET) all libraries categorized (uncategorized=$$UNCAT)\n"; \
	  fi

### Misc

watch: ## Re-run tests on file change (needs cargo-watch)
	@cargo watch -x 'test --test integration -- --nocapture --test-threads=1'

clean: ## cargo clean + drop viewer build output
	@cargo clean
	@rm -rf $(VIEWER_DIR)/dist

distclean: clean ## Also drop viewer/node_modules (full reset)
	@rm -rf $(VIEWER_DIR)/node_modules

help: ## Show this help (auto-generated from inline doc strings)
	@printf "\n$(BLUE)drift-static-profiler — make targets$(RESET)\n"
	@awk 'BEGIN { FS = ":.*## " } \
		/^### / { printf "\n  $(GREEN)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_.-]+:.*## / { printf "    $(BLUE)%-18s$(RESET) %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@printf "\n"
