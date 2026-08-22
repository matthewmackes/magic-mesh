#!/bin/sh
# lint-func033-keep.sh — FUNC-033 leftover: keep own_nebula_ip; no live PBX spawn.
#
# The Kamailio/RTPengine stack is deleted. Leftover is keep
# `own_nebula_ip` in lib `voip_rtt.rs` (other mackesd paths still call it).
# This gate fails if that function is removed, or if crates/ or packaging/
# grow a live `mde-voice-config` / `kamailio-mde` / `rtpengine-mde` spawn.
# Archive, ledger, evidence, salvage, and COMPLIANCE diary are out of scope.
#
# Exit 0 = keep present and live trees clean. Exit 1 = violation.
# --self-test exercises planted keep-missing and live-ref fixtures.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${MCNF_FUNC033_ROOT:-$ROOT}"
KEEP="crates/mesh/mackesd/src/voip_rtt.rs"
FORBIDDEN='mde-voice-config|kamailio-mde|rtpengine-mde'

usage() {
  printf '%s\n' "Usage: install-helpers/lint-func033-keep.sh [--self-test]"
}

keep_present() {
  path="$ROOT/$KEEP"
  if [ ! -f "$path" ] || [ -L "$path" ]; then
    echo "lint-func033-keep: REFUSED: $KEEP is missing or a symlink" >&2
    return 1
  fi
  if ! grep -q 'pub fn own_nebula_ip' "$path"; then
    echo "lint-func033-keep: REFUSED: $KEEP lost pub fn own_nebula_ip" >&2
    return 1
  fi
  return 0
}

live_trees_clean() {
  hits="$(
    {
      find "$ROOT/crates" "$ROOT/packaging" \( -type f -o -type l \) \
        ! -path '*/target/*' \
        -print 2>/dev/null \
      | xargs -r grep -Eln "$FORBIDDEN" 2>/dev/null || true
    }
  )"
  if [ -n "$hits" ]; then
    echo "lint-func033-keep: REFUSED: live PBX name in crates/ or packaging/:" >&2
    printf '%s\n' "$hits" >&2
    return 1
  fi
  return 0
}

scan() {
  keep_present
  live_trees_clean
  echo "lint-func033-keep: PASS: own_nebula_ip kept; crates/packaging have no live PBX spawn"
}

self_test() {
  fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/lint-func033-keep.XXXXXX")"
  trap 'rm -rf "$td"' EXIT
  mkdir -p "$td/crates/mesh/mackesd/src" "$td/packaging"

  # Missing keep.
  if MCNF_FUNC033_ROOT="$td" "$0" >/dev/null 2>&1; then
    echo "  FAIL: missing keep should fail" >&2
    fails=$((fails + 1))
  else
    echo "  ok: missing keep fails"
  fi

  printf '%s\n' 'pub fn other() {}' >"$td/crates/mesh/mackesd/src/voip_rtt.rs"
  if MCNF_FUNC033_ROOT="$td" "$0" >/dev/null 2>&1; then
    echo "  FAIL: keep without own_nebula_ip should fail" >&2
    fails=$((fails + 1))
  else
    echo "  ok: keep without own_nebula_ip fails"
  fi

  printf '%s\n' 'pub fn own_nebula_ip() -> Option<String> { None }' \
    >"$td/crates/mesh/mackesd/src/voip_rtt.rs"
  if MCNF_FUNC033_ROOT="$td" "$0" >/dev/null 2>&1; then
    echo "  ok: keep + clean trees pass"
  else
    echo "  FAIL: keep + clean trees should pass" >&2
    fails=$((fails + 1))
  fi

  printf '%s\n' 'ExecStart=/usr/bin/mde-voice-config' >"$td/packaging/kamailio-mde.service"
  if MCNF_FUNC033_ROOT="$td" "$0" >/dev/null 2>&1; then
    echo "  FAIL: live PBX name in packaging/ should fail" >&2
    fails=$((fails + 1))
  else
    echo "  ok: live PBX name in packaging/ fails"
  fi

  if [ "$fails" -eq 0 ]; then
    echo "lint-func033-keep.sh: self-test passed"
    return 0
  fi
  echo "lint-func033-keep.sh: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) usage ;;
  "") scan ;;
  *) echo "lint-func033-keep: unknown argument" >&2; exit 1 ;;
esac
