#!/usr/bin/env bash
# Stage every shippable artifact from a Tauri bundle dir into a flat output
# dir, then compute SHA256SUMS inside it.
#
# Usage: collect-artifacts.sh <bundle_dir> <out_dir>
#
# Mirrors the layout `make drift-lab-export` produces in
# dist/drift-lab/<version>/ — flat dir with one of every shippable artifact
# (dmg/deb/AppImage/rpm + updater tarballs + signatures) plus SHA256SUMS.
#
# CI uses this to produce a single, reviewer-friendly artifact zip per
# platform. The script is unit-tested by test-collect-artifacts.sh against
# a synthetic fixture so the glob list can't silently drift from what Tauri
# actually emits.

set -euo pipefail

bundle="${1:-}"
out="${2:-}"
if [ -z "$bundle" ] || [ -z "$out" ]; then
  echo "usage: $0 <bundle_dir> <out_dir>" >&2
  exit 2
fi
if [ ! -d "$bundle" ]; then
  echo "::error::bundle dir not found: $bundle" >&2
  exit 1
fi

mkdir -p "$out"

# find -exec cp handles whitespace in filenames (e.g. "Drift Lab_*.dmg")
# correctly. -maxdepth 3 covers the standard Tauri layout where artifacts
# live one level under bundle/ (bundle/dmg/foo.dmg, bundle/macos/bar.app.tar.gz,
# bundle/msi/baz.msi, bundle/nsis/qux.exe).
#
# Pattern list covers every shippable artifact across mac/linux/windows so
# the script stays correct as platforms come and go from the matrix. A
# platform that isn't currently built just contributes zero matches.
find "$bundle" -maxdepth 3 -type f \
  \( -name '*.dmg' \
  -o -name '*.deb' \
  -o -name '*.AppImage' \
  -o -name '*.rpm' \
  -o -name '*.app.tar.gz' \
  -o -name '*.app.tar.gz.sig' \
  -o -name '*.AppImage.tar.gz' \
  -o -name '*.AppImage.tar.gz.sig' \
  -o -name '*.msi' \
  -o -name '*.msi.zip' \
  -o -name '*.msi.zip.sig' \
  -o -name '*.exe' \
  -o -name '*.nsis.zip' \
  -o -name '*.nsis.zip.sig' \) \
  -exec cp {} "$out/" \;

# Empty staging dir == cargo tauri build emitted no bundles. Fail loudly
# here rather than producing an empty upload-artifact.
if [ -z "$(ls -A "$out" 2>/dev/null)" ]; then
  echo "::error::no shippable artifacts under $bundle — nothing to stage" >&2
  exit 1
fi

# Manifest.
here="$(cd "$(dirname "$0")" && pwd)"
bash "$here/compute-shasums.sh" "$out" >/dev/null

echo "staged to $out:"
# Plain shell glob — portable to BSD find (macOS) and handles whitespace in
# filenames (which `ls -1 | sed` would mangle). The empty-bundle case is
# already caught above, so `*` will always expand to at least one entry.
( cd "$out" && for f in *; do echo "  $f"; done )
