#!/usr/bin/env bash
# xcp-parallel.sh — run the build/test gates across MULTIPLE slots at once, so the
# slow poles (cargo test, clippy) run concurrently on different VMs instead of
# serially on one. The "parallel work + testing always in flight" engine for the
# XCP build farm (operator goal 2026-06-20). Reads the slots xcp-slots.sh
# registered in .xcp-slots.conf and drives xcp-build.sh per slot.
#
# Usage:
#   xcp-parallel.sh gates [slot...]        split the standard gate set across slots
#   xcp-parallel.sh run "<gateA gateB>" "<gateC>" ...   explicit per-slot gate groups
#                                           (one quoted group per slot, in order)
#
# Standard gate set: test clippy fmt boundary carbon (heavy gates spread first,
# so `test` and `clippy` land on different slots). With one slot, runs serially.
# Each slot syncs once, then runs its assigned gates (--no-sync). Results land in
# .xcp-build/results/; a summary table + overall pass/fail prints at the end.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SLOTS_CONF="${MCNF_SLOTS_CONF:-$REPO/.xcp-slots.conf}"
RESULTS_DIR="$REPO/.xcp-build/results"
XB="$REPO/install-helpers/xcp-build.sh"
log() { echo "==> xcp-parallel: $*" >&2; }
die() { echo "!! xcp-parallel: $*" >&2; exit 1; }

# registered slot names (or the env fallback "main")
mapfile -t ALL_SLOTS < <(grep -vE '^\s*#' "$SLOTS_CONF" 2>/dev/null | awk 'NF{print $1}')
[ ${#ALL_SLOTS[@]} -gt 0 ] || ALL_SLOTS=(main)

run_groups() { # args: one gate-group string per slot, aligned to SLOTS[]
  local -n slots=$1; shift
  local groups=("$@")
  mkdir -p "$RESULTS_DIR"
  local start; start="$(date +%s)"
  local pids=()
  local i=0
  for s in "${slots[@]}"; do
    local grp="${groups[$i]:-}"; i=$((i+1))
    [ -z "$grp" ] && continue
    log "slot '$s' ← gates: $grp"
    ( "$XB" sync --slot "$s" >/dev/null 2>&1 || { echo "sync failed on $s" >&2; exit 1; }
      for g in $grp; do "$XB" gate "$g" --slot "$s" --no-sync >/dev/null 2>&1 || true; done
    ) &
    pids+=("$!")
  done
  for p in "${pids[@]}"; do wait "$p" || true; done

  # summarize from the result JSONs produced during this run
  echo
  printf '%-8s %-10s %-6s %s\n' SLOT GATE STATUS DURs
  local overall=0
  for f in "$RESULTS_DIR"/*.json; do
    [ -f "$f" ] || continue
    local ep; ep="$(basename "$f" | sed -E 's/^[^-]+-([0-9]+)-.*/\1/')"
    [ "$ep" -ge "$start" ] 2>/dev/null || continue
    if command -v python3 >/dev/null; then
      read -r slot gate ok dur < <(python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print(d.get("slot","?"), d.get("gate","?"), d.get("ok",False), d.get("duration_s",0))' "$f")
    else
      slot="?"; gate="?"; ok=$(grep -o '"ok":[a-z]*' "$f"|head -1|cut -d: -f2); dur="?"
    fi
    local st=PASS; [ "$ok" = "True" ] || [ "$ok" = "true" ] || { st=FAIL; overall=1; }
    printf '%-8s %-10s %-6s %ss\n' "$slot" "$gate" "$st" "$dur"
  done
  echo
  [ $overall -eq 0 ] && log "ALL GREEN" || log "FAILURES above — xcp-build.sh result <file> for the tail"
  return $overall
}

CMD="${1:-gates}"; shift || true
case "$CMD" in
  gates)
    SLOTS=("$@"); [ ${#SLOTS[@]} -gt 0 ] || SLOTS=("${ALL_SLOTS[@]}")
    # spread heavy gates first so test/clippy land on different slots
    ORDERED=(test clippy fmt boundary carbon)
    GROUPS=(); for _ in "${SLOTS[@]}"; do GROUPS+=(""); done
    i=0; for g in "${ORDERED[@]}"; do idx=$(( i % ${#SLOTS[@]} )); GROUPS[$idx]="${GROUPS[$idx]} $g"; i=$((i+1)); done
    log "splitting gates across ${#SLOTS[@]} slot(s): ${SLOTS[*]}"
    run_groups SLOTS "${GROUPS[@]}"
    ;;
  run)
    [ $# -ge 1 ] || die "run needs one quoted gate-group per slot"
    SLOTS=("${ALL_SLOTS[@]:0:$#}")
    [ ${#SLOTS[@]} -eq $# ] || die "have ${#ALL_SLOTS[@]} slots but $# groups given"
    run_groups SLOTS "$@"
    ;;
  *) die "unknown command '$CMD' (gates | run)";;
esac
