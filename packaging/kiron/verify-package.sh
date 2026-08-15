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

verify_source_revision() {
  local expected_revision=$1 resolved
  [[ "$expected_revision" =~ ^[0-9a-f]{40}$ \
      && "$expected_revision" != 0000000000000000000000000000000000000000 ]] \
    || fail 'expected source revision must be one non-null lowercase Git commit ID'
  resolved=$(git -C "$ROOT" rev-parse --verify "$expected_revision^{commit}" 2>/dev/null) \
    || fail 'expected source revision is not available in the package repository'
  [[ "$resolved" == "$expected_revision" ]] \
    || fail 'expected source revision did not resolve exactly'
  git -C "$ROOT" cat-file -e "$expected_revision:assets/kiron/manifest-v2.json" 2>/dev/null \
    || fail 'expected source revision does not contain the governed Kiron manifest'
  git -C "$ROOT" cat-file -e "$expected_revision:assets/kiron/payload" 2>/dev/null \
    || fail 'expected source revision does not contain the governed Kiron payload'
  git -C "$ROOT" diff --quiet --no-ext-diff --no-textconv "$expected_revision" -- \
    assets/kiron/manifest-v2.json assets/kiron/payload \
    || fail 'governed Kiron package differs from the expected source revision'
}

verify_source_package() {
  local expected_revision=${1:-}
  local manifest="${KIRON_MANIFEST:-$ROOT/assets/kiron/manifest-v2.json}"
  local asset_root="${KIRON_ASSET_ROOT:-$ROOT/assets/kiron}"
  verify_wiring
  [[ -f "$manifest" ]] || fail "production manifest is missing: $manifest"
  [[ -d "$asset_root/payload" ]] || fail "production payload is missing: $asset_root/payload"
  if [[ -n "$expected_revision" ]]; then
    [[ "$manifest" == "$ROOT/assets/kiron/manifest-v2.json" \
        && "$asset_root" == "$ROOT/assets/kiron" ]] \
      || fail 'source-revision admission requires the canonical Kiron package paths'
  fi
  python3 "$VERIFIER" --root "$asset_root" "$manifest" \
    || fail 'production asset package did not pass governed v2 admission'
  if [[ -n "$expected_revision" ]]; then
    verify_source_revision "$expected_revision"
  fi
  printf 'verify-kiron-package: PASS: governed workstation RPM asset package admitted\n'
}

self_test() {
  python3 "$VERIFIER" --self-test >/dev/null
  verify_wiring
  local output revision
  revision=$(git -C "$ROOT" rev-parse --verify 'HEAD^{commit}') \
    || fail 'self-test could not resolve the package repository revision'
  verify_source_package "$revision" >/dev/null
  if output="$(verify_source_package 0000000000000000000000000000000000000000 2>&1)"; then
    fail 'self-test admitted a null expected source revision'
  fi
  [[ "$output" == *'expected source revision must be one non-null'* ]] \
    || fail 'self-test did not fail at the malformed-revision boundary'
  if output="$(KIRON_MANIFEST="$ROOT/assets/kiron/definitely-absent-v2.json" verify_source_package "$revision" 2>&1)"; then
    fail 'self-test admitted a missing production manifest'
  fi
  [[ "$output" == *'production manifest is missing'* ]] \
    || fail 'self-test did not fail at the missing-manifest boundary'
  printf 'verify-kiron-package: self-test PASS (schema hostility + RPM wiring + missing production rejection)\n'
}

case "${1:-}" in
  --self-test) [[ $# -eq 1 ]] || fail '--self-test takes no additional arguments'; self_test ;;
  --source)
    case "$#" in
      1) verify_source_package ;;
      3)
        [[ "$2" == --expected-source-revision ]] \
          || fail 'usage: verify-package.sh --source [--expected-source-revision REV] | --self-test'
        verify_source_package "$3"
        ;;
      *) fail 'usage: verify-package.sh --source [--expected-source-revision REV] | --self-test' ;;
    esac
    ;;
  *) fail 'usage: verify-package.sh --source [--expected-source-revision REV] | --self-test' ;;
esac
