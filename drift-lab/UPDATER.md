# Drift Lab auto-updater — bootstrap & release guide

Once configured, Drift Lab checks its updater endpoint on startup, downloads
the new bundle in the background, verifies its signature, and prompts the
user to relaunch. Until the **first signed release is published**, the
Settings → Updates panel shows a "not configured yet" message — that's
because the [endpoint][endpoint] returns 404 with no `latest.json` to fetch.

This document is the **one-time bootstrap** plus the steady-state release
recipe.

---

## How the pieces fit

```
┌───────────────────────────────────┐
│ tauri.conf.json                   │
│   plugins.updater                 │
│     endpoints[0] ─── points to ───┼──▶ GitHub Releases /latest/latest.json
│     pubkey       (verifies sig)   │
└───────────────────────────────────┘
              ▲
              │ signed at build
              │
┌───────────────────────────────────┐
│ Release CI                        │
│   tauri-apps/tauri-action@v0      │
│     ├─ builds desktop bundles     │
│     ├─ signs with PRIVATE_KEY     │
│     ├─ creates GitHub draft       │
│     └─ uploads latest.json        │
└───────────────────────────────────┘
              ▲
              │ trigger
              │
       push tag drift-lab-v*
```

The key invariant: **a build is only useful for auto-updates if it was
signed with the private key whose public counterpart is baked into
`tauri.conf.json`**. The updater rejects any manifest whose `signature`
doesn't verify against that embedded `pubkey`. **If you lose the private
key, you can never publish updates that existing installs will accept** —
you'd have to ship a new app identifier and ask users to reinstall.

---

## One-time bootstrap (do this once per app)

### The shortcut: one command does everything

If you have `gh` (GitHub CLI) installed and authenticated (`brew install gh && gh auth login`),
the entire bootstrap is:

```bash
bash drift-lab/scripts/bootstrap-updater.sh
```

The script generates the keypair (if not present), patches `tauri.conf.json` with the
public key, and uploads both GitHub Actions secrets via `gh secret set` — so you never
touch the GitHub settings UI. It's safe to re-run; every step is idempotent.

After it finishes, just `git add … && git commit && git push`, then tag a release.

The manual breakdown below is for anyone who wants to understand what the script does,
or who can't install `gh` for some reason.

---

### 1. Generate a minisign keypair

From the repo root, run:

```bash
cd drift-lab/desktop-ui
npm run tauri signer generate -- -w ../../.tauri-keys/drift-lab.key
```

This writes two files **outside the repo** (the `.tauri-keys` dir is
gitignored — verify before pushing):

- `~/Projects/drift/.tauri-keys/drift-lab.key` — **private key** (secret)
- `~/Projects/drift/.tauri-keys/drift-lab.key.pub` — public key (commit-safe)

The CLI prompts for a passphrase — pick a strong one and store it in your
password manager.

### 2. Paste the public key into `tauri.conf.json`

Open [drift-lab/src-tauri/tauri.conf.json](src-tauri/tauri.conf.json) and
replace the placeholder:

```jsonc
"plugins": {
  "updater": {
    "pubkey": "REPLACE_WITH_TAURI_SIGNER_PUBLIC_KEY",  // ← paste contents of drift-lab.key.pub
    ...
  }
}
```

The `.key.pub` file is one line of base64 — copy it as-is, no quotes
inside the value. Commit the change.

### 3. Add three GitHub Actions secrets

Repo → Settings → Secrets and variables → Actions → New repository secret:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | The **entire** content of `drift-lab.key` (multi-line — includes the `untrusted comment:` header) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The passphrase you set in step 1 |

(`GITHUB_TOKEN` is provided by Actions automatically — no manual setup.)

### 4. Back up the keys

Copy `drift-lab.key` and the password to your password manager. **Do not**
commit them, do not lose them. A second engineer who needs to ship
releases must be given the same key — there is no "rotate to a new key"
without a coordinated client migration.

---

## Cutting a release (the steady-state recipe)

1. Bump the version in two places (they must match):
   - [drift-lab/src-tauri/Cargo.toml](src-tauri/Cargo.toml) → `[package].version`
   - [drift-lab/src-tauri/tauri.conf.json](src-tauri/tauri.conf.json) → `version`
   - [drift-lab/desktop-ui/package.json](desktop-ui/package.json) → `version`
2. Commit the bump on `main`.
3. Tag and push:
   ```bash
   git tag drift-lab-v0.1.1
   git push origin drift-lab-v0.1.1
   ```
4. The `drift-lab desktop / release` workflow runs (see
   [.github/workflows/drift-lab-desktop-release.yml](../.github/workflows/drift-lab-desktop-release.yml)).
   It produces a **draft** GitHub Release.
5. Review the draft in the GitHub UI — the assets should include:
   - `Drift Lab_<version>_universal.dmg`
   - `Drift Lab_<version>_universal.app.tar.gz` + `.app.tar.gz.sig`
   - `drift-lab_<version>_amd64.AppImage` + `.AppImage.sig`
   - `drift-lab_<version>_amd64.deb`
   - `latest.json`
6. Click **Publish release**. Within ~60 seconds, every installed Drift Lab
   notices the new version on next startup and prompts to update.

### What `latest.json` looks like

After tauri-action runs, the manifest at
`https://github.com/refactorlab/drift/releases/latest/download/latest.json`
should look like:

```json
{
  "version": "0.1.1",
  "notes": "See the assets to download this version and install.",
  "pub_date": "2026-05-14T12:34:56Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IC4uLgo=",
      "url": "https://github.com/refactorlab/drift/releases/download/drift-lab-v0.1.1/Drift%20Lab_0.1.1_universal.app.tar.gz"
    },
    "darwin-x86_64":  { "signature": "...", "url": "...(same universal asset)..." },
    "linux-x86_64":   { "signature": "...", "url": "...AppImage.tar.gz" }
  }
}
```

(The release workflow's final step **mirrors** the macOS universal entry
under both `darwin-aarch64` and `darwin-x86_64` keys — tauri-action only
writes the host runner's triple by default.)

---

## Testing locally before tagging

You can dry-run the signing path without publishing:

```bash
cd drift-lab/src-tauri
export TAURI_SIGNING_PRIVATE_KEY="$(cat ../../.tauri-keys/drift-lab.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="your-passphrase"
cargo tauri build
ls -la target/release/bundle/macos/   # check for .app.tar.gz + .sig
```

If the `.sig` files exist next to the bundles, signing works. To verify
the updater end-to-end:

1. Build twice with **different versions** (e.g. 0.1.0 then 0.1.1).
2. Install the 0.1.0 build locally.
3. Host the 0.1.1 bundle + a hand-edited `latest.json` on a local HTTP
   server (e.g. `python -m http.server`).
4. Temporarily point `plugins.updater.endpoints[0]` at the local URL,
   rebuild 0.1.0, run it — it should prompt for the 0.1.1 update.
5. Revert the endpoint before merging.

---

## Troubleshooting

| Symptom | Likely cause |
| --- | --- |
| "Auto-update isn't configured yet" banner persists after first release | `tauri.conf.json` still has the placeholder `pubkey`, or the release was published with a different private key |
| `signature verification failed` in app logs | Public key in config doesn't match the signing key used during build |
| `latest.json` missing `darwin-aarch64` entry on M-series Mac | The post-build mirror step in the release workflow didn't run / didn't find the existing darwin entry — re-run the workflow |
| Release workflow fails on `tauri-action` step with "private key not found" | Secret `TAURI_SIGNING_PRIVATE_KEY` not set or pasted with leading/trailing whitespace |

[endpoint]: https://github.com/refactorlab/drift/releases/latest/download/latest.json
