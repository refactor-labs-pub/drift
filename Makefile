# =============================================================================
# drift — repo-level orchestration
#
# === The handful you'll actually use ===
#   make help                # this menu (auto-generated)
#   make install             # bootstrap Docker Model Runner + pull MODEL
#   make run                 # boot the full stack via docker compose
#   make drift-lab-export    # one-shot production build → dist/drift-lab/<v>/
#
# Sub-projects keep their own Makefiles with deeper targets:
#   - drift-lab/Makefile             (desktop app: dev, compile, ship)
#   - drift-static-profiler/Makefile (CLI analyzer + viewer)
#
# Help is auto-generated: add `## doc string` after any target ':' and it shows
# up in `make help`. `### Section name` lines start a new help section.
# =============================================================================

# --- Config -------------------------------------------------------------------

MODEL        ?= ai/gemma4:latest
CONTEXT_SIZE ?= 40192
DMR_PORT     ?= 12434

# Tauri updater signing keys (gitignored, generated locally by `cargo tauri
# signer generate`). tauri.conf.json's `plugins.updater.pubkey` is set, so
# `cargo tauri build` errors with "A public key has been found, but no
# private key" when these aren't exported. The `.pwd` companion file is
# optional — keys generated with `-w ''` (empty password) don't need it.
TAURI_SIGNING_KEY_FILE      ?= .tauri-keys/drift-lab.key
TAURI_SIGNING_KEY_PWD_FILE  ?= .tauri-keys/drift-lab.key.pwd

BLUE   := \033[1;34m
GREEN  := \033[1;32m
YELLOW := \033[1;33m
CYAN   := \033[1;36m
RED    := \033[1;31m
RESET  := \033[0m

.DEFAULT_GOAL := help
.PHONY: help \
        install check run run-llm db \
        kill-port kill-port-test kill-bun-sock kill-dev \
        dev setup \
        test test-fast test-all test-profiler test-lab test-clean test-clean-target \
        drift-lab-viewer-bundle \
        drift-lab-build drift-lab-build-release drift-lab-verify \
        drift-lab-export drift-lab-export-clean \
        drift-lab-ci-preflight

# Internal: assert the Tauri signing key exists before invoking cargo. Cheaper
# to fail here than wait for the bundle stage to hit the same wall. The key
# file is base64-wrapped — decode the first line and look for "encrypted" to
# detect when a passphrase is required (rsign/minisign tags encrypted keys as
# "rsign encrypted secret key" vs plain "rsign secret key").
define require_tauri_key
	@test -f $(TAURI_SIGNING_KEY_FILE) || { \
	  printf "$(RED)✗$(RESET) Missing $(TAURI_SIGNING_KEY_FILE) — generate with:\n"; \
	  printf "   cd drift-lab/src-tauri && cargo tauri signer generate -w ''\n"; \
	  printf "   (then move the .key / .key.pub files into .tauri-keys/)\n"; \
	  exit 1; \
	}
endef

# Internal: resolve the Tauri signing-key passphrase into $$PASS. Precedence:
#   1. $$TAURI_SIGNING_PRIVATE_KEY_PASSWORD already exported (CI, scripted use)
#   2. .tauri-keys/drift-lab.key.pwd file (gitignored convenience cache —
#      create with `echo -n "yourpass" > .tauri-keys/drift-lab.key.pwd`)
#   3. interactive prompt (default for fresh shells; only happens when the
#      key looks encrypted, so plain-text keys generated with `-w ''` skip it)
#
# Must be invoked inline with the cargo command (single shell line, joined by
# `&& \`) so $$PASS stays in scope — Make spawns a fresh /bin/sh for each
# recipe line, which would otherwise lose the variable.
#
# The `stty -echo` dance hides keystrokes during the prompt without relying
# on `read -s` (a bashism not honoured by /bin/sh on Debian where dash is
# default).
define resolve_tauri_pwd
	if [ -n "$$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" ]; then \
	  PASS="$$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"; \
	elif [ -f "$(TAURI_SIGNING_KEY_PWD_FILE)" ]; then \
	  PASS="$$(cat $(TAURI_SIGNING_KEY_PWD_FILE))"; \
	elif openssl base64 -d -A < $(TAURI_SIGNING_KEY_FILE) 2>/dev/null \
	     | head -c 80 | grep -qi "encrypted"; then \
	  printf "$(BLUE)?$(RESET) Updater key passphrase (input hidden): " >&2; \
	  stty -echo 2>/dev/null; \
	  IFS= read -r PASS; rc=$$?; \
	  stty echo 2>/dev/null; \
	  echo >&2; \
	  [ $$rc -eq 0 ] || { printf "$(RED)✗$(RESET) read aborted\n"; exit 1; }; \
	  [ -n "$$PASS" ] || { printf "$(RED)✗$(RESET) empty passphrase\n"; exit 1; }; \
	else \
	  PASS=""; \
	fi
endef

### Tier 1 (the ones you'll use)

help: ## Show this help (auto-generated from inline doc strings)
	@printf "\n$(BLUE)drift — make targets$(RESET)\n"
	@awk 'BEGIN { FS = ":.*## " } \
		/^### / { printf "\n  $(GREEN)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_.-]+:.*## / { printf "    $(BLUE)%-26s$(RESET) %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@printf "\n  $(GREEN)Sub-project Makefiles$(RESET)\n"
	@printf "    $(BLUE)%-26s$(RESET) %s\n" "make -C drift-lab help"             "desktop app targets (dev, compile, ship)"
	@printf "    $(BLUE)%-26s$(RESET) %s\n" "make -C drift-static-profiler help" "CLI analyzer + viewer targets"
	@printf "\n"

### Whole-repo bootstrap

# `make setup` is the single command a fresh clone runs to become buildable:
# every subproject's deps + the viewer dist that `drift-lab` embeds.
#
# Order is deliberate:
#   1. drift-lab/setup        → rustup, cargo-tauri, icons, npm deps for desktop-ui
#   2. drift-static-profiler/setup → npm deps for viewer/ (rust is a no-op after #1)
#   3. drift-lab-viewer-bundle    → vite build, so cargo doesn't embed the build.rs stub
#   4. action/ npm ci            → GitHub Action source (best-effort)
#   5. web-app/ bun install      → web app (best-effort; only run when bun is installed)
#
# Idempotent — re-running after a `git pull` is the recommended way to re-sync deps.
setup: ## Install everything across all subprojects (rust + tauri-cli + npm deps + viewer dist + action + web-app). Idempotent — run after fresh clone or git pull
	@printf "$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"
	@printf "$(BLUE)drift / setup$(RESET) — bootstrapping the entire repo\n"
	@printf "$(BLUE)═══════════════════════════════════════════════════════════════$(RESET)\n"

	@printf "\n$(CYAN)[1/5] drift-lab — rust + tauri-cli + icons + desktop-ui npm deps$(RESET)\n"
	@$(MAKE) --no-print-directory -C drift-lab setup

	@printf "\n$(CYAN)[2/5] drift-static-profiler — viewer npm deps$(RESET)\n"
	@$(MAKE) --no-print-directory -C drift-static-profiler setup

	@printf "\n$(CYAN)[3/5] viewer dist (so cargo embeds the real viewer, not the build.rs stub)$(RESET)\n"
	@$(MAKE) --no-print-directory drift-lab-viewer-bundle

	@printf "\n$(CYAN)[4/5] action — npm ci$(RESET)\n"
	@if command -v npm >/dev/null 2>&1; then \
	  cd action && npm ci && printf "$(GREEN)✓$(RESET) action deps installed\n"; \
	else \
	  printf "$(YELLOW)!$(RESET) npm not on PATH — skipping action/ (install Node 22+ and re-run if you need it)\n"; \
	fi

	@printf "\n$(CYAN)[5/5] web-app — bun install$(RESET)\n"
	@if command -v bun >/dev/null 2>&1; then \
	  cd web-app && bun install --frozen-lockfile && printf "$(GREEN)✓$(RESET) web-app deps installed\n"; \
	else \
	  printf "$(YELLOW)!$(RESET) bun not on PATH — skipping web-app/ (https://bun.sh/install — only needed if you work on web-app/)\n"; \
	fi

	@printf "\n$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"
	@printf "$(GREEN)✓$(RESET) setup complete\n"
	@printf "  Next: $(CYAN)make dev$(RESET) (launches the desktop app + localhost:5151 HTTP server)\n"
	@printf "        $(CYAN)make help$(RESET) (full target list)\n"
	@printf "$(GREEN)═══════════════════════════════════════════════════════════════$(RESET)\n"

### Docker Model Runner

install: ## Enable Model Runner, pull MODEL=..., set CONTEXT_SIZE=... (override on CLI)
	@docker version >/dev/null || { printf "$(RED)✗$(RESET) Docker not running\n"; exit 1; }
	@docker model version >/dev/null 2>&1 || { \
	  printf "$(RED)✗$(RESET) docker model plugin missing — update Docker Desktop to 4.41+\n"; exit 1; }
	@printf "$(BLUE)▶$(RESET) enabling Model Runner on tcp:$(DMR_PORT)\n"
	@docker desktop enable model-runner --tcp $(DMR_PORT) || true
	@printf "$(BLUE)▶$(RESET) pulling $(CYAN)$(MODEL)$(RESET)\n"
	@docker model pull $(MODEL)
	@printf "$(BLUE)▶$(RESET) setting context size to $(CONTEXT_SIZE)\n"
	@docker model configure --context-size $(CONTEXT_SIZE) $(MODEL)
	@printf "$(GREEN)✓$(RESET) ready — try: make check\n"

check: ## Show runner status, loaded models, and current config
	@printf "$(CYAN)── docker model status ──$(RESET)\n"
	@docker model status
	@printf "\n$(CYAN)── docker model list ──$(RESET)\n"
	@docker model list
	@printf "\n$(CYAN)── docker model inspect $(MODEL) ──$(RESET)\n"
	@docker model inspect $(MODEL)

run: ## Start the full stack (docker compose up --build)
	@printf "$(BLUE)▶$(RESET) docker compose up --build\n"
	@docker compose up --build

run-llm: ## Reconfigure context-size on a secondary model (ai/gemma4:E4B)
	@docker model configure --context-size 40960 ai/gemma4:E4B

db: ## Start just the postgres database
	@printf "$(BLUE)▶$(RESET) starting postgres\n"
	@docker compose up -d db

### Dev hygiene

kill-port: ## Kill processes on PORT (default 8000)
	@PORT=$${PORT:-8000}; \
	PIDS=$$(lsof -ti tcp:$$PORT); \
	if [ -z "$$PIDS" ]; then \
	  printf "$(GREEN)✓$(RESET) nothing on port $$PORT\n"; \
	else \
	  printf "$(BLUE)▶$(RESET) killing PIDs on port $$PORT: $$PIDS\n"; \
	  kill -9 $$PIDS; \
	fi

kill-port-test: ## Kill processes on PORT (default 8000) — test-suite variant
	@PORT=$${PORT:-8000}; \
	PIDS=$$(lsof -ti tcp:$$PORT); \
	if [ -z "$$PIDS" ]; then \
	  printf "$(GREEN)✓$(RESET) nothing on port $$PORT\n"; \
	else \
	  printf "$(BLUE)▶$(RESET) killing PIDs on port $$PORT: $$PIDS\n"; \
	  kill -9 $$PIDS; \
	fi

kill-bun-sock: ## Remove stale Bun debugger unix sockets in TMPDIR
	@DIR="$${TMPDIR:-/tmp}"; \
	find "$$DIR" -maxdepth 1 -type s -name '*.sock' \
	  ! -name '*-*' ! -name '*_*' ! -name '*.*.sock' 2>/dev/null \
	  | while read -r s; do \
	    base=$$(basename "$$s" .sock); \
	    case "$$base" in *[!a-z0-9]*) continue;; esac; \
	    [ $${#base} -gt 13 ] && continue; \
	    if ! lsof "$$s" >/dev/null 2>&1; then \
	      printf "$(BLUE)▶$(RESET) removing stale Bun socket: $$s\n"; \
	      rm -f "$$s"; \
	    fi; \
	  done

kill-dev: kill-port kill-port-test kill-bun-sock ## kill-port + kill-port-test + kill-bun-sock (one shot)

### Testing
#
# `make test` is the fast green gate (drift-static-profiler ONLY): the
# 380-ish unit + integration tests for the ORM analyzer. Every test runs
# against the tiny synthetic fixtures in `drift-static-profiler/tests/
# fixtures/` — never against any real / large external codebase.
# Typical wall time on a clean target/: < 5s after the initial build.
#
# `make test-all` adds drift-lab/src-tauri tests. Those include heavy
# live integration tests (openai_live.rs hits Docker Model Runner; the
# repo carries a 469MB GGUF model in tests/.gguf-cache/) — slow and
# environment-dependent. Run when you actually need them, not by default.

test: test-profiler test-lab test-clean ## Run every cargo test suite (drift-static-profiler + drift-lab) and tidy up scan artifacts. ≈5s warm, ≈45s cold

test-fast: test-profiler test-clean ## Fast subset: drift-static-profiler only — skips drift-lab's slow live-LLM tests (≈5s warm)

test-all: test ## Alias for `make test` (kept for backwards compatibility)

test-profiler: ## drift-static-profiler tests (380+ tests, all on small synthetic fixtures, ≈5s total)
	@printf "$(BLUE)▶$(RESET) cargo test — drift-static-profiler\n"
	@cd drift-static-profiler && cargo test --quiet 2>&1 | tee /tmp/.drift-profiler-test.log | \
	  grep -E "^test result|FAILED|error\[" || true
	@if grep -q "FAILED" /tmp/.drift-profiler-test.log 2>/dev/null; then \
	  printf "$(RED)✗$(RESET) drift-static-profiler tests failed\n"; \
	  rm -f /tmp/.drift-profiler-test.log; exit 1; \
	fi
	@rm -f /tmp/.drift-profiler-test.log
	@printf "$(GREEN)✓$(RESET) drift-static-profiler tests passed\n"

test-lab: drift-lab-viewer-bundle ## drift-lab/src-tauri tests (depends on viewer bundle for RustEmbed; includes openai_live.rs)
	@if [ -f drift-lab/src-tauri/Cargo.toml ]; then \
	  printf "$(BLUE)▶$(RESET) cargo test — drift-lab/src-tauri\n"; \
	  cd drift-lab/src-tauri && cargo test --quiet 2>&1 | tee /tmp/.drift-lab-test.log | \
	    grep -E "^test result|FAILED|error\[" || true; \
	  if grep -q "FAILED" /tmp/.drift-lab-test.log 2>/dev/null; then \
	    printf "$(RED)✗$(RESET) drift-lab tests failed\n"; \
	    rm -f /tmp/.drift-lab-test.log; exit 1; \
	  fi; \
	  rm -f /tmp/.drift-lab-test.log; \
	  printf "$(GREEN)✓$(RESET) drift-lab tests passed\n"; \
	else \
	  printf "$(YELLOW)!$(RESET) drift-lab/src-tauri/Cargo.toml not found — skipping\n"; \
	fi

test-clean: ## Remove test scan artifacts (/tmp scans + temp files from older test runs)
	@DIR="$${TMPDIR:-/tmp}"; \
	REMOVED=$$( \
	  find "$$DIR" -maxdepth 2 -name "drift-mg-*" -delete -print 2>/dev/null; \
	  find /tmp -maxdepth 2 -name "drift-mg-*" -delete -print 2>/dev/null; \
	  find "$$DIR" -maxdepth 2 -name "drift-walker-*" -delete -print 2>/dev/null; \
	  find "$$DIR" -maxdepth 2 -name "drift-analyze-*" -delete -print 2>/dev/null; \
	  rm -rf /tmp/orm-out 2>/dev/null && echo "/tmp/orm-out"; \
	); \
	if [ -n "$$REMOVED" ]; then \
	  printf "$(BLUE)▶$(RESET) cleaned test artifacts:\n"; \
	  echo "$$REMOVED" | sed 's/^/    /'; \
	else \
	  printf "$(GREEN)✓$(RESET) no leftover test artifacts\n"; \
	fi

test-clean-target: ## Wipe cargo's target/ dirs (frees the ~1.5 GB of test/dep binaries cargo accumulates). Next build will rebuild from scratch
	@printf "$(BLUE)▶$(RESET) cargo clean (drift-static-profiler)\n"
	@cd drift-static-profiler && cargo clean 2>&1 | tail -1
	@if [ -d drift-lab/src-tauri/target ]; then \
	  printf "$(BLUE)▶$(RESET) cargo clean (drift-lab/src-tauri)\n"; \
	  cd drift-lab/src-tauri && cargo clean 2>&1 | tail -1; \
	fi
	@printf "$(GREEN)✓$(RESET) target/ directories cleaned — next build re-compiles\n"

### Drift Lab — desktop app
# These mirror what .github/workflows/{ci,drift-lab-desktop-build}.yml run,
# so passing locally is strong evidence CI will too. `cargo tauri build`
# internally runs the frontend build via tauri.conf.json's
# beforeBuildCommand, so one target covers the *desktop UI* pipeline — but
# the static-profiler viewer is a SEPARATE frontend that the Rust crate
# embeds at compile time via rust-embed. Every build target below depends
# on `drift-lab-viewer-bundle` so the embedded viewer reflects the live
# `drift-static-profiler/viewer/` sources, not the stub `build.rs` creates
# for fresh clones.

drift-lab-viewer-bundle: ## Build the embedded static-profiler viewer (drift-static-profiler/viewer → dist/). Required before every drift-lab build
	@printf "$(BLUE)▶$(RESET) building static-profiler viewer (npm ci + vite build)\n"
	@cd drift-static-profiler/viewer && \
	  if [ -f package-lock.json ]; then npm ci; else npm install; fi && \
	  npm run build
	@test -f drift-static-profiler/viewer/dist/index.html || { \
	  printf "$(RED)✗$(RESET) viewer build did not produce dist/index.html\n"; exit 1; }
	@printf "$(GREEN)✓$(RESET) viewer dist ready → drift-static-profiler/viewer/dist/\n"

dev: drift-lab-viewer-bundle ## Run the entire desktop app in dev mode (viewer bundled + hot-reload desktop-ui + Rust backend + localhost:5151 HTTP server)
	@printf "$(BLUE)▶$(RESET) launching drift-lab in dev mode — Ctrl+C to stop\n"
	@printf "    Desktop window: opens automatically (Tauri dev)\n"
	@printf "    HTTP server:    $(CYAN)http://127.0.0.1:5151$(RESET) (viewer at /, Swagger at /docs)\n"
	@$(MAKE) --no-print-directory -C drift-lab dev

drift-lab-build: drift-lab-viewer-bundle ## Debug-profile desktop build (~30s, no LTO). Prompts for the updater key passphrase if not cached
	$(require_tauri_key)
	@printf "$(BLUE)▶$(RESET) cargo tauri build --debug\n"
	@$(resolve_tauri_pwd) && cd drift-lab && \
	  TAURI_SIGNING_PRIVATE_KEY="$$(cat ../$(TAURI_SIGNING_KEY_FILE))" \
	  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$$PASS" \
	  cargo tauri build --debug
	@printf "$(GREEN)✓$(RESET) bundles → $(CYAN)drift-lab/src-tauri/target/debug/bundle/$(RESET)\n"

drift-lab-build-release: drift-lab-viewer-bundle ## Release-profile desktop build (matches CI, full LTO, ~5-10min). Prompts for passphrase if not cached
	$(require_tauri_key)
	@printf "$(BLUE)▶$(RESET) cargo tauri build (release)\n"
	@$(resolve_tauri_pwd) && cd drift-lab && \
	  TAURI_SIGNING_PRIVATE_KEY="$$(cat ../$(TAURI_SIGNING_KEY_FILE))" \
	  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$$PASS" \
	  cargo tauri build
	@printf "$(GREEN)✓$(RESET) bundles → $(CYAN)drift-lab/src-tauri/target/release/bundle/$(RESET)\n"

drift-lab-verify: drift-lab-viewer-bundle ## Pre-flight: viewer build + cargo check --release + unit tests + desktop-ui prod build (~30s sanity gate)
	@printf "$(BLUE)▶$(RESET) [1/3] cargo check --release\n"
	@cd drift-lab/src-tauri && cargo check --release
	@printf "$(BLUE)▶$(RESET) [2/3] cargo test --lib\n"
	@cd drift-lab/src-tauri && cargo test --lib
	@printf "$(BLUE)▶$(RESET) [3/3] npm run build (desktop-ui)\n"
	@cd drift-lab/desktop-ui && npm run build
	@printf "$(GREEN)✓$(RESET) verify passed — safe to push\n"

# drift-lab-export — one-shot production export
#
# Runs the release-profile build and copies every distributable artifact
# (.app bundle, .dmg installer, updater tarball + signature, Linux .deb /
# AppImage if present) into dist/drift-lab/<version>/. Produces a clean,
# versioned drop folder you can hand off to QA, attach to a GitHub release,
# or sync to a download bucket — no hunting through target/.
#
# Output layout:
#   dist/drift-lab/<version>/
#     Drift Lab.app/                        # macOS bundle (recursive copy)
#     Drift Lab_<v>_<arch>.dmg              # installer image
#     Drift Lab_<v>_<arch>.app.tar.gz       # updater payload (if signed)
#     Drift Lab_<v>_<arch>.app.tar.gz.sig   # updater signature
#     drift-lab_<v>_amd64.deb               # linux (if compile-linux ran)
#     drift-lab_<v>_amd64.AppImage          # linux (if compile-linux ran)
#     SHA256SUMS                            # checksum manifest
drift-lab-export: ## Production export: release build + copy every artifact → dist/drift-lab/<version>/ + SHA256SUMS
	@VERSION=$$(awk -F\" '/^version *= */ {print $$2; exit}' drift-lab/src-tauri/Cargo.toml); \
	if [ -z "$$VERSION" ]; then \
	  printf "$(RED)✗$(RESET) couldn't read version from drift-lab/src-tauri/Cargo.toml\n"; exit 1; \
	fi; \
	OUT="dist/drift-lab/$$VERSION"; \
	printf "$(BLUE)▶$(RESET) exporting Drift Lab v$$VERSION → $(CYAN)$$OUT$(RESET)\n"; \
	$(MAKE) --no-print-directory drift-lab-build-release; \
	rm -rf "$$OUT"; mkdir -p "$$OUT"; \
	SRC_REL="drift-lab/src-tauri/target/release/bundle"; \
	SRC_LINUX="drift-lab/src-tauri/target-linux/release/bundle"; \
	if [ -d "$$SRC_REL/macos/Drift Lab.app" ]; then \
	  printf "  $(BLUE)•$(RESET) copying macOS .app\n"; \
	  cp -R "$$SRC_REL/macos/Drift Lab.app" "$$OUT/"; \
	fi; \
	find "$$SRC_REL" -maxdepth 3 -type f \
	  \( -name '*.dmg' -o -name '*.app.tar.gz' -o -name '*.app.tar.gz.sig' \) \
	  -exec cp {} "$$OUT/" \; 2>/dev/null || true; \
	if [ -d "$$SRC_LINUX" ]; then \
	  printf "  $(BLUE)•$(RESET) copying Linux bundles\n"; \
	  find "$$SRC_LINUX" -maxdepth 3 -type f \
	    \( -name '*.deb' -o -name '*.AppImage' -o -name '*.rpm' \) \
	    -exec cp {} "$$OUT/" \; 2>/dev/null || true; \
	fi; \
	printf "  $(BLUE)•$(RESET) computing SHA256SUMS\n"; \
	( cd "$$OUT" && find . -maxdepth 1 -type f ! -name SHA256SUMS \
	    -exec shasum -a 256 {} + | sed 's|  \./|  |' > SHA256SUMS ) || true; \
	printf "$(GREEN)✓$(RESET) exported to $(CYAN)$$OUT$(RESET)\n"; \
	ls -1 "$$OUT" | sed 's/^/    /'

drift-lab-export-clean: ## Wipe dist/drift-lab/ so the next drift-lab-export starts clean
	@printf "$(BLUE)▶$(RESET) rm -rf dist/drift-lab\n"
	@rm -rf dist/drift-lab
	@printf "$(GREEN)✓$(RESET) cleaned\n"

# drift-lab-ci-preflight — cheap local mirror of the PR-build workflow
#
# Runs everything the CI workflow runs that's host-independent:
#   1. Build the embedded static-profiler viewer (required for the desktop
#      crate's RustEmbed to pick up the real dist/, not the build.rs stub).
#   2. Read the drift-lab version via the same script CI uses.
#   3. Run the CI helper-script self-tests (compute-shasums, read-version)
#      against synthetic fixtures — so logic bugs surface here, not on the
#      ~10-minute matrix runners.
#
# Skipped on purpose: `cargo tauri build` (5-10min, needs a signing key)
# and `cargo test --lib` — those live in `drift-lab-verify` / `drift-lab-
# build-release`. Run those first if you want the full mirror.
drift-lab-ci-preflight: drift-lab-viewer-bundle ## CI parity check — viewer + helper-script tests + version sanity (skips cargo build/test; cheap)
	@printf "$(BLUE)▶$(RESET) reading drift-lab version\n"
	@v=$$(bash drift-lab/scripts/ci/read-version.sh); \
	  printf "    drift-lab v$(CYAN)%s$(RESET)\n" "$$v"
	@printf "$(BLUE)▶$(RESET) running CI helper-script self-tests\n"
	@bash drift-lab/scripts/ci/test.sh
	@printf "$(GREEN)✓$(RESET) preflight passed\n"
	@printf "    (run $(CYAN)make drift-lab-verify$(RESET) for the heavier mirror: cargo check + lib tests)\n"
