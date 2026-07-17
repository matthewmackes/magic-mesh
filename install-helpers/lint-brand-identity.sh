#!/usr/bin/env bash
# lint-brand-identity.sh — QBRAND/WL-UX-004 canonical spelling guard.
#
# The operator selected "Quazar" as the user-facing 12.x codename. This gate
# prevents the superseded "Quasar" spelling from returning to current source,
# generated-user-facing metadata, install helpers, and front-door governance
# docs. Historical archives and lower-case asset paths such as
# assets/brand/quasar are intentionally outside this check.
#
# Run with `--self-test` to verify. Exit 0 = clean, 1 = a violation.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SUPERSEDED='Quasar'

default_paths() {
  local p
  for p in \
    "$ROOT/AI_GOVERNANCE.md" \
    "$ROOT/README.md" \
    "$ROOT/CHANGELOG.md" \
    "$ROOT/docs/NEEDS-OPERATOR.md" \
    "$ROOT/docs/platform/WORKLIST.md" \
    "$ROOT/docs/design/quasar-branding.md" \
    "$ROOT/crates" \
    "$ROOT/install-helpers" \
    "$ROOT/packaging"
  do
    [ -e "$p" ] && printf '%s\n' "$p"
  done
}

allowed_hit() {
  local path="$1" text="$2"
  case "$path" in
    */install-helpers/lint-brand-identity.sh)
      # The guard itself must name the token it rejects and plant it in
      # self-tests.
      return 0
      ;;
    */docs/design/quasar-branding.md)
      [[ "$text" == *'supersedes the earlier "Quasar" spelling'* ]]
      ;;
    */docs/NEEDS-OPERATOR.md)
      [[ "$text" == *'vs "Quasar" (S)'* ]]
      ;;
    *)
      return 1
      ;;
  esac
}

scan() {
  local roots=("$@") raw hit path rest line text rc=0
  raw="$(rg -n --hidden \
    --glob '!target/**' \
    --glob '!docs/worklist-archive/**' \
    --glob '!.git/**' \
    "$SUPERSEDED" "${roots[@]}" 2>/dev/null || true)"

  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    path="${hit%%:*}"
    rest="${hit#*:}"
    line="${rest%%:*}"
    text="${rest#*:}"
    if ! allowed_hit "$path" "$text"; then
      if [ "$rc" -eq 0 ]; then
        echo "lint-brand-identity.sh: superseded brand spelling found:" >&2
      fi
      printf '  %s:%s:%s\n' "$path" "$line" "$text" >&2
      rc=1
    fi
  done <<<"$raw"

  return "$rc"
}

self_test() {
  local td fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/lint-brand-identity.XXXXXX")"
  trap "rm -rf '$td'" EXIT
  mkdir -p "$td/crates/demo/src" "$td/docs/design" "$td/docs"

  printf 'const NAME: &str = "MDE Quazar";\n' >"$td/crates/demo/src/lib.rs"
  if scan "$td/crates" >/dev/null 2>/dev/null; then
    echo "  ok: clean source passes"
  else
    echo "  FAIL: clean source should pass" >&2
    fails=$((fails + 1))
  fi

  printf 'const NAME: &str = "MDE Quasar";\n' >"$td/crates/demo/src/lib.rs"
  if scan "$td/crates" >/dev/null 2>/dev/null; then
    echo "  FAIL: old spelling should fail" >&2
    fails=$((fails + 1))
  else
    echo "  ok: old spelling fails"
  fi

  printf '%s\n' \
    '| 9 | Canonical | supersedes the earlier "Quasar" spelling |' \
    >"$td/docs/design/quasar-branding.md"
  printf '%s\n' \
    '- NAMING-1: brand spelling "Quazar" (Z) vs "Quasar" (S) — resolved' \
    >"$td/docs/NEEDS-OPERATOR.md"
  if scan "$td/docs/design/quasar-branding.md" "$td/docs/NEEDS-OPERATOR.md" >/dev/null 2>/dev/null; then
    echo "  ok: documented old-spelling decision is allowed"
  else
    echo "  FAIL: old-spelling decision lines should be allowed" >&2
    fails=$((fails + 1))
  fi

  if [ "$fails" -eq 0 ]; then
    echo "lint-brand-identity.sh: self-test passed"
    return 0
  fi
  echo "lint-brand-identity.sh: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-}" in
  --self-test)
    self_test
    ;;
  -h|--help)
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    ;;
  *)
    if [ "$#" -gt 0 ]; then
      scan "$@"
    else
      mapfile -t roots < <(default_paths)
      scan "${roots[@]}"
    fi
    echo "lint-brand-identity.sh: clean — current user-facing brand spelling is Quazar"
    ;;
esac
