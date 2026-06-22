#!/usr/bin/env bash
# farm-reconcile.sh — FARM-AUTO-4: declarative GitOps reconciler.
#
# Desired state = the worklist's active @farm jobs. Each run converges the farm to
# "every active job has a FRESH result for the current source rev". Idempotent: a
# job whose recorded result already matches HEAD (clean tree) is skipped, so the
# timer is cheap when nothing changed. Jobs that need running are dispatched
# concurrently across the free nodes (dispatch's per-node flock packs them).
#
# Runs from a systemd timer on the control host (packaging/systemd/
# mcnf-farm-reconcile.{service,timer}) — fleet-side, no AI in the loop.
#
# Usage: farm-reconcile.sh [--once] [--dry-run]
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LIB="$HERE/../lib"
REPO="$(cd "$HERE/../.." && pwd)"
STATE="${MCNF_FARM_STATE:-$REPO/automation/.state}"
RESULTS="$STATE/results"
SUMMARY="$STATE/farm-status.txt"
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1
mkdir -p "$RESULTS"

rev() { local r; r="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)"; git -C "$REPO" diff --quiet 2>/dev/null || r="${r}-dirty"; printf '%s' "$r"; }
CUR="$(rev)"
log() { echo "[$(date -u +%H:%M:%SZ)] reconcile: $*"; }

# Is this job's result fresh for the current rev?  (a -dirty tree is never fresh)
is_fresh() {
  local jid="$1"; local f="$RESULTS/$jid.json"
  [ -f "$f" ] || return 1
  case "$CUR" in *-dirty) return 1;; esac
  local rc; rc="$(python3 -c "import json,sys;print(json.load(open('$f')).get('commit',''))" 2>/dev/null)"
  [ "$rc" = "$CUR" ]
}

log "rev=$CUR  jobs from $(basename "$(cd "$LIB/../.." && pwd)")/docs/WORKLIST.md"
declare -a NEED=()
declare -A CMD=()
while IFS=$'\t' read -r jid status task cmd; do
  [ -n "$jid" ] || continue
  if is_fresh "$jid"; then
    log "  skip  $task/$jid (fresh @ $CUR)"
  else
    log "  need  $task/$jid : $cmd"
    NEED+=("$jid"); CMD["$jid"]="$cmd"
  fi
done < <("$LIB/farm-jobs.sh" active)

if [ "${#NEED[@]}" -eq 0 ]; then log "nothing to do — farm converged @ $CUR"; exit 0; fi
if [ "$DRY" -eq 1 ]; then log "dry-run: would dispatch ${#NEED[@]} job(s)"; exit 0; fi

# Dispatch each needed job in the background; retry-on-busy (EX_TEMPFAIL=75) so
# jobs queue onto nodes as they free up. dispatch's flock serializes per node.
for jid in "${NEED[@]}"; do
  ( while :; do "$LIB/farm-dispatch.sh" run "$jid" "${CMD[$jid]}"; rc=$?; [ "$rc" -eq 75 ] || break; sleep 5; done ) &
done
wait

# Summary (the report-back; the result JSONs are the per-job record).
{
  echo "MCNF build-farm status @ $(date -u +%Y-%m-%dT%H:%M:%SZ)  rev=$CUR"
  for jid in "${NEED[@]}"; do
    o="$(python3 -c "import json;print(json.load(open('$RESULTS/$jid.json')).get('outcome','?'))" 2>/dev/null || echo missing)"
    n="$(python3 -c "import json;print(json.load(open('$RESULTS/$jid.json')).get('node','?'))" 2>/dev/null || echo '?')"
    printf '  %-12s %-5s %s  (%s)\n' "$jid" "$o" "${CMD[$jid]}" "$n"
  done
} | tee "$SUMMARY"
fails="$(for jid in "${NEED[@]}"; do python3 -c "import json;print(json.load(open('$RESULTS/$jid.json')).get('outcome',''))" 2>/dev/null; done | grep -c fail)"
log "done: ${#NEED[@]} dispatched, $fails failed"
[ "$fails" -eq 0 ]
