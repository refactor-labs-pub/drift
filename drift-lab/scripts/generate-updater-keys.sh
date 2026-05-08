#!/usr/bin/env bash
# Generate the Ed25519 keypair the Tauri updater plugin uses to verify update
# downloads. Run this ONCE per project. The public key gets committed; the
# private key gets pasted into a GitHub Actions secret.
#
# This is independent of Apple code signing — it's an integrity check on the
# update bundle the plugin downloads. Without it, the plugin will refuse to
# install anything (which is the safe default).
set -euo pipefail

key_dir="${TAURI_KEY_DIR:-$HOME/.tauri}"
key_name="${TAURI_KEY_NAME:-drift-lab.key}"
key_path="$key_dir/$key_name"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH. Run 'make install-rust' first." >&2
  exit 1
fi
if ! cargo tauri --version >/dev/null 2>&1; then
  echo "error: cargo tauri not installed. Run 'make install-tauri-cli'." >&2
  exit 1
fi

mkdir -p "$key_dir"
chmod 700 "$key_dir"

if [ -f "$key_path" ]; then
  echo "Key already exists at $key_path — refusing to overwrite."
  echo "Remove it manually if you really want to regenerate."
  exit 1
fi

echo "Generating Ed25519 keypair → $key_path"
cargo tauri signer generate -w "$key_path"

pub_key_path="${key_path}.pub"
if [ ! -f "$pub_key_path" ]; then
  echo "error: expected public key at $pub_key_path" >&2
  exit 1
fi

pub_key=$(tr -d '\n' < "$pub_key_path")

cat <<EOF

─────────────────────────────────────────────────────────────────
Done. Two things to do next:

  1. Paste this public key into drift-lab/src-tauri/tauri.conf.json
     under plugins.updater.pubkey:

$(echo "$pub_key" | sed 's/^/        /')

  2. Set the private key (and password, if you set one) as GitHub
     Actions repository secrets:

       gh secret set TAURI_SIGNING_PRIVATE_KEY < "$key_path"
       # If you set a password during generation:
       gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD

The private key NEVER leaves your machine and the GitHub secret store.
Lose it and you'll have to ship a new public key — which means
existing installs can't auto-update until they install a new build
that bundles the new public key.
─────────────────────────────────────────────────────────────────
EOF
