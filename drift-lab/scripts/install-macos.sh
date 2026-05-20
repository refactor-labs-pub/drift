#!/usr/bin/env bash
#
# Drift Lab — macOS one-line installer.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/refactorlab/drift/main/drift-lab/scripts/install-macos.sh | bash
#
# What this does:
#   1. Resolves the latest drift-lab-v* release from GitHub
#   2. Downloads the universal Drift Lab*.dmg
#   3. Mounts it, copies Drift Lab.app to /Applications
#   4. Clears com.apple.quarantine so Gatekeeper doesn't block first launch
#   5. Unmounts and (optionally) opens the app
#
# Why this is needed:
#   The DMG isn't notarized by Apple (would require a paid Developer ID),
#   so Safari attaches a quarantine attribute on download and macOS shows the
#   "Drift Lab Not Opened" dialog with no Open-Anyway button. This script
#   removes that quarantine flag, which is the standard free workflow for
#   open-source apps on macOS (rustup, Homebrew, etc. all do equivalent).

set -euo pipefail

REPO="refactorlab/drift"
APP_NAME="Drift Lab"
TAG_PREFIX="drift-lab-v"

color_blue=$'\033[1;34m'
color_green=$'\033[1;32m'
color_yellow=$'\033[1;33m'
color_red=$'\033[1;31m'
color_reset=$'\033[0m'

step()  { printf '%s▶%s %s\n' "$color_blue"   "$color_reset" "$*"; }
ok()    { printf '%s✓%s %s\n' "$color_green"  "$color_reset" "$*"; }
warn()  { printf '%s!%s %s\n' "$color_yellow" "$color_reset" "$*"; }
die()   { printf '%s✗%s %s\n' "$color_red"    "$color_reset" "$*" >&2; exit 1; }

# --- Sanity ------------------------------------------------------------------

[[ "$(uname -s)" == "Darwin" ]] || die "this installer is macOS only (uname -s = $(uname -s))"
command -v curl     >/dev/null 2>&1 || die "curl not found"
command -v hdiutil  >/dev/null 2>&1 || die "hdiutil not found (macOS sanity check)"
command -v xattr    >/dev/null 2>&1 || die "xattr not found"

# --- Resolve latest drift-lab tag -------------------------------------------

step "looking up latest $TAG_PREFIX* release on $REPO"
latest_json=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=30")
latest_tag=$(printf '%s' "$latest_json" \
  | grep -oE "\"tag_name\": \"$TAG_PREFIX[^\"]+\"" \
  | head -1 \
  | sed -E "s/\"tag_name\": \"(.*)\"/\1/")

[[ -n "$latest_tag" ]] || die "couldn't find any $TAG_PREFIX* release on $REPO"
ok "latest release: $latest_tag"

# --- Pick the right DMG asset -----------------------------------------------

dmg_url=$(printf '%s' "$latest_json" \
  | grep -oE '"browser_download_url": "[^"]+\.dmg"' \
  | grep -E "/$latest_tag/" \
  | head -1 \
  | sed -E 's/"browser_download_url": "(.*)"/\1/')

if [[ -z "$dmg_url" ]]; then
  # fallback: any .dmg in the latest release block
  dmg_url=$(printf '%s' "$latest_json" \
    | awk -v tag="$latest_tag" '
        /"tag_name":/ { in_block = ($0 ~ tag) }
        in_block && /browser_download_url.*\.dmg/ {
          gsub(/.*"browser_download_url": "/, "")
          gsub(/".*/, "")
          print; exit
        }')
fi

[[ -n "$dmg_url" ]] || die "couldn't find a .dmg asset on $latest_tag"
ok "dmg url: $dmg_url"

# --- Download ----------------------------------------------------------------

tmpdir=$(mktemp -d -t drift-lab-install)
trap 'rm -rf "$tmpdir"' EXIT
dmg_path="$tmpdir/drift-lab.dmg"

step "downloading $(basename "$dmg_url") (~7 MB)"
curl -fsSL --progress-bar "$dmg_url" -o "$dmg_path"

# --- Mount + copy ------------------------------------------------------------

step "mounting DMG"
hdiutil attach "$dmg_path" -nobrowse -quiet -mountpoint "$tmpdir/mount"
trap 'hdiutil detach "$tmpdir/mount" -quiet 2>/dev/null || true; rm -rf "$tmpdir"' EXIT

src_app="$tmpdir/mount/$APP_NAME.app"
[[ -d "$src_app" ]] || die "$APP_NAME.app not found inside DMG"

dest_app="/Applications/$APP_NAME.app"
if [[ -d "$dest_app" ]]; then
  warn "removing existing $dest_app"
  rm -rf "$dest_app"
fi

step "copying to /Applications"
cp -R "$src_app" "$dest_app"

# --- Trust -------------------------------------------------------------------

step "clearing com.apple.quarantine (Gatekeeper bypass)"
xattr -dr com.apple.quarantine "$dest_app" 2>/dev/null || true

ok "$APP_NAME installed and trusted at $dest_app"

# --- Launch ------------------------------------------------------------------

if [[ -t 0 ]] && [[ "${DRIFT_LAB_NO_LAUNCH:-}" != "1" ]]; then
  step "launching $APP_NAME"
  open "$dest_app"
fi
