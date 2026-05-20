#!/usr/bin/env bash
# Self-test for collect-artifacts.sh.
#
# Builds a synthetic bundle dir matching Tauri 2's release layout, runs
# the script, and asserts every shippable artifact (and only those) is
# flattened into the out dir alongside a valid SHA256SUMS.

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

tmp="$(mktemp -d -t drift-lab-ci-collect.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

bundle="$tmp/bundle"
out="$tmp/out"

# Fixture — one of every shippable (mac + linux + windows) + noise.
# Windows entries are exercised even though that matrix leg is disabled
# today, so the script stays correct when windows comes back.
mkdir -p "$bundle/dmg" "$bundle/macos" "$bundle/appimage" "$bundle/deb" "$bundle/rpm" "$bundle/msi" "$bundle/nsis"
printf 'fake dmg\n'              > "$bundle/dmg/Drift Lab_0.1.0_aarch64.dmg"
printf 'fake app tarball\n'      > "$bundle/macos/Drift Lab_0.1.0_aarch64.app.tar.gz"
printf 'fake app sig\n'          > "$bundle/macos/Drift Lab_0.1.0_aarch64.app.tar.gz.sig"
printf 'fake deb\n'              > "$bundle/deb/drift-lab_0.1.0_amd64.deb"
printf 'fake AppImage\n'         > "$bundle/appimage/drift-lab_0.1.0_amd64.AppImage"
printf 'fake AppImage tarball\n' > "$bundle/appimage/drift-lab_0.1.0_amd64.AppImage.tar.gz"
printf 'fake AppImage sig\n'     > "$bundle/appimage/drift-lab_0.1.0_amd64.AppImage.tar.gz.sig"
printf 'fake rpm\n'              > "$bundle/rpm/drift-lab-0.1.0-1.x86_64.rpm"
printf 'fake msi\n'              > "$bundle/msi/Drift Lab_0.1.0_x64_en-US.msi"
printf 'fake msi zip\n'          > "$bundle/msi/Drift Lab_0.1.0_x64_en-US.msi.zip"
printf 'fake msi sig\n'          > "$bundle/msi/Drift Lab_0.1.0_x64_en-US.msi.zip.sig"
printf 'fake exe\n'              > "$bundle/nsis/Drift Lab_0.1.0_x64-setup.exe"
printf 'fake nsis zip\n'         > "$bundle/nsis/Drift Lab_0.1.0_x64-setup.nsis.zip"
printf 'fake nsis sig\n'         > "$bundle/nsis/Drift Lab_0.1.0_x64-setup.nsis.zip.sig"

# Decoys — must NOT be staged.
printf 'debug symbols\n'         > "$bundle/macos/Drift Lab.dSYM"
mkdir -p "$bundle/macos/Drift Lab.app"
printf 'app bundle internals\n'  > "$bundle/macos/Drift Lab.app/Info.plist"
printf 'intermediate stage\n'    > "$bundle/dmg/bundle.dmg.tmp"

bash "$here/collect-artifacts.sh" "$bundle" "$out" >/dev/null

# Assertions.
expected=(
  'Drift Lab_0.1.0_aarch64.dmg'
  'Drift Lab_0.1.0_aarch64.app.tar.gz'
  'Drift Lab_0.1.0_aarch64.app.tar.gz.sig'
  'drift-lab_0.1.0_amd64.deb'
  'drift-lab_0.1.0_amd64.AppImage'
  'drift-lab_0.1.0_amd64.AppImage.tar.gz'
  'drift-lab_0.1.0_amd64.AppImage.tar.gz.sig'
  'drift-lab-0.1.0-1.x86_64.rpm'
  'Drift Lab_0.1.0_x64_en-US.msi'
  'Drift Lab_0.1.0_x64_en-US.msi.zip'
  'Drift Lab_0.1.0_x64_en-US.msi.zip.sig'
  'Drift Lab_0.1.0_x64-setup.exe'
  'Drift Lab_0.1.0_x64-setup.nsis.zip'
  'Drift Lab_0.1.0_x64-setup.nsis.zip.sig'
  'SHA256SUMS'
)
for f in "${expected[@]}"; do
  if [ ! -f "$out/$f" ]; then
    echo "FAIL: expected file missing from staging dir: $f" >&2
    ls -la "$out" >&2
    exit 1
  fi
done

# 15 expected files (14 artifacts + SHA256SUMS); nothing else.
actual_count=$(find "$out" -maxdepth 1 -type f | wc -l | tr -d ' ')
if [ "$actual_count" != "15" ]; then
  echo "FAIL: staging dir has $actual_count files, expected 15" >&2
  ls -la "$out" >&2
  exit 1
fi

# Decoys excluded.
for noise in 'Drift Lab.dSYM' 'Info.plist' 'bundle.dmg.tmp'; do
  if [ -f "$out/$noise" ]; then
    echo "FAIL: decoy file leaked into staging dir: $noise" >&2
    exit 1
  fi
done

# SHA256SUMS lists exactly 14 artifacts (not itself).
sums_count=$(wc -l < "$out/SHA256SUMS" | tr -d ' ')
if [ "$sums_count" != "14" ]; then
  echo "FAIL: SHA256SUMS has $sums_count lines, expected 14" >&2
  cat "$out/SHA256SUMS" >&2
  exit 1
fi

# Round-trip.
( cd "$out" && shasum -a 256 -c SHA256SUMS >/dev/null ) \
  || { echo "FAIL: shasum -c on staged SHA256SUMS" >&2; exit 1; }

# Empty-bundle case → script must fail loudly.
empty="$tmp/empty"
mkdir -p "$empty/bundle"
if bash "$here/collect-artifacts.sh" "$empty/bundle" "$empty/out" >/dev/null 2>&1; then
  echo "FAIL: collect-artifacts didn't fail on an empty bundle dir" >&2
  exit 1
fi

echo "PASS: collect-artifacts.sh (14 artifacts across mac/linux/windows + SHA256SUMS, decoys excluded, empty-bundle fails)"
