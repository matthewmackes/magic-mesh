#!/usr/bin/env bash
# park-blocker.sh — DRAIN-5: the mechanical "park-and-continue" helper.
#
# When an autonomous unit hits a blocker the loop must NOT resolve itself (a live
# fleet, an operator secret, a gated activation, a missing upstream dep), park it
# instead of stalling: (1) append a structured entry to docs/NEEDS-OPERATOR.md
# (id + reason + unblock-action) and (2) flip that unit's worklist marker to `[!]`.
# The loop then advances to the next unblocked unit — zero whole-loop stalls on one
# item (docs/design/autonomous-drain.md L3).
#
# Usage:
#   park-blocker.sh --id <ID> --reason <text> --unblock <action> [opts]
#   park-blocker.sh --self-test
#   park-blocker.sh -h | --help
#
# Options:
#   --worklist <file>   worklist to flip the marker in (default docs/WORKLIST.md
#                       or $MCNF_WORKLIST)
#   --needs <file>      blocker log to append to    (default docs/NEEDS-OPERATOR.md
#                       or $MCNF_NEEDS_OPERATOR)
#   --dry-run           show what WOULD change; touch nothing (operates on copies)
#
# Idempotent: re-parking an id already in NEEDS-OPERATOR.md is a no-op append; the
# marker flip is a no-op if the id's marker is already `[!]`. A `--id` that is NOT a
# worklist task line (e.g. a sub-note like FOO-ACTIVATE) still gets a NEEDS-OPERATOR
# entry; the marker flip is a warned no-op (nothing in the worklist to flip) — so
# this NEVER edits the worklist for an id that isn't a real task.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORKLIST="${MCNF_WORKLIST:-$REPO/docs/WORKLIST.md}"
NEEDS="${MCNF_NEEDS_OPERATOR:-$REPO/docs/NEEDS-OPERATOR.md}"

# ensure_header <needs-file> — create the blocker log with its preamble if absent.
ensure_header() {
  local needs="$1"
  [ -f "$needs" ] && return 0
  cat > "$needs" <<'HDR'
# NEEDS-OPERATOR — parked blockers awaiting an operator action

Entries are appended by `install-helpers/park-blocker.sh` (DRAIN-5). Each is a
unit the autonomous loop **parked** (`[!]` in `docs/WORKLIST.md`) because it needs
a live fleet, an operator secret, or a gated activation the loop must not perform
itself. Clear an entry by doing its **unblock** action, then flip the worklist
marker back off `[!]`.
HDR
}

# entry_exists <needs-file> <id>
entry_exists() { grep -Fxq "## $2" "$1" 2>/dev/null; }

# append_blocker <needs-file> <id> <reason> <unblock> — append one structured
# section (idempotent: skip if the id already has one). Echoes appended|exists.
append_blocker() {
  local needs="$1" id="$2" reason="$3" unblock="$4"
  ensure_header "$needs"
  if entry_exists "$needs" "$id"; then echo "exists"; return 0; fi
  {
    printf '\n## %s\n' "$id"
    printf -- '- **parked:** %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u)"
    printf -- '- **reason:** %s\n' "$reason"
    printf -- '- **unblock:** %s\n' "$unblock"
  } >> "$needs"
  echo "appended"
}

# flip_marker <worklist-file> <id> — rewrite the `- [x]` marker of the worklist
# task line whose bolded id is exactly <id> (the line contains "**<id>:") to
# `- [!]`. Echoes flipped|already|not-found. NEVER rewrites the file unless a
# matching line is actually found (so a non-task id can't churn the worklist).
flip_marker() {
  local wl="$1" id="$2" needle tmp out rc
  [ -f "$wl" ] || { echo "not-found"; return 0; }
  needle="**$id:"
  # Already parked? (its marker is [!]) → nothing to do.
  if grep -Fq "$needle" "$wl" 2>/dev/null && \
     grep -F "$needle" "$wl" | grep -qE '^- \[!\] '; then
    echo "already"; return 0
  fi
  tmp="$(mktemp "${TMPDIR:-/tmp}/mcnf-park.XXXXXX")"
  # awk: flip the marker ONLY on the task line that literally contains "**<id>:".
  # `[^]]*` inside the brackets handles any marker incl. the multibyte ✓.
  if awk -v needle="$needle" '
        index($0, needle) && /^- \[[^]]*\] \*\*/ { sub(/^- \[[^]]*\]/, "- [!]"); flipped=1 }
        { print }
        END { exit (flipped ? 0 : 9) }
      ' "$wl" > "$tmp"; then
    out="flipped"
  else
    rc=$?
    if [ "$rc" -eq 9 ]; then rm -f "$tmp"; echo "not-found"; return 0; fi
    rm -f "$tmp"; echo "error" >&2; return 1
  fi
  # Only replace the real file when something actually changed.
  if cmp -s "$tmp" "$wl"; then rm -f "$tmp"; echo "already"; return 0; fi
  cat "$tmp" > "$wl"; rm -f "$tmp"
  echo "$out"
}

# ---------------------------------------------------------------------------
# --self-test — dry-run on temp copies; touches no real file.
# ---------------------------------------------------------------------------
self_test() {
  local fails=0 wl needs
  check() { if [ "$2" = "$3" ]; then echo "  ok: $1"
            else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1)); fi; }
  echo "park-blocker --self-test:"

  wl="$(mktemp "${TMPDIR:-/tmp}/mcnf-park-wl.XXXXXX")"
  needs="$(mktemp "${TMPDIR:-/tmp}/mcnf-park-no.XXXXXX")"; rm -f "$needs"  # exercise header-create
  cat > "$wl" <<'WL'
- [ ] **FOO-1: a real task.** body
- [>] **FOO-2: in progress.** body
- [✓] **FOO-3: done already.** body
WL

  # --- marker flip ---
  check "flip [ ] task → flipped" "$(flip_marker "$wl" FOO-1)" flipped
  check "FOO-1 line now [!]" \
    "$(grep -c '^- \[!\] \*\*FOO-1:' "$wl")" 1
  check "re-park FOO-1 → already" "$(flip_marker "$wl" FOO-1)" already
  check "flip [>] task → flipped" "$(flip_marker "$wl" FOO-2)" flipped
  check "flip multibyte [✓] task → flipped" "$(flip_marker "$wl" FOO-3)" flipped
  check "FOO-3 line now [!] (✓ marker handled)" \
    "$(grep -c '^- \[!\] \*\*FOO-3:' "$wl")" 1
  check "unknown id → not-found" "$(flip_marker "$wl" NOPE-9)" not-found
  # The unknown-id flip must NOT have churned the file (still 3 lines, all [!] now).
  check "worklist line count unchanged" "$(grep -c '^- ' "$wl")" 3

  # --- NEEDS-OPERATOR append ---
  check "append creates + appends" \
    "$(append_blocker "$needs" FOO-1 'needs live fleet' 'operator runs X')" appended
  check "header was created" "$(grep -c '^# NEEDS-OPERATOR' "$needs")" 1
  check "entry id present"    "$(grep -Fxc '## FOO-1' "$needs")" 1
  check "reason recorded"     "$(grep -Fc 'needs live fleet' "$needs")" 1
  check "unblock recorded"    "$(grep -Fc 'operator runs X' "$needs")" 1
  check "re-append → exists (idempotent)" \
    "$(append_blocker "$needs" FOO-1 'needs live fleet' 'operator runs X')" exists
  check "still one entry"     "$(grep -Fxc '## FOO-1' "$needs")" 1

  rm -f "$wl" "$needs"
  if [ "$fails" -eq 0 ]; then echo "park-blocker: self-test passed"; return 0; fi
  echo "park-blocker: SELF-TEST FAILED ($fails)" >&2; return 1
}

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
ID=""; REASON=""; UNBLOCK=""; DRY=0
[ $# -gt 0 ] || { sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
while [ $# -gt 0 ]; do case "$1" in
  --self-test) self_test; exit $? ;;
  -h|--help)   sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  --id)        ID="$2"; shift 2 ;;
  --reason)    REASON="$2"; shift 2 ;;
  --unblock)   UNBLOCK="$2"; shift 2 ;;
  --worklist)  WORKLIST="$2"; shift 2 ;;
  --needs)     NEEDS="$2"; shift 2 ;;
  --dry-run)   DRY=1; shift ;;
  *) echo "park-blocker: unknown arg: $1" >&2; exit 2 ;;
esac; done

[ -n "$ID" ]      || { echo "park-blocker: --id is required" >&2; exit 2; }
[ -n "$REASON" ]  || { echo "park-blocker: --reason is required" >&2; exit 2; }
[ -n "$UNBLOCK" ] || { echo "park-blocker: --unblock is required" >&2; exit 2; }

if [ "$DRY" -eq 1 ]; then
  # Operate on COPIES so nothing real is touched; report what would change.
  tno="$(mktemp)"; [ -f "$NEEDS" ] && cp "$NEEDS" "$tno"
  twl="$(mktemp)"; [ -f "$WORKLIST" ] && cp "$WORKLIST" "$twl"
  echo "park-blocker --dry-run ($ID):"
  echo "  NEEDS-OPERATOR ($NEEDS): $(append_blocker "$tno" "$ID" "$REASON" "$UNBLOCK")"
  echo "  worklist marker ($WORKLIST): $(flip_marker "$twl" "$ID")"
  rm -f "$tno" "$twl"
  exit 0
fi

echo "park-blocker: parking $ID"
echo "  NEEDS-OPERATOR: $(append_blocker "$NEEDS" "$ID" "$REASON" "$UNBLOCK") ($NEEDS)"
echo "  worklist marker: $(flip_marker "$WORKLIST" "$ID") ($WORKLIST)"
