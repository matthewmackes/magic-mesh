#!/usr/bin/env bash
# Fail-closed workstation RPM admission for the governed Kiron A-F asset pack.
set -euo pipefail

ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
MANIFEST="${KIRON_MANIFEST:-$ROOT/assets/kiron/manifest-v2.json}"
ASSET_ROOT="${KIRON_ASSET_ROOT:-$ROOT/assets/kiron}"
RPM_MANIFEST="${KIRON_RPM_MANIFEST:-$ROOT/crates/mesh/mackesd/Cargo.toml}"
VERIFIER="$ROOT/install-helpers/verify-kiron-assets.py"

fail() { printf 'verify-kiron-package: FAIL: %s\n' "$*" >&2; exit 1; }

verify_wiring() {
  grep -Eq 'source[[:space:]]*=[[:space:]]*"assets/kiron/manifest-v2.json",[[:space:]]*dest[[:space:]]*=[[:space:]]*"/usr/share/mde/kiron/manifest-v2.json"' "$RPM_MANIFEST" \
    || fail 'base RPM does not ship the governed v2 manifest at the canonical path'
  grep -Eq 'source[[:space:]]*=[[:space:]]*"assets/kiron/payload/\*\*/\*",[[:space:]]*dest[[:space:]]*=[[:space:]]*"/usr/share/mde/kiron/payload/"' "$RPM_MANIFEST" \
    || fail 'base RPM does not ship the governed Kiron payload tree'
}

verify_source_package() {
  local manifest="${KIRON_MANIFEST:-$ROOT/assets/kiron/manifest-v2.json}"
  local asset_root="${KIRON_ASSET_ROOT:-$ROOT/assets/kiron}"
  verify_wiring
  [[ -f "$manifest" ]] || fail "production manifest is missing: $manifest"
  [[ -d "$asset_root/payload" ]] || fail "production payload is missing: $asset_root/payload"
  python3 "$VERIFIER" --root "$asset_root" "$manifest" \
    || fail 'production asset package did not pass governed v2 admission'
  printf 'verify-kiron-package: PASS: governed workstation RPM asset package admitted\n'
}

self_test() {
  python3 "$VERIFIER" --self-test >/dev/null
  verify_wiring
  local output
  if output="$(KIRON_MANIFEST="$ROOT/assets/kiron/definitely-absent-v2.json" verify_source_package 2>&1)"; then
    fail 'self-test admitted a missing production manifest'
  fi
  [[ "$output" == *'production manifest is missing'* ]] \
    || fail 'self-test did not fail at the missing-manifest boundary'
  printf 'verify-kiron-package: self-test PASS (schema hostility + RPM wiring + missing production rejection)\n'
}

case "${1:-}" in
  --self-test) [[ $# -eq 1 ]] || fail '--self-test takes no additional arguments'; self_test ;;
  --source) [[ $# -eq 1 ]] || fail '--source takes no additional arguments'; verify_source_package ;;
  *) fail 'usage: verify-package.sh --source | --self-test' ;;
esac
