#!/usr/bin/env bash
# WL-FUNC-020 — focused Android/Cuttlefish packaging contract entrypoint.
#
# This is a static and fixture gate. It proves that the packaging manifest,
# guest-tool receipt, and placement-readiness verifier are wired together. It
# never starts Cuttlefish and never upgrades tooling evidence into guest boot,
# package-installation, display, or launch proof.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ANDROID="$ROOT/packaging/android"
MANIFEST_VERIFY="$ANDROID/verify-manifest.sh"
TOOL_READINESS="$ANDROID/record-guest-tool-readiness.sh"
PLACEMENT_READINESS="$ROOT/install-helpers/verify-cuttlefish-readiness.py"

fail() {
    echo "verify-android-contract: $*" >&2
    exit 1
}

[ -x "$MANIFEST_VERIFY" ] || fail "Android manifest verifier is not executable"
[ -x "$TOOL_READINESS" ] || fail "guest-tool readiness recorder is not executable"
[ -x "$PLACEMENT_READINESS" ] || fail "Cuttlefish placement verifier is not executable"

bash -n "$MANIFEST_VERIFY" "$TOOL_READINESS" "$0"
python3 -m py_compile "$PLACEMENT_READINESS"

# Keep the packaging path explicit: readiness must call the real Android
# manifest verifier, not a second permissive parser or an always-success stub.
grep -Fq 'packaging/android/verify-manifest.sh' "$PLACEMENT_READINESS" \
    || fail "placement verifier is not wired to the Android manifest verifier"

"$MANIFEST_VERIFY" --self-test >/dev/null
"$TOOL_READINESS" --self-test >/dev/null
"$PLACEMENT_READINESS" --self-test >/dev/null

echo "Android/Cuttlefish packaging contract checks passed"
