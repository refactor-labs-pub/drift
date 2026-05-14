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
        drift-lab-build drift-lab-build-release drift-lab-verify \
        drift-lab-export drift-lab-export-clean

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

### Drift Lab — desktop app
# These mirror what .github/workflows/{ci,drift-lab-desktop-build}.yml run,
# so passing locally is strong evidence CI will too. `cargo tauri build`
# internally runs the frontend build via tauri.conf.json's
# beforeBuildCommand, so one target covers the whole pipeline.

drift-lab-build: ## Debug-profile desktop build (~30s, no LTO). Prompts for the updater key passphrase if not cached
	$(require_tauri_key)
	@printf "$(BLUE)▶$(RESET) cargo tauri build --debug\n"
	@$(resolve_tauri_pwd) && cd drift-lab && \
	  TAURI_SIGNING_PRIVATE_KEY="$$(cat ../$(TAURI_SIGNING_KEY_FILE))" \
	  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$$PASS" \
	  cargo tauri build --debug
	@printf "$(GREEN)✓$(RESET) bundles → $(CYAN)drift-lab/src-tauri/target/debug/bundle/$(RESET)\n"

drift-lab-build-release: ## Release-profile desktop build (matches CI, full LTO, ~5-10min). Prompts for passphrase if not cached
	$(require_tauri_key)
	@printf "$(BLUE)▶$(RESET) cargo tauri build (release)\n"
	@$(resolve_tauri_pwd) && cd drift-lab && \
	  TAURI_SIGNING_PRIVATE_KEY="$$(cat ../$(TAURI_SIGNING_KEY_FILE))" \
	  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$$PASS" \
	  cargo tauri build
	@printf "$(GREEN)✓$(RESET) bundles → $(CYAN)drift-lab/src-tauri/target/release/bundle/$(RESET)\n"

drift-lab-verify: ## Pre-flight: cargo check --release + unit tests + frontend prod build (~15s sanity gate)
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
