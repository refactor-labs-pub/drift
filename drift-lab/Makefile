# =============================================================================
# Drift Lab — desktop app build orchestration
#
# === The 3 commands you'll actually use ===
#   make            # run the desktop app with hot reload (ctrl+C to stop)
#   make compile    # release build for the host (.app + .dmg on macOS)
#   make ship       # compile + open the built .app
#
# Everything else (setup, mock UI, icons, linux build, cleanup) is below;
# see `make help` for the full list.
#
# Help is auto-generated: add `## doc string` after any target ':' and it shows
# up in `make help`. `### Section name` lines start a new help section.
# =============================================================================

# --- Config -------------------------------------------------------------------

UI_DIR        := desktop-ui
TAURI_DIR     := src-tauri
APP_NAME      := drift-lab
BUNDLE_MAC    := $(TAURI_DIR)/target/release/bundle/macos/Drift Lab.app
BUNDLE_DMG    := $(TAURI_DIR)/target/release/bundle/dmg
LINUX_IMAGE   := rust:1.82-bookworm
ICON_SOURCE   := $(TAURI_DIR)/icon-source.png

UNAME_S := $(shell uname -s)

# Make cargo + cargo-installed binaries visible to recipes even on the same
# `make` run that just installed them (rustup writes to ~/.cargo/bin, which
# isn't on PATH until a new shell sources ~/.cargo/env).
export PATH := $(HOME)/.cargo/bin:$(PATH)

# Pick tauri-cli: prefer cargo-installed `cargo-tauri`, else the binary
# `@tauri-apps/cli` drops into desktop-ui/node_modules/.bin/. Both must run
# from the drift-lab/ root so tauri can locate src-tauri/tauri.conf.json.
ifneq ($(shell command -v cargo-tauri 2>/dev/null),)
TAURI := cargo tauri
else
TAURI := $(UI_DIR)/node_modules/.bin/tauri
endif

BLUE   := \033[1;34m
GREEN  := \033[1;32m
YELLOW := \033[1;33m
RESET  := \033[0m

.DEFAULT_GOAL := dev
.PHONY: dev run ship setup ui \
        install install-js install-rust install-tauri-cli check-rust \
        icons \
        compile compile-mac compile-linux \
        open trust updater-keys clean distclean help \
        warnings lint

# === Tier 1 — the 3 commands ================================================
### Tier 1 (the 3 you'll use)

dev: install ## ① Run desktop app with hot reload (ctrl+C to stop)
	@printf "$(BLUE)▶$(RESET) launching drift-lab on $(UNAME_S) — ctrl+C to stop\n"
	@$(TAURI) dev

compile: install ## ② Release build for the host OS
	@printf "$(BLUE)▶$(RESET) building drift-lab release for $(UNAME_S)\n"
	@$(TAURI) build
	@printf "$(GREEN)✓$(RESET) bundles → $(TAURI_DIR)/target/release/bundle/\n"

ship: compile open ## ③ Compile, then open the built .app

# === Setup ===================================================================
### Setup

setup: install-rust install-tauri-cli icons install-js ## One-time machine setup
	@printf "$(GREEN)✓$(RESET) drift-lab is ready — try: make\n"

install: install-js check-rust ## Ensure deps are present (called by dev/compile)

install-js: $(UI_DIR)/node_modules/.package-lock.json ## npm install if needed

$(UI_DIR)/node_modules/.package-lock.json: $(UI_DIR)/package.json $(UI_DIR)/package-lock.json
	@printf "$(BLUE)▶$(RESET) installing JS deps in $(UI_DIR)/\n"
	@cd $(UI_DIR) && npm install
	@touch $@

check-rust: ## Fail fast if cargo + cargo-tauri aren't on PATH
	@command -v cargo >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) Rust not installed. Run: make install-rust\n"; exit 1; }
	@command -v cargo-tauri >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) tauri-cli missing. Run: make install-tauri-cli\n"; exit 1; }

install-rust: ## Install rustup (writes to ~/.cargo, ~/.rustup, ~/.zshenv)
	@if command -v cargo >/dev/null 2>&1; then \
		printf "$(GREEN)✓$(RESET) rust already installed: $$(cargo --version)\n"; \
	else \
		printf "$(BLUE)▶$(RESET) installing rustup (default profile, stable toolchain)\n"; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default; \
		printf "$(YELLOW)!$(RESET) open a new terminal or: source \$$HOME/.cargo/env\n"; \
	fi

install-tauri-cli: ## cargo install tauri-cli (~3-5 min the first time)
	@command -v cargo >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) install rust first: make install-rust\n"; exit 1; }
	@if command -v cargo-tauri >/dev/null 2>&1; then \
		printf "$(GREEN)✓$(RESET) tauri-cli already installed: $$(cargo-tauri --version)\n"; \
	else \
		cargo install tauri-cli --version "^2.0" --locked; \
	fi

# === Dev variants ============================================================
### Dev variants

run: dev ## alias for `dev`

ui: install-js ## Run just the React UI in a browser (no backend — IPC calls will fail)
	@printf "$(BLUE)▶$(RESET) vite dev server — http://localhost:1420\n"
	@printf "$(YELLOW)!$(RESET) no Tauri runtime: anything that calls the Rust backend will throw.\n"
	@printf "    use $(BLUE)make dev$(RESET) for end-to-end work.\n"
	@cd $(UI_DIR) && npm run dev

# === Icons ===================================================================
### Icons

icons: $(ICON_SOURCE) ## Regenerate every platform icon from icon-source.png
	@command -v cargo-tauri >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) tauri-cli missing. Run: make install-tauri-cli\n"; exit 1; }
	@printf "$(BLUE)▶$(RESET) generating icon set from $(ICON_SOURCE)\n"
	@cd $(TAURI_DIR) && cargo tauri icon icon-source.png

$(ICON_SOURCE):
	@printf "$(BLUE)▶$(RESET) creating placeholder icon-source.png (1024x1024 orange gradient)\n"
	@python3 -c "import math,struct,zlib,sys;\
SIZE=1024;\
A=(0xFF,0x6B,0x3D); B=(0xFF,0x95,0x58); C=(0xFF,0xB5,0x47); BG=(0xFA,0xFA,0xF7);\
lerp=lambda a,b,t:tuple(int(a[i]+(b[i]-a[i])*t) for i in range(3));\
g=lambda x,y,w,h:(lerp(A,B,(x+y)/(w+h)*2) if (x+y)/(w+h)<0.5 else lerp(B,C,((x+y)/(w+h)-0.5)*2));\
mask=lambda x,y,w,h,r:(1.0 if not((x<r or x>=w-r) and (y<r or y>=h-r)) else max(0.0,min(1.0,1.0-(math.hypot(x-(r if x<r else w-r),y-(r if y<r else h-r))-r))));\
rad=int(SIZE*0.22);\
rows=[];\
[rows.append(bytes(b''.join(bytes(tuple(int(g(x,y,SIZE,SIZE)[k]*mask(x,y,SIZE,SIZE,rad)+BG[k]*(1-mask(x,y,SIZE,SIZE,rad))) for k in range(3))) for x in range(SIZE)))) for y in range(SIZE)];\
chunk=lambda t,d:struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xFFFFFFFF);\
ihdr=struct.pack('>IIBBBBB',SIZE,SIZE,8,2,0,0,0);\
raw=b''.join(b'\x00'+r for r in rows);\
open('$@','wb').write(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',ihdr)+chunk(b'IDAT',zlib.compress(raw,6))+chunk(b'IEND',b''))"

# === Compile variants ========================================================
### Compile variants

compile-mac: ## macOS-only release build (errors on other hosts)
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) compile
else
	@printf "$(YELLOW)!$(RESET) compile-mac requires macOS (host: $(UNAME_S))\n"; exit 1
endif

compile-linux: install-js ## Linux .deb + AppImage via Docker (works from macOS)
	@command -v docker >/dev/null 2>&1 || { \
		printf "$(YELLOW)!$(RESET) Docker required for Linux cross-build\n"; exit 1; }
	@printf "$(BLUE)▶$(RESET) [1/2] building frontend on host\n"
	@cd $(UI_DIR) && npm run build
	@printf "$(BLUE)▶$(RESET) [2/2] building tauri linux bundle inside $(LINUX_IMAGE)\n"
	@docker run --rm \
		--platform linux/amd64 \
		-v "$(CURDIR):/work" \
		-w /work/$(TAURI_DIR) \
		-e CARGO_HOME=/work/.cargo-linux \
		-e CARGO_TARGET_DIR=/work/$(TAURI_DIR)/target-linux \
		$(LINUX_IMAGE) bash -lc '\
			apt-get update -qq && \
			DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
				libwebkit2gtk-4.1-dev build-essential curl wget file \
				libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
				pkg-config libsoup-3.0-dev > /dev/null && \
			cargo install tauri-cli --version ^2.0 --locked --quiet && \
			cargo tauri build --bundles deb,appimage \
		'
	@printf "$(GREEN)✓$(RESET) linux bundles → $(TAURI_DIR)/target-linux/release/bundle/\n"

# === Launch ==================================================================
### Launch

open: ## Open the most-recently-built .app (macOS)
ifeq ($(UNAME_S),Darwin)
	@if [ -d "$(BUNDLE_MAC)" ]; then \
		printf "$(BLUE)▶$(RESET) launching $(BUNDLE_MAC)\n"; \
		open "$(BUNDLE_MAC)"; \
	else \
		printf "$(YELLOW)!$(RESET) no build yet. Run: make compile\n"; exit 1; \
	fi
else
	@printf "$(YELLOW)!$(RESET) make open is macOS-only; on Linux launch the AppImage directly\n"; exit 1
endif

updater-keys: ## Generate the Ed25519 keypair for the in-app auto-updater (one-time)
	@bash scripts/generate-updater-keys.sh

trust: ## macOS: clear com.apple.quarantine on the installed .app (Gatekeeper bypass)
ifeq ($(UNAME_S),Darwin)
	@APP="/Applications/Drift Lab.app"; \
	if [ ! -d "$$APP" ]; then APP="$(BUNDLE_MAC)"; fi; \
	if [ ! -d "$$APP" ]; then \
		printf "$(YELLOW)!$(RESET) Drift Lab.app not found in /Applications or build dir.\n"; \
		printf "    install it first: open the .dmg and drag to Applications.\n"; exit 1; \
	fi; \
	printf "$(BLUE)▶$(RESET) clearing com.apple.quarantine on %s\n" "$$APP"; \
	xattr -dr com.apple.quarantine "$$APP" && \
	printf "$(GREEN)✓$(RESET) Drift Lab is now trusted — open normally.\n"
else
	@printf "$(YELLOW)!$(RESET) make trust is macOS-only (Gatekeeper is a macOS feature)\n"; exit 1
endif

# === Lint / warnings =========================================================
### Lint / warnings

warnings: check-rust ## Show all rustc warnings (forces re-check so cache can't hide them)
	@printf "$(BLUE)▶$(RESET) running cargo check on $(TAURI_DIR) (all targets)\n"
	@cd $(TAURI_DIR) && cargo clean -p drift-lab --quiet 2>/dev/null || true
	@cd $(TAURI_DIR) && cargo check --all-targets --color always

lint: check-rust ## Show all clippy warnings (forces re-check so cache can't hide them)
	@printf "$(BLUE)▶$(RESET) running cargo clippy on $(TAURI_DIR) (all targets)\n"
	@cd $(TAURI_DIR) && cargo clean -p drift-lab --quiet 2>/dev/null || true
	@cd $(TAURI_DIR) && cargo clippy --all-targets --color always

# === Cleanup =================================================================
### Cleanup

clean: ## Remove build outputs (keeps node_modules + cargo cache)
	rm -rf $(UI_DIR)/dist $(UI_DIR)/dist-tsbuild-node $(UI_DIR)/*.tsbuildinfo
	rm -rf $(TAURI_DIR)/target/release/bundle
	rm -rf $(TAURI_DIR)/target-linux

distclean: clean ## Also drop node_modules + cargo target (full reset)
	rm -rf $(UI_DIR)/node_modules
	rm -rf $(TAURI_DIR)/target $(TAURI_DIR)/gen
	rm -rf .cargo-linux

help: ## Show this help (auto-generated from inline doc strings)
	@printf "\n$(BLUE)Drift Lab — make targets$(RESET)\n"
	@awk 'BEGIN { FS = ":.*## " } \
		/^### / { printf "\n  $(GREEN)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_.-]+:.*## / { printf "    $(BLUE)%-18s$(RESET) %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@printf "\n"
