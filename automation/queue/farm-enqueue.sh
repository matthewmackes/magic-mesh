#!/usr/bin/env bash
# farm-enqueue.sh — FARM-AUTO-3 (control host): push the worklist's active @farm
# jobs into the etcd queue and sync the tree to the build nodes so the agents
# (which build their LOCAL copy — the pull model) have current source.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; LIB="$HERE/../lib"; REPO="$(cd "$HERE/../.." && pwd)"
. "$HERE/etcd-lib.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
NODES="${MCNF_BUILD_NODES:-172.20.0.52 172.20.0.50 172.20.0.51}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=12)

echo "==> sync tree to build nodes"
for n in $NODES; do
  timeout 4 bash -c "cat </dev/null >/dev/tcp/$n/22" 2>/dev/null || { echo "  skip $n (down)"; continue; }
  rsync -az --delete -e "${SSH[*]}" --exclude '/target' --exclude '/target-f4*' \
    --exclude '/.git/objects/pack/tmp_*' --exclude '/automation/.state' "$REPO/" "mm@$n:magic-mesh/" \
    && echo "  synced $n"
done

echo "==> enqueue active @farm jobs"
n=0
while IFS=$'\t' read -r jid status task cmd; do
  [ -n "$jid" ] || continue
  etcd_put "/farm/queue/$jid" "$cmd"
  etcd_put "/farm/meta/$jid"  "$task"
  echo "  queued $task/$jid : $cmd"; n=$((n+1))
done < <("$LIB/farm-jobs.sh" active)
echo "==> $n job(s) in the etcd queue"
