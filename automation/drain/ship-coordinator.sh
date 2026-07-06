#!/usr/bin/env bash
# ship-coordinator.sh — DRAIN-6 /ship loop entrypoint.
#
# Encodes the coordinator tick:
#   1. run disk watchdog preflight
#   2. reconcile autoscale + @farm jobs
#   3. surface needs-review and triage queues
#   4. provide the mandatory isolated-agent STEP-0 command
#
# This script intentionally does not invent a Codex subprocess API. It is the
# durable operator/agent entrypoint that keeps the farm-building half moving and
# makes the next supervisor action explicit.
set -euo pipefail

REPO="${MCNF_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
STATE="${MCNF_FARM_STATE:-$REPO/automation/.state}"
WATCHDOG="${MCNF_DISK_WATCHDOG:-$REPO/install-helpers/disk-watchdog.sh}"
RECONCILE="${MCNF_FARM_RECONCILE:-$REPO/automation/reconciler/farm-reconcile.sh}"
GUARD="$REPO/automation/drain/worktree-guard.sh"
DRY=0
ONCE=1

usage() { sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; }
log() { echo "==> ship-coordinator: $*" >&2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1; shift ;;
    --loop) ONCE=0; shift ;;
    --once) ONCE=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ship-coordinator: unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

print_queue() {
  local file="$1" label="$2"
  if [ -s "$file" ]; then
    log "$label:"
    tail -20 "$file" | sed 's/^/  /' >&2
  else
    log "$label: empty"
  fi
}

tick() {
  if [ -x "$WATCHDOG" ]; then
    log "preflight disk watchdog"
    "$WATCHDOG" || log "disk watchdog reported a non-fatal issue"
  else
    log "disk watchdog missing at $WATCHDOG"
  fi

  log "farm reconcile"
  if [ "$DRY" -eq 1 ]; then
    "$RECONCILE" --dry-run
  else
    "$RECONCILE"
  fi

  print_queue "$STATE/needs-review.txt" "needs-review queue"
  print_queue "$STATE/triage.txt" "triage queue"
  log "agent STEP-0: run '$GUARD' from each isolated worktree before editing"
}

self_test() {
  local td fake_watch fake_rec
  td="$(mktemp -d)"
  trap 'rm -rf "$td"' RETURN
  fake_watch="$td/watchdog"; fake_rec="$td/reconcile"
  printf '#!/usr/bin/env bash\necho watchdog >>"%s/log"\n' "$td" > "$fake_watch"
  printf '#!/usr/bin/env bash\necho reconcile "$@" >>"%s/log"\n' "$td" > "$fake_rec"
  chmod +x "$fake_watch" "$fake_rec"
  MCNF_REPO="$REPO" MCNF_FARM_STATE="$td/state" MCNF_DISK_WATCHDOG="$fake_watch" MCNF_FARM_RECONCILE="$fake_rec" "$0" --dry-run >/dev/null 2>&1
  grep -q '^watchdog$' "$td/log"
  grep -q '^reconcile --dry-run$' "$td/log"
  echo "ship-coordinator: self-test passed"
}

if [ "${SELF_TEST:-0}" = "1" ]; then
  self_test
  exit $?
fi

while :; do
  tick
  [ "$ONCE" -eq 1 ] && break
  sleep "${MCNF_SHIP_TICK_SECS:-60}"
done
