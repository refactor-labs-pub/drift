# =============================================================================
# drift-static-profiler — language-agnostic static call-tree analyzer
#
# === The 3 commands you'll actually use ===
#   make            # ASCII analyzer output across every fixture (no npm needed)
#   make test       # run the integration test suite
#   make viewer     # refresh fixtures, then launch the React viewer at :5180
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
        refresh refresh-python refresh-java refresh-ts refresh-js viewer-data \
        viewer-bun viewer-stop viewer-install viewer-build \
        scan scan-roots \
        diff-demo schema-validate \
        watch clean distclean help

### Tier 1 (the 3 you'll use)

demo: install build demo-python demo-java demo-ts demo-js ## ① ASCII analyzer output for every fixture

test: check-rust ## ② Run the integration test suite
	@printf "$(BLUE)▶$(RESET) cargo test --test integration\n"
	@cargo test --test integration

viewer: install refresh viewer-stop ## ③ Refresh fixtures, then start the React viewer at :5180
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

refresh: refresh-python refresh-java refresh-ts refresh-js ## Rerun ALL fixtures through the profiler
	@printf "$(GREEN)✓$(RESET) all viewer fixtures regenerated from the profiler\n"

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

viewer-bun: install-viewer refresh viewer-stop ## Start dev server with bun on http://localhost:5180/
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

### Custom scan (any path → viewer)

# Point the profiler at any folder on disk and have the result show up in the
# viewer as the "Custom Scan" fixture. Override SCAN_PATH, ENTRY, and
# (optionally) NAME on the command line.
#
#   make scan SCAN_PATH=/Users/me/code/myproj ENTRY=handleRequest
#   make scan SCAN_PATH=../other-repo         ENTRY="createOrder updateOrder"
#   make scan SCAN_PATH=./some/dir            ENTRY=main NAME=myproj
#
# ENTRY accepts a single name OR a space-separated list (quote the value).
# Then `make viewer` (or refresh your open tab) and choose "Custom Scan".
# Allow a positional path: `make scan-roots /abs/path` or `make scan /abs/path ...`
# Treat any extra goals after scan/scan-roots as args: first one → SCAN_PATH,
# and stub them out as no-op targets so make doesn't try to build them.
ifneq ($(filter scan scan-roots,$(MAKECMDGOALS)),)
  SCAN_POSITIONAL := $(filter-out scan scan-roots,$(MAKECMDGOALS))
  ifneq ($(SCAN_POSITIONAL),)
    SCAN_PATH ?= $(firstword $(SCAN_POSITIONAL))
    $(eval $(SCAN_POSITIONAL):;@:)
  endif
endif

SCAN_PATH  ?=
ENTRY      ?=
NAME       ?= custom

scan: $(BIN) ## Analyze any folder and write it as viewer/public/fixtures/<NAME>.json
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan /abs/path ENTRY=symName [NAME=fixture_name]\n"; \
	  printf "          or: make scan SCAN_PATH=/abs/path ENTRY=symName [NAME=fixture_name]\n"; \
	  printf "   tip: ENTRY=\"foo bar baz\" passes multiple entry points\n"; \
	  exit 1; \
	fi
	@ENTRIES=""; \
	  for e in $(ENTRY); do ENTRIES="$$ENTRIES --entry $$e"; done; \
	  printf "$(BLUE)▶$(RESET) scanning $(SCAN_PATH) → $(FIXTURE_OUT)/$(NAME).json\n"; \
	  $(BIN) scan "$(SCAN_PATH)" $$ENTRIES --name "$(NAME)" --out-dir "$(FIXTURE_OUT)" --no-accessors

# Root-level profile: auto-discover every plausible entry point in a folder
# (symbols with no in-graph caller, ranked by transitive reach) and profile
# each one. The viewer's "Roots" tab renders a sortable table; click any row
# to drill into that root's flame graph + call tree.
#
# This is the static-analysis analog of Chrome DevTools' Top-Down view,
# pprof's `top -cum`, or Speedscope's Sandwich view.
#
#   make scan-roots SCAN_PATH=/Users/me/code/myproj
#   make scan-roots SCAN_PATH=./some/dir NAME=myproj-roots MIN_REACH=4
#
MIN_REACH  ?= 2
MAX_ROOTS  ?= 200
ROOTS_NAME ?= roots

scan-roots: $(BIN) ## Auto-discover root entry points and write a multi-root profile JSON
	@if [ -z "$(SCAN_PATH)" ]; then \
	  printf "$(YELLOW)!$(RESET) usage: make scan-roots /abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  printf "          or: make scan-roots SCAN_PATH=/abs/path [ROOTS_NAME=name] [MIN_REACH=N] [MAX_ROOTS=N]\n"; \
	  exit 1; \
	fi
	@printf "$(BLUE)▶$(RESET) discovering roots in $(SCAN_PATH) → $(FIXTURE_OUT)/$(ROOTS_NAME).json\n"
	@$(BIN) analyze-root "$(SCAN_PATH)" \
	  --name "$(ROOTS_NAME)" \
	  --out-dir "$(FIXTURE_OUT)" \
	  --min-reach $(MIN_REACH) \
	  --max-roots $(MAX_ROOTS) \
	  --no-accessors \
	  --print

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
