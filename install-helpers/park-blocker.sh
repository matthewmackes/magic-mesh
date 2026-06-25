#!/usr/bin/env bash
# park-blocker.sh — DRAIN-5 (park-and-continue, operator-locked 2026-06-24). The
# autonomous drain must NEVER stall on one item: when a unit hits a live-infra /
# artifact / gate blocker it can't clear from a build, park it and move on.
#
# This makes "park" one mechanical, idempotent command instead of an ad-hoc hand
# edit (so it can't be skipped or done inconsistently). It:
#   1. flips the unit's worklist marker  [ ]/[>]  ->  [!]  and annotates the line
#      with  _(BLOCKED: <reason> — see NEEDS-OPERATOR.md)_  (if not already),
#   2. surfaces the unit in docs/NEEDS-OPERATOR.md under "Parked by the drain loop",
#   3. exits 0 — so the caller (the drain loop) continues with the next unit.
#
#   ./install-helpers/park-blocker.sh <TASK-ID> "<reason>"
#   e.g. ./install-helpers/park-blocker.sh CONNECT-3 "needs a live lighthouse to verify"
#
# Overridable for tests:
#   MCNF_WORKLIST  (default docs/WORKLIST.md)   MCNF_NEEDS (default docs/NEEDS-OPERATOR.md)
set -uo pipefail

ID="${1:-}"
REASON="${2:-}"
if [ -z "$ID" ] || [ -z "$REASON" ]; then
  echo "usage: $0 <TASK-ID> \"<reason>\"" >&2; exit 2
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
WORKLIST="${MCNF_WORKLIST:-$ROOT/docs/WORKLIST.md}"
NEEDS="${MCNF_NEEDS:-$ROOT/docs/NEEDS-OPERATOR.md}"
STAMP="$(date -u +%Y-%m-%d)"

[ -f "$WORKLIST" ] || { echo "park-blocker: no worklist at $WORKLIST" >&2; exit 2; }

# --- 1. flip the first OPEN ([ ]/[>]) bullet for this id to [!] + annotate -----
export PB_ID="$ID" PB_REASON="$REASON"
tmp="$(mktemp)"
status="$(awk '
  BEGIN { id = ENVIRON["PB_ID"]; reason = ENVIRON["PB_REASON"]; done = 0; already = 0;
          # id boundary: **<ID> followed by a non-identifier char (: ( space *) or EOL
          pat = "[*][*]" id "([^A-Za-z0-9-]|$)"; }
  {
    if (!done && $0 ~ pat) {
      if ($0 ~ /^- \[!\] /) { already = 1; done = 1; print; next }       # already parked
      if ($0 ~ /^- \[[ >]\] /) {
        sub(/^- \[[ >]\] /, "- [!] ");
        if ($0 !~ /_\(BLOCKED/) $0 = $0 " _(BLOCKED: " reason " — see NEEDS-OPERATOR.md)_";
        done = 1; print; next
      }
    }
    print
  }
  END { if (already) print "ALREADY" > "/dev/stderr";
        else if (done) print "FLIPPED" > "/dev/stderr";
        else print "NOTFOUND" > "/dev/stderr"; }
' "$WORKLIST" 2>&1 >"$tmp")"

case "$status" in
  FLIPPED)  mv "$tmp" "$WORKLIST"; echo "park-blocker: $ID -> [!] in $(basename "$WORKLIST")";;
  ALREADY)  rm -f "$tmp"; echo "park-blocker: $ID already [!] (idempotent)";;
  NOTFOUND) rm -f "$tmp"; echo "park-blocker: no OPEN ([ ]/[>]) bullet for '$ID' in $WORKLIST" >&2; exit 1;;
  *)        rm -f "$tmp"; echo "park-blocker: unexpected awk status '$status'" >&2; exit 1;;
esac

# --- 2. surface in NEEDS-OPERATOR.md (idempotent on the id) ---------------------
SECTION="## Parked by the drain loop (DRAIN-5)"
[ -f "$NEEDS" ] || printf '# Needs-Operator — blocked worklist items\n' >"$NEEDS"
if ! grep -qF "$SECTION" "$NEEDS"; then
  printf '\n%s\n\nUnits the drain loop parked automatically (a live-infra/artifact/gate blocker it could not clear from a build). Each needs an operator/live action.\n' "$SECTION" >>"$NEEDS"
fi
# one line per id: refresh it if present, else append.
if grep -qE "^- \*\*$ID\*\* " "$NEEDS"; then
  echo "park-blocker: $ID already listed in $(basename "$NEEDS") (idempotent)"
else
  printf -- '- **%s** (parked %s) — %s\n' "$ID" "$STAMP" "$REASON" >>"$NEEDS"
  echo "park-blocker: surfaced $ID in $(basename "$NEEDS")"
fi

# --- 3. never stall: the loop continues -----------------------------------------
exit 0
