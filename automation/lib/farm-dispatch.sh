#!/usr/bin/env bash
# farm-dispatch.sh — the shared "run a job on the fleet" core (requirement C).
# Given a command, it picks a FREE, ready build node (per-node flock so N nodes
# run N jobs concurrently — optimum hardware use), rsyncs the working tree to it,
# runs the command THERE (the VM does the work, not the caller/AI), and records a
# JSON result. Used by every build-farm automation capability.
#
# Node selection prefers the highest-capacity free node first (BIGBOY → small),
# so big jobs land on big iron. A node is eligible only if reachable + toolchained.
#
# Usage:
#   farm-dispatch.sh run <jobid> "<command>"   sync+run on a free node; writes result
#   farm-dispatch.sh result <jobid>            print the JSON result (if any)
#   farm-dispatch.sh nodes                     show node free/busy/ready state
#
# Env: MCNF_BUILD_NODES (space list, capacity order), MCNF_FARM_KEY, MCNF_FARM_STATE.
set -uo pipefail

KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
# Highest-capacity first (XEN-BIGBOY .52 → small .50/.51) so big jobs get big iron.
NODES="${MCNF_BUILD_NODES:-172.20.0.52 172.20.0.50 172.20.0.51}"
STATE="${MCNF_FARM_STATE:-$(cd "$(dirname "$0")/../.." && pwd)/automation/.state}"
RESULTS="$STATE/results"; LOGS="$STATE/logs"; LOCKS="$STATE/locks"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$RESULTS" "$LOGS" "$LOCKS"

SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=12)
log() { echo "==> dispatch: $*" >&2; }

reachable()   { timeout 4 bash -c "cat </dev/null >/dev/tcp/$1/22" 2>/dev/null; }
toolchained() { "${SSH[@]}" -n "mm@$1" '. "$HOME/.cargo/env" 2>/dev/null; command -v cargo >/dev/null && command -v g++ >/dev/null' 2>/dev/null; }

# run <jobid> <command> — claim a free node, run, record result JSON.
cmd_run() {
  local jobid="${1:?jobid}"; shift
  local command="$*"
  [ -n "$command" ] || { echo "empty command" >&2; return 2; }
  local node="" lockfd
  for n in $NODES; do
    exec {lockfd}>"$LOCKS/$n.lock"
    if flock -n "$lockfd"; then
      if reachable "$n" && toolchained "$n"; then node="$n"; break; fi
      flock -u "$lockfd"; exec {lockfd}>&-   # not ready → release, try next
    else
      exec {lockfd}>&-                        # busy → try next
    fi
  done
  [ -n "$node" ] || { log "no free ready node (all busy/down) — retry later"; return 75; }  # EX_TEMPFAIL

  local started log_file; started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; log_file="$LOGS/$jobid.log"
  log "job $jobid → $node : $command"
  # rsync the tree (dirty ok), then run the command in the repo on the VM.
  rsync -az --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.git/objects/pack/tmp_*' --exclude '/automation/.state' \
    "$REPO/" "mm@$node:magic-mesh/" >>"$log_file" 2>&1
  "${SSH[@]}" "mm@$node" ". \"\$HOME/.cargo/env\"; cd magic-mesh && $command" >>"$log_file" 2>&1
  local exit_code=$?
  local ended; ended="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  flock -u "$lockfd"; exec {lockfd}>&-

  local outcome="pass"; [ "$exit_code" -eq 0 ] || outcome="fail"
  # Record the source rev (with -dirty marker) so a reconciler can tell stale from fresh.
  local commit; commit="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  git -C "$REPO" diff --quiet 2>/dev/null || commit="${commit}-dirty"
  printf '{"jobid":"%s","outcome":"%s","exit":%d,"node":"%s","commit":"%s","command":%s,"started":"%s","ended":"%s","log":"%s"}\n' \
    "$jobid" "$outcome" "$exit_code" "$node" "$commit" "$(printf '%s' "$command" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read()))')" \
    "$started" "$ended" "$log_file" > "$RESULTS/$jobid.json"
  log "job $jobid $outcome (exit $exit_code) on $node — result $RESULTS/$jobid.json"
  [ "$exit_code" -eq 0 ]
}

cmd_result() { cat "$RESULTS/${1:?jobid}.json" 2>/dev/null || { echo "no result for $1" >&2; return 1; }; }

cmd_nodes() {
  printf '  %-16s %-7s %-7s %s\n' NODE REACH TOOLCH LOCK
  for n in $NODES; do
    local r="down" t="-" l="free"
    reachable "$n" && { r="up"; toolchained "$n" && t="ready" || t="bare"; }
    exec {fd}>"$LOCKS/$n.lock"; flock -n "$fd" || l="BUSY"; flock -u "$fd" 2>/dev/null; exec {fd}>&-
    printf '  %-16s %-7s %-7s %s\n' "$n" "$r" "$t" "$l"
  done
}

case "${1:-nodes}" in
  run)    shift; cmd_run "$@" ;;
  result) shift; cmd_result "$@" ;;
  nodes)  cmd_nodes ;;
  -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: farm-dispatch.sh run <jobid> <cmd> | result <jobid> | nodes" >&2; exit 1 ;;
esac
