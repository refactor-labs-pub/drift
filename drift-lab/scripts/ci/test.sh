#!/usr/bin/env bash
# Run every CI helper-script self-test. Used by `make drift-lab-ci-preflight`
# and by the CI workflow itself (so logic bugs surface here, not on the
# expensive matrix runners).

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

bash "$here/test-read-version.sh"
bash "$here/test-compute-shasums.sh"
bash "$here/test-collect-artifacts.sh"

echo "✓ all CI helper-script tests passed"
