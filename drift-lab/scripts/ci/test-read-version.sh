#!/usr/bin/env bash
# Self-test for read-version.sh.
#
# Asserts the output is a semver triple. We don't pin to a specific version
# because Cargo.toml gets bumped — the test should keep passing across bumps.

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

ver="$(bash "$here/read-version.sh")"
case "$ver" in
  [0-9]*.[0-9]*.[0-9]*)
    echo "PASS: read-version.sh → $ver"
    ;;
  *)
    echo "FAIL: not a semver triple: '$ver'" >&2
    exit 1
    ;;
esac
