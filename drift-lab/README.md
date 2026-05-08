# Drift Lab

Desktop app for profiling Dockerized services. Built with **Tauri 2 + React 18
+ TypeScript + Rust**. MIT licensed · open source.

[![Latest release](https://img.shields.io/github/v/release/refactor-labs-pub/drift?filter=drift-lab-v*&label=drift-lab&color=ff6b3d)](https://github.com/refactor-labs-pub/drift/releases?q=drift-lab&expanded=true)

---

## The 3 commands

```sh
make            # ① run with hot reload (ctrl+C to stop)
make compile    # ② release build for this OS (.app + .dmg on macOS)
make ship       # ③ compile, then open the built .app
```

That's the daily loop. Everything else is below.

---

## From zero to running

```sh
git clone https://github.com/waste-labs/drift.git
cd drift/drift-lab
make setup      # one-time: rustup + tauri-cli + icons + npm deps (~5 min)
make            # opens the native app with hot reload
```

Once `setup` is done, only `make` is needed day-to-day. Edit a `.tsx` and the
webview hot-reloads; edit a `.rs` and the binary rebuilds + relaunches.

Run `make help` for the full target list, grouped by purpose.

### Get a pre-built binary

Every push to `main` produces installable bundles attached to a
[GitHub Release](https://github.com/refactor-labs-pub/drift/releases) under a
`drift-lab-vX.Y.Z` tag:

| Platform | File |
| -------- | ---- |
| **macOS** (Apple Silicon + Intel, single fat `.dmg`) | `Drift Lab_*.dmg` |
| **Debian / Ubuntu**                                  | `drift-lab_*_amd64.deb` |
| **Other Linux** (portable, no install)               | `drift-lab_*_amd64.AppImage` |

#### macOS — recommended one-line install

```sh
curl -fsSL https://raw.githubusercontent.com/refactor-labs-pub/drift/main/drift-lab/scripts/install-macos.sh | bash
```

That script downloads the latest `Drift Lab.dmg`, copies the app to
`/Applications`, **clears the `com.apple.quarantine` flag**, and launches it.
The Gatekeeper "Not Opened" dialog never appears because quarantine is
cleared before macOS sees the app — the same pattern `rustup`, Homebrew,
and most modern open-source CLIs use.

> **Why a script and not a regular DMG download?** Drift Lab is open source
> and isn't signed with a paid Apple Developer ID ($99/yr), so on macOS
> Sonoma+ a regular DMG download will trigger a *"Drift Lab Not Opened"*
> dialog with **no "Open Anyway" button** — Apple removed that path for
> unsigned apps. The script side-steps the entire dialog by clearing the
> quarantine attribute that browsers attach to downloads.

##### Manual install (if you'd rather not pipe a script to bash)

```sh
# 1. Download the .dmg from GitHub Releases manually, then:
open ~/Downloads/Drift\ Lab_*.dmg
cp -R "/Volumes/Drift Lab/Drift Lab.app" /Applications/
xattr -dr com.apple.quarantine "/Applications/Drift Lab.app"
open "/Applications/Drift Lab.app"
```

Or from a clone of this repo: `cd drift-lab && make trust` does the `xattr`
step on an already-installed `.app`.

##### Click-only fallback (no terminal)

If you've already double-clicked the app and seen the strict dialog:

1. Click **Done** on the dialog.
2. Open **System Settings → Privacy & Security**.
3. Scroll to the "Security" section — you'll see *"Drift Lab was blocked because it is not from an identified developer"* with an **`Open Anyway`** button. Click it.
4. Try opening Drift Lab again — a softer dialog appears with `Open Anyway`. Click it.
5. App opens and macOS remembers your decision.

#### Linux

```sh
# Debian / Ubuntu / derivatives
sudo dpkg -i ~/Downloads/drift-lab_*_amd64.deb

# Anywhere else (AppImage is portable, no install)
chmod +x ~/Downloads/drift-lab_*_amd64.AppImage
~/Downloads/drift-lab_*_amd64.AppImage
```

---

## First-time setup

```sh
make setup      # installs rustup + tauri-cli + generates icons + npm install
```

If you'd rather do it piece by piece:

```sh
make install-rust         # rustup (curl | sh, default profile)
make install-tauri-cli    # cargo install tauri-cli (~3-5 min)
make icons                # generate every platform icon from icon-source.png
make install-js           # npm install in desktop-ui/
```

After `install-rust` you may need a fresh terminal so `cargo` is on PATH
(or `source $HOME/.cargo/env`).

---

## All Make targets

| Tier        | Target              | What it does |
| ----------- | ------------------- | ------------ |
| **①**       | `make` / `make dev` | Hot-reload native app. ctrl+C stops; `.tsx` edits HMR; `.rs` edits trigger a rebuild + relaunch. |
| **②**       | `make compile`      | Release build for the host OS. macOS → `.app` + `.dmg`. Linux → `.deb` + AppImage. |
| **③**       | `make ship`         | `make compile` then `open` the resulting `.app`. |
| Setup       | `make setup`        | install-rust + install-tauri-cli + icons + install-js (idempotent). |
| Setup       | `make install-rust` | One-time rustup install (skips if already installed). |
| Setup       | `make install-tauri-cli` | One-time `cargo install tauri-cli`. |
| Setup       | `make install-js`   | `npm install` in `desktop-ui/`. Auto-runs from dev/compile. |
| Setup       | `make check-rust`   | Fails with a hint if cargo or cargo-tauri are missing. |
| Setup       | `make icons`        | Regenerate the full icon set from `src-tauri/icon-source.png`. Auto-creates a placeholder source if missing. |
| Dev         | `make run`          | Alias for `make dev`. |
| Dev         | `make ui`           | Vite dev server only (no Rust). React app runs in browser at `http://localhost:1420` with a mocked IPC layer. Useful for designers without Rust installed. |
| Compile     | `make compile-mac`  | Same as `compile`; errors out if not on macOS. |
| Compile     | `make compile-linux`| Cross-builds Linux `.deb` + AppImage **inside Docker** (works from macOS). Output lands in `src-tauri/target-linux/release/bundle/`. |
| Launch      | `make open`         | Open the most-recently-built `Drift Lab.app` on macOS. |
| Cleanup     | `make clean`        | Remove `dist/`, bundles, Linux target. Keeps `node_modules` and Rust cache. |
| Cleanup     | `make distclean`    | Also nuke `node_modules`, `src-tauri/target`, `.cargo-linux`. Full reset. |
| Help        | `make help`         | Print the live target list with descriptions. |

---

## Manual / underlying commands

If you'd rather skip the Makefile, here's what each target actually runs.

```sh
# === Setup ===
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
cargo install tauri-cli --version "^2.0" --locked
cd src-tauri && cargo tauri icon icon-source.png

cd desktop-ui && npm install

# === Dev ===
cargo tauri dev                                 # native app, hot reload
cd desktop-ui && npm run dev                    # frontend only (mock IPC)

# === Build ===
cargo tauri build                               # release for this OS
cargo tauri build --bundles deb,appimage        # explicit bundle targets

# === Launch ===
open "src-tauri/target/release/bundle/macos/Drift Lab.app"
```

### Linux build via Docker (manual)

```sh
cd desktop-ui && npm run build && cd ..
docker run --rm --platform linux/amd64 \
  -v "$(pwd):/work" -w /work/src-tauri \
  -e CARGO_HOME=/work/.cargo-linux \
  -e CARGO_TARGET_DIR=/work/src-tauri/target-linux \
  rust:1.82-bookworm bash -lc '
    apt-get update -qq && apt-get install -y -qq \
      libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev \
      libayatana-appindicator3-dev librsvg2-dev pkg-config libsoup-3.0-dev && \
    cargo install tauri-cli --version "^2.0" --locked --quiet && \
    cargo tauri build --bundles deb,appimage'
```

---

## Auto-update

Drift Lab self-updates from GitHub Releases via the Tauri 2 updater plugin.
On launch the app silently calls
`https://github.com/refactor-labs-pub/drift/releases/latest/download/latest.json`,
compares versions, and shows an "Update & relaunch" banner when a newer
`drift-lab-v*` release exists. The same UI is also available in
**Settings → Updates** with a manual "Check for updates" button.

The updater verifies an **Ed25519 signature** on every download — separate
from Apple code signing. Without a valid signature, the install is refused.
This is the only chain of trust for an app that's not signed by an Apple
Developer ID, so it's not optional.

### One-time signing key setup

```sh
make updater-keys             # generates ~/.tauri/drift-lab.key + .pub
```

That script:

1. Runs `cargo tauri signer generate -w ~/.tauri/drift-lab.key`.
2. Prints the public key — paste it into
   [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json) under
   `plugins.updater.pubkey`.
3. Reminds you to push the private key to GitHub Actions:

   ```sh
   gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/drift-lab.key
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # only if you set one
   ```

After that, every push to `main` produces signed bundles + a `latest.json`
manifest attached to the GitHub Release. Existing installs see the new
version on next launch and can update with a single click.

> **Lost the private key?** Generate a new one and ship a build with the
> new public key. Existing installs won't auto-update past the rotation —
> users will have to reinstall once. The key never leaves your machine and
> the GitHub secret store, so this is rare.

### What the user sees

- **Home screen** — a non-blocking banner top-left when an update is
  available. Click *Update & relaunch* to apply.
- **Settings → Updates** — current version, manual check button,
  release notes, download progress.

The download streams in the background via Rust (no CSP impact); install
swaps the `.app` / AppImage in place; `tauri-plugin-process::relaunch()`
brings the new binary back up.

## Continuous integration

Two GitHub Actions workflows at the repo root, both scoped via `paths:` so
they only run when `drift-lab/**` changes:

| File | Trigger | What it does |
| ---- | ------- | ------------ |
| `drift-lab-desktop-build.yml`   | PR touching `drift-lab/**` · `workflow_dispatch` | Validation matrix build (no release). Uploads bundles as 7-day workflow artifacts so reviewers can grab a build off a PR. |
| `drift-lab-desktop-release.yml` | **Every push to `main`** touching `drift-lab/**` · `workflow_dispatch` | Auto-bumps a `drift-lab-v*` tag (conventional-commit driven), runs the matrix, attaches all installable bundles to a published GitHub Release. |

Matrix legs: `macos-14` builds a **universal `.dmg`** that runs on both Apple
Silicon and Intel (single fat binary via `--target universal-apple-darwin`,
avoids the chronic macos-13 runner queue) · `ubuntu-22.04` for the
`libwebkit2gtk-4.1` deps Tauri 2 requires. Rust `target/`, npm modules, and
the `cargo-tauri` binary are all cached between runs.

### How the auto-release works

```
push to main (drift-lab/** touched)
        │
        ▼
┌──────────────┐    ┌──────────────────────────────┐    ┌────────────────────────┐
│ bump (1 job) │ →  │ build matrix (2 parallel)    │ →  │ publish (1 job)        │
│              │    │  ─ macos-universal → .dmg    │    │  ─ download artifacts  │
│ reads last   │    │     (fat: arm64 + x86_64)    │    │  ─ generate notes      │
│ drift-lab-v* │    │  ─ linux-x86_64 → .deb +     │    │  ─ git tag + push      │
│ tag, decides │    │     .AppImage                │    │  ─ gh release create   │
│ next version │    └──────────────────────────────┘    └────────────────────────┘
└──────────────┘
```

**Version bumps** (since the last `drift-lab-v*.*.*` tag):

- Any commit message containing `BREAKING CHANGE` or matching `<type>!:` → **major**
- Any commit message starting with `feat:` or `feat(scope):` → **minor**
- Anything else → **patch**

Only commits that actually touched `drift-lab/` count toward the bump or the
release notes — drift-lab releases are independent of the surrounding
`action/` and `web-app/` work in this repo.

**To force a release** without a code change, use the workflow's **Run
workflow** button (workflow_dispatch). The bump job will skip if there are no
new drift-lab commits since the last tag.

### First release

On first run, no `drift-lab-v*` tag exists → version starts at `v0.0.0` and
gets bumped from there. With the current commit history (`feat: add drift-lab
desktop app…`) the first auto-release will be `drift-lab-v0.1.0`.

## Stack

| Layer            | Choice                                |
| ---------------- | ------------------------------------- |
| Shell            | Tauri 2                               |
| Backend          | Rust (`tokio`, `bollard`, `sqlx`)     |
| Frontend         | React 18 + Vite + TypeScript          |
| Routing          | `react-router-dom`                    |
| FE state         | Zustand · TanStack Query              |
| Local DB         | SQLite via `sqlx`                     |
| Tauri plugins    | `dialog`, `fs`, `log`, `opener`, `updater`, `window-state`, built-in `tray-icon` |

## Layout

```
drift-lab/
  Makefile              # one-command dev/compile (you are here)
  desktop-ui/           # React + Vite + TS
    package.json
    index.html
    vite.config.ts
    src/
      main.tsx App.tsx
      pages/ components/ lib/ store/ styles/
  src-tauri/            # Rust crate (Tauri shell + workflow + docker + db)
    Cargo.toml tauri.conf.json build.rs
    capabilities/default.json
    icon-source.png     # 1024x1024 master — feed for `make icons`
    icons/              # generated PNGs + .icns + .ico
    src/
      main.rs lib.rs
      commands.rs       # #[tauri::command]s
      workflow.rs       # 5-stage pipeline (currently stubbed)
      docker.rs         # bollard wrappers
      db.rs             # sqlite pool + schema
      events.rs         # serde payloads
      tray.rs           # system tray menu
```

## How the workflow runs

1. UI calls `startRun(projectPath)` → Tauri command spawns a tokio task and
   returns a `runId` immediately.
2. Rust streams progress as `run://step` events, terminating with
   `run://complete` (or `run://error`).
3. The React `runStore` (Zustand) listens to those events; the page renders
   idle / running / done off store state.
4. Outside Tauri (`make ui`), the same surface is served by an in-process mock
   in `desktop-ui/src/lib/tauri.ts`, so the UI animates without the Rust side
   built.

## Wiring real work

Stages live in [`src-tauri/src/workflow.rs`](src-tauri/src/workflow.rs). Each
calls `run_stage(index)` which currently `tokio::sleep`s. Replace with:

| Stage | Real implementation |
| ----- | ------------------- |
| 0 — Locate Docker image | `docker::find_image(path)` (parse Dockerfile / compose) |
| 1 — Detect runtime      | `docker::inspect_layers(image)` + heuristic |
| 2 — Install profiler    | `bollard` exec into container, drop py-spy/async-profiler |
| 3 — Run profiling       | drive load (reqwest / vegeta sidecar), collect samples |
| 4 — Analyze             | parse samples, rank, persist to sqlite via `db::pool()` |

Persisted runs land in the `runs` table created by [`src-tauri/src/db.rs`](src-tauri/src/db.rs).
