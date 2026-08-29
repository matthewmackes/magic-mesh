#!/usr/bin/env bash
# leftover-units.sh — leftover demand after cargo is fresh at a clean HEAD.
# dest-operator / keep / release-wait are fail-closed (secrets, keep-lint,
# freeze predecessors). They are not a place to park an open-source choice;
# if a dest-backed open-source path exists, take it (source / live-seat).
#
# farm-jobs.sh / farm-reconcile.sh only see @farm:{cargo …}. Once those
# results match HEAD, slots go idle while Remaining epics still have dest,
# live-seat, keep, or source leftovers. That idle is not a broken farm.
# This script is the shared next-act list for every agent runtime. It is
# not a second scheduler: drain-coordinator.sh plan prints it; agents fan
# leftover work from it.
#
# Convention, anywhere in a Remaining epic body:
#   @leftover:{live-seat}
#   @leftover:{dest-operator}
#   @leftover:{keep}
#   @leftover:{source}
#   @leftover:{release-wait}
#
# Usage:
#   leftover-units.sh list       all Remaining leftovers (epic, class)
#   leftover-units.sh runnable   live-seat and source only (can run now)
#   leftover-units.sh parked     dest-operator, keep, release-wait
#   leftover-units.sh --self-test
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WORKLIST="${MCNF_WORKLIST:-$REPO/docs/platform/WORKLIST.md}"

is_class() {
  case "$1" in
    live-seat|dest-operator|keep|source|release-wait) return 0 ;;
    *) return 1 ;;
  esac
}

is_runnable() {
  case "$1" in
    live-seat|source) return 0 ;;
    *) return 1 ;;
  esac
}

parse() {
  [ -f "$WORKLIST" ] || { echo "leftover-units: no worklist at $WORKLIST" >&2; return 1; }
  python3 - "$WORKLIST" <<'PY'
import re, sys
path = sys.argv[1]
valid = {"live-seat", "dest-operator", "keep", "source", "release-wait"}
text = open(path, encoding="utf-8").read().splitlines()
epic = None
status = None
body = []

def flush():
    if not epic or status != "Remaining":
        return
    seen = []
    for line in body:
        for m in re.finditer(r"@leftover:\{([^}]*)\}", line):
            cls = m.group(1).strip()
            if cls in valid and cls not in seen:
                seen.append(cls)
                print(f"{epic}\t{cls}")

for line in text:
    m = re.match(r"^### (WL-[A-Z0-9-]+) - ", line)
    if m:
        flush()
        epic = m.group(1)
        status = None
        body = []
        continue
    if line.startswith("## "):
        flush()
        epic = None
        status = None
        body = []
        continue
    sm = re.match(r"^\s*-\s*Status:\s*(Remaining|Blocked|Awaiting testing|Needs clarification)\s*$", line)
    if sm and epic:
        status = sm.group(1)
        body.append(line)
        continue
    if epic:
        body.append(line)
flush()
PY
}

self_test() {
  local td fails=0
  td="$(mktemp -d "${TMPDIR:-/tmp}/leftover-units.XXXXXX")"
  trap 'rm -rf "$td"' RETURN
  cat >"$td/WORKLIST.md" <<'EOF'
# Platform Worklist
- **3 active epics:** 2 `Remaining`, 0 `Blocked`, 1 `Awaiting testing`, 0 `Needs clarification`.

### WL-FUNC-025 - Files
- Status: Remaining
- Remaining work: leftover is live seat Files. @leftover:{live-seat}
- Verification method: x. @farm:{cargo test -p mde-files}

### WL-REL-006 - Inputs
- Status: Remaining
- Remaining work: Maps dest. @leftover:{dest-operator} @leftover:{release-wait}
- Verification method: x. @farm:{cargo build --workspace}

### WL-FUNC-099 - Closed-looking
- Status: Blocked
- Remaining work: ignore. @leftover:{live-seat}

### WL-TEST-003 - Testing wait
- Status: Awaiting testing
- Remaining work: ignore. @leftover:{live-seat}
EOF
  echo "leftover-units --self-test:"
  local out
  out="$(MCNF_WORKLIST="$td/WORKLIST.md" "$0" list)"
  echo "$out" | grep -qx 'WL-FUNC-025	live-seat' || { echo "  FAIL: live-seat row" >&2; fails=$((fails+1)); }
  echo "$out" | grep -qx 'WL-REL-006	dest-operator' || { echo "  FAIL: dest-operator row" >&2; fails=$((fails+1)); }
  echo "$out" | grep -q 'WL-FUNC-099' && { echo "  FAIL: blocked epic leaked" >&2; fails=$((fails+1)); }
  echo "$out" | grep -q 'WL-TEST-003' && { echo "  FAIL: awaiting-testing epic leaked" >&2; fails=$((fails+1)); }
  out="$(MCNF_WORKLIST="$td/WORKLIST.md" "$0" runnable)"
  echo "$out" | grep -q dest-operator && { echo "  FAIL: dest-operator in runnable" >&2; fails=$((fails+1)); }
  echo "$out" | grep -qx 'WL-FUNC-025	live-seat' || { echo "  FAIL: runnable live-seat" >&2; fails=$((fails+1)); }
  out="$(MCNF_WORKLIST="$td/WORKLIST.md" "$0" parked)"
  echo "$out" | grep -qx 'WL-REL-006	dest-operator' || { echo "  FAIL: parked dest-operator" >&2; fails=$((fails+1)); }
  if [ "$fails" -eq 0 ]; then
    echo "  ok: list/runnable/parked split Remaining leftovers"
    echo "leftover-units: self-test passed"
    return 0
  fi
  echo "leftover-units: SELF-TEST FAILED ($fails)" >&2
  return 1
}

case "${1:-runnable}" in
  --self-test) self_test ;;
  list) parse ;;
  runnable)
    parse | while IFS=$'\t' read -r epic cls; do
      is_runnable "$cls" || continue
      printf '%s\t%s\n' "$epic" "$cls"
    done
    ;;
  parked)
    parse | while IFS=$'\t' read -r epic cls; do
      is_runnable "$cls" && continue
      printf '%s\t%s\n' "$epic" "$cls"
    done
    ;;
  *)
    echo "usage: $0 {list|runnable|parked|--self-test}" >&2
    exit 2
    ;;
esac
