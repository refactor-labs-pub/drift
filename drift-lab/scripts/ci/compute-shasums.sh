#!/usr/bin/env bash
# Compute a SHA256SUMS manifest for a Tauri bundle directory.
#
# Usage: compute-shasums.sh <bundle_dir>
#
# Walks the standard Tauri 2 release layout (dmg/, macos/, appimage/, deb/,
# rpm/) and emits one line per shippable artifact:
#
#     <hex>  <relative-path>
#
# into <bundle_dir>/SHA256SUMS. Mirrors the SHA256SUMS that the local
# `make drift-lab-export` target produces, so a CI artifact and a local
# export of the same code are byte-identical (modulo build determinism).
#
# Patterns matched (everything else is ignored — debug symbols, intermediate
# Wix/Bundle files, etc.):
#   *.dmg                     - macOS installer
#   *.app.tar.gz / .sig       - macOS updater payload + signature
#   *.deb                     - Debian package
#   *.AppImage                - Linux portable
#   *.AppImage.tar.gz / .sig  - Linux updater payload + signature
#   *.rpm                     - Red Hat package
#   *.msi / *.msi.zip / .sig  - Windows MSI installer + updater payload
#   *.exe / *.nsis.zip / .sig - Windows NSIS installer + updater payload
#
# Output is sorted by relative path so re-running on the same set of inputs
# produces a byte-identical SHA256SUMS file.

set -euo pipefail

bundle="${1:-}"
if [ -z "$bundle" ]; then
  echo "usage: $0 <bundle_dir>" >&2
  exit 2
fi
if [ ! -d "$bundle" ]; then
  echo "::error::bundle dir not found: $bundle" >&2
  exit 1
fi

out="$bundle/SHA256SUMS"

# -exec shasum -a 256 {} + handles whitespace in filenames ("Drift Lab_*.dmg")
# correctly — xargs without -0 would split on the embedded space. sort -k 2
# orders by the path column so the output is deterministic.
( cd "$bundle" && \
  find . -mindepth 1 -type f \
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
    ! -name SHA256SUMS \
    -exec shasum -a 256 {} + \
  | sort -k 2 \
  | sed 's|  \./|  |' \
) > "$out"

if [ ! -s "$out" ]; then
  echo "::error::no bundle artifacts found under $bundle (SHA256SUMS would be empty)" >&2
  rm -f "$out"
  exit 1
fi

# Round-trip: verify each entry actually hashes to what we just wrote. Catches
# races where a file gets rewritten between find and shasum, or where shasum
# couldn't read a file we listed.
( cd "$bundle" && shasum -a 256 -c SHA256SUMS >/dev/null )

count=$(wc -l < "$out" | tr -d ' ')
echo "wrote $out ($count file(s))"
cat "$out"
