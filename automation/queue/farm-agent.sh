#!/usr/bin/env bash
# farm-agent.sh — FARM-AUTO-3 builder agent. Runs ON a build VM (systemd:
# mcnf-farm-agent.service), pulling jobs from the etcd queue and building its
# LOCAL tree — the pull model, fleet-side, no AI. Atomic lease-claim means many
# agents share the queue without double-building; a dead agent's lock expires so
# the job is re-claimable.
#
# Usage:  farm-agent.sh [--once]
#   --once : process at most one job, then exit (for tests). Default: loop forever.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/etcd-lib.sh"
AGENT="${MCNF_AGENT_ID:-$(hostname)}"
REPO="${MCNF_REPO:-$HOME/magic-mesh}"
ONCE=0; [ "${1:-}" = "--once" ] && ONCE=1
log() { echo "[$(date -u +%H:%M:%SZ)] agent($AGENT): $*"; }

process_one() {
  local key jid cmd lease rc
  for key in $(etcd_range_keys "/farm/queue/"); do
    jid="${key#/farm/queue/}"
    lease="$(etcd_lease 1800)"; [ -n "$lease" ] || continue
    if etcd_claim "/farm/lock/$jid" "$AGENT" "$lease"; then
      cmd="$(etcd_get "$key")"
      if [ -z "$cmd" ]; then etcd_del "/farm/lock/$jid"; continue; fi   # raced away
      log "claimed $jid : $cmd"
      ( cd "$REPO" && . "$HOME/.cargo/env" 2>/dev/null; eval "$cmd" ) >"/tmp/farm-$jid.log" 2>&1
      rc=$?
      local outcome=pass; [ $rc -eq 0 ] || outcome=fail
      etcd_put "/farm/result/$jid" "{\"outcome\":\"$outcome\",\"exit\":$rc,\"agent\":\"$AGENT\",\"ended\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}"
      etcd_del "/farm/queue/$jid"     # done — leave the queue
      etcd_del "/farm/lock/$jid"      # release (lease would also expire it)
      log "$jid $outcome (exit $rc)"
      return 0
    fi
  done
  return 1   # nothing claimable this pass
}

log "starting (etcd=$MCNF_ETCD, repo=$REPO)"
while :; do
  process_one || true
  [ "$ONCE" -eq 1 ] && break
  sleep 5
done
