#!/usr/bin/env bash
# Print the drift-lab semantic version from drift-lab/src-tauri/Cargo.toml.
#
# Used by .github/workflows/drift-lab-desktop-build.yml to label uploaded
# artifacts and to drive `::notice::` logs. Kept as a script (not an inline
# awk one-liner) so it can be unit-tested by test-read-version.sh.
#
# Exits non-zero with a clear error if the file is missing or no `version =`
# line is found in the top-level `[package]` block.

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"   # drift-lab/scripts/ci/ → repo root
cargo_toml="$repo/drift-lab/src-tauri/Cargo.toml"

if [ ! -f "$cargo_toml" ]; then
  echo "::error::not found: $cargo_toml" >&2
  exit 1
fi

# First top-level `version = "..."` line. Stops before any `[dependencies]`
# section so a dep's `version =` can't shadow the package version.
version="$(awk -F'"' '
  /^\[/ && !/^\[package\]/ { exit }
  /^version *= *"/         { print $2; exit }
' "$cargo_toml")"

if [ -z "$version" ]; then
  echo "::error::no version line in $cargo_toml" >&2
  exit 1
fi

echo "$version"
