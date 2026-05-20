#!/usr/bin/env bash
# Self-test for compute-shasums.sh.
#
# Builds a synthetic bundle directory matching Tauri 2's release layout,
# populates it with one of every shippable artifact + one noise file (debug
# symbols), runs the script, and asserts:
#   1. SHA256SUMS exists and is non-empty
#   2. It contains exactly one line per shippable artifact (8 in this fixture)
#   3. The noise file is NOT included
#   4. `shasum -a 256 -c` round-trip verification passes
#   5. Output is sorted (deterministic across reruns)

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

tmp="$(mktemp -d -t drift-lab-ci-shasums.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

# Fixture: one of every shippable artifact type (mac + linux + windows) +
# a decoy. Windows patterns are included even though that matrix leg is
# disabled today, so the glob list stays exercised against future builds.
mkdir -p "$tmp/dmg" "$tmp/macos" "$tmp/appimage" "$tmp/deb" "$tmp/rpm" "$tmp/msi" "$tmp/nsis"
printf 'fake dmg\n'               > "$tmp/dmg/Drift Lab_0.1.0_aarch64.dmg"
printf 'fake app tarball\n'       > "$tmp/macos/Drift Lab_0.1.0_aarch64.app.tar.gz"
printf 'fake app sig\n'           > "$tmp/macos/Drift Lab_0.1.0_aarch64.app.tar.gz.sig"
printf 'fake deb\n'               > "$tmp/deb/drift-lab_0.1.0_amd64.deb"
printf 'fake AppImage\n'          > "$tmp/appimage/drift-lab_0.1.0_amd64.AppImage"
printf 'fake AppImage tarball\n'  > "$tmp/appimage/drift-lab_0.1.0_amd64.AppImage.tar.gz"
printf 'fake AppImage sig\n'      > "$tmp/appimage/drift-lab_0.1.0_amd64.AppImage.tar.gz.sig"
printf 'fake rpm\n'               > "$tmp/rpm/drift-lab-0.1.0-1.x86_64.rpm"
printf 'fake msi\n'               > "$tmp/msi/Drift Lab_0.1.0_x64_en-US.msi"
printf 'fake msi zip\n'           > "$tmp/msi/Drift Lab_0.1.0_x64_en-US.msi.zip"
printf 'fake msi sig\n'           > "$tmp/msi/Drift Lab_0.1.0_x64_en-US.msi.zip.sig"
printf 'fake exe\n'               > "$tmp/nsis/Drift Lab_0.1.0_x64-setup.exe"
printf 'fake nsis zip\n'          > "$tmp/nsis/Drift Lab_0.1.0_x64-setup.nsis.zip"
printf 'fake nsis sig\n'          > "$tmp/nsis/Drift Lab_0.1.0_x64-setup.nsis.zip.sig"

# Decoy — must not appear in SHA256SUMS.
printf 'debug symbols, noise\n' > "$tmp/macos/Drift Lab.dSYM"

bash "$here/compute-shasums.sh" "$tmp" >/dev/null

[ -f "$tmp/SHA256SUMS" ] || { echo "FAIL: SHA256SUMS not written" >&2; exit 1; }

count=$(wc -l < "$tmp/SHA256SUMS" | tr -d ' ')
if [ "$count" != "14" ]; then
  echo "FAIL: expected 14 lines in SHA256SUMS, got $count" >&2
  cat "$tmp/SHA256SUMS" >&2
  exit 1
fi

if grep -q 'dSYM' "$tmp/SHA256SUMS"; then
  echo "FAIL: noise file leaked into SHA256SUMS" >&2
  exit 1
fi

# Spot-check a few expected entries (filenames with spaces are the risky case).
for needle in \
  'Drift Lab_0.1.0_aarch64.dmg' \
  'Drift Lab_0.1.0_aarch64.app.tar.gz.sig' \
  'drift-lab_0.1.0_amd64.AppImage.tar.gz' \
  'drift-lab-0.1.0-1.x86_64.rpm' \
  'Drift Lab_0.1.0_x64_en-US.msi' \
  'Drift Lab_0.1.0_x64_en-US.msi.zip.sig' \
  'Drift Lab_0.1.0_x64-setup.exe' \
  'Drift Lab_0.1.0_x64-setup.nsis.zip.sig'
do
  if ! grep -qF "$needle" "$tmp/SHA256SUMS"; then
    echo "FAIL: SHA256SUMS missing entry for $needle" >&2
    cat "$tmp/SHA256SUMS" >&2
    exit 1
  fi
done

# Round-trip: every line must verify.
( cd "$tmp" && shasum -a 256 -c SHA256SUMS >/dev/null ) \
  || { echo "FAIL: shasum -c round-trip" >&2; exit 1; }

# Determinism: a second run on the same inputs must produce the same bytes.
first_hash=$(shasum -a 256 < "$tmp/SHA256SUMS" | awk '{print $1}')
bash "$here/compute-shasums.sh" "$tmp" >/dev/null
second_hash=$(shasum -a 256 < "$tmp/SHA256SUMS" | awk '{print $1}')
if [ "$first_hash" != "$second_hash" ]; then
  echo "FAIL: SHA256SUMS not deterministic across reruns" >&2
  echo "  first:  $first_hash" >&2
  echo "  second: $second_hash" >&2
  exit 1
fi

echo "PASS: compute-shasums.sh (14 artifacts across mac/linux/windows, noise excluded, round-trip ok, deterministic)"
